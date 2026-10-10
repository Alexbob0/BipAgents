"""``python -m bipinstall [--dry-run]``: installs BipAgents for the person running it, in their own account, without
admin rights. Asks where the model is, picks a setup that fits the machine, creates the first agents, and shows the
QR code to scan in the app. `--dry-run` shows every step and command without changing anything."""
from __future__ import annotations

import argparse
import asyncio
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import List, Optional

import httpx
import yaml

from . import hermes, models
from .detect import Machine, detect
from .plan import make_plan
from .ports import pick_block
from .stack import AgentEntry, Install, bridge_service, bridge_toml, pocket_service, tailscale_commands

REPO = Path(__file__).resolve().parents[2]
TEMPLATES = yaml.safe_load((Path(__file__).with_name("templates.yaml")).read_text(encoding="utf-8"))


def say(text: str = "") -> None:
    print(text, flush=True)


def ask(question: str, default: str = "") -> str:
    shown = f" [{default}]" if default else ""
    try:
        answer = input(f"{question}{shown} : ").strip()
    except EOFError:  # no one to answer (answers given in advance ran out): stop rather than ask forever
        say("\nPas de réponse : installation interrompue, rien d'autre n'a été modifié.")
        raise SystemExit(1)
    return answer or default


def choose(question: str, options: List[str], default: int = 1) -> int:
    for i, option in enumerate(options, 1):
        say(f"  {i}. {option}")
    while True:
        answer = ask(question, str(default))
        if answer.isdigit() and 1 <= int(answer) <= len(options):
            return int(answer)


# -- the model -----------------------------------------------------------------------------------------

def pick_model(machine: Machine) -> tuple[hermes.Model, bool]:
    say("\nOù tourne le modèle (le « cerveau » des agents) ?")
    where = choose("Choix", ["Sur une autre machine de mon réseau (PC avec GPU, Spark, Mac…)",
                             "Dans le cloud, par API (OpenRouter, OpenAI, Anthropic, Mistral)",
                             "Sur cette machine (Ollama ou LM Studio déjà installé)"])
    if where == 2:
        provider = models.CLOUD[choose("Fournisseur", [p.name for p in models.CLOUD]) - 1]
        say(f"Ta clé se crée ici : {provider.key_hint}")
        key = ask("Clé d'API")
        name = ask("Modèle (identifiant exact)", {"openrouter": "anthropic/claude-sonnet-5.5", "openai": "gpt-5.1",
                                                   "anthropic": "claude-sonnet-5-5", "mistral": "mistral-large-latest"}
                   .get(provider.key, ""))
        hermes_provider = {"openrouter": "openrouter", "openai": "openai-api", "anthropic": "anthropic"}.get(provider.key, "custom")
        model = hermes.Model(hermes_provider, name, base_url=provider.base_url, api_key=key)
        return verify(model), False
    say("Je cherche les serveurs de modèles sur ton réseau…")
    found = asyncio.run(models.discover(machine.lan_addresses if where == 1 else []))
    found = [s for s in found if (s.base_url.startswith("http://127.0.0.1")) == (where == 3)]
    if not found:
        say("Aucun trouvé automatiquement.")
        base = ""
        while not base.startswith(("http://", "https://")):
            base = ask("Adresse du serveur (ex. http://192.168.1.20:11434/v1)")
        base = base.rstrip("/")
        server = models.Server(base_url=base if base.endswith("/v1") else base + "/v1", kind="manuel")
    else:
        server = found[choose("Serveur", [f"{s.kind} — {s.base_url} ({len(s.models)} modèle(s))" for s in found]) - 1]
    key = ask("Clé d'API du serveur (vide s'il n'en a pas)")
    names = server.models or asyncio.run(models.list_models(server.base_url, key or None))
    name = names[choose("Modèle", names) - 1] if len(names) > 1 else (names[0] if names else ask("Modèle"))
    provider = "lmstudio" if server.kind == "LM Studio" else "custom"
    return verify(hermes.Model(provider, name, base_url=server.base_url, api_key=key or None)), where == 3


def verify(model: hermes.Model) -> hermes.Model:
    say(f"Je teste {model.name}…")
    result = asyncio.run(models.check(model.base_url or "", model.name, model.api_key))
    if not result.ok:
        say(f"Le modèle ne répond pas ({result.error}). Vérifie l'adresse, la clé ou le nom, puis relance.")
        sys.exit(1)
    model.vision = bool(result.vision)
    say(f"OK en {result.seconds:.1f} s. {'Il lit les images : les photos lui seront envoyées telles quelles.' if model.vision else 'Il ne lit pas les images : Hermes les lui décrira.'}")
    return model


# -- the run -------------------------------------------------------------------------------------------

CATEGORIES = [("daily", "Quotidien"), ("wellness", "Bien-être"), ("finance", "Finances"), ("home", "Maison"),
              ("work", "Travail"), ("learning", "Apprendre"), ("creative", "Créatif"), ("tech", "Informatique")]


def custom_agent(name: str) -> dict:
    """« Autre » : the person writes the agent's role, tone and limits; they become its SOUL.md."""
    say(f"\nOn construit {name} sur mesure (Entrée pour passer une question).")
    role = ""
    while not role:
        role = ask(f"Que fait {name} pour toi ? (ex. « suit mes plantes et me dit quand les arroser »)")
    tone = ask("Son ton ? (ex. direct, chaleureux, drôle, formel)", "clair et bienveillant")
    rules = ask("Ce qu'il doit toujours faire ou ne jamais faire ?", "")
    language = ask("Dans quelle langue répond-il ?", "français")
    say("Quelle couleur de Bip ?")
    category = CATEGORIES[choose("Couleur", [label for _, label in CATEGORIES], default=8) - 1][0]
    soul = (f"Tu es {name}, un agent personnel de ton utilisateur. Ton rôle : {role.rstrip('.')}.\n"
            f"Ton ton : {tone}. Tu réponds en {language}, de façon concise.\n")
    if rules:
        soul += f"Règles à respecter : {rules.rstrip('.')}.\n"
    soul += "Tu demandes avant toute action irréversible (envoyer, acheter, supprimer).\n"
    return {"category": category, "label": "Autre", "description": role[:120], "soul": soul}
    """(the name the person gives, its template) for each agent to create. No name is imposed."""
    say("\nQuels genres d'agents créer pour commencer ? (numéros séparés par des virgules ; d'autres s'ajoutent depuis l'app)")
    for i, t in enumerate(TEMPLATES, 1):
        say(f"  {i}. {t['label']} — {t['description']}")
    picked = ask("Genres", "1")
    chosen = [TEMPLATES[int(x) - 1] for x in picked.replace(" ", "").split(",") if x.isdigit() and 1 <= int(x) <= len(TEMPLATES)]
    named = []
    for template in chosen or [TEMPLATES[0]]:
        name = ""
        while not name:
            name = ask(f"Comment s'appelle ton agent « {template['label']} » ?").strip()
        named.append((name, custom_agent(name) if template["category"] == "custom" else template))
    return named


def pocket_running() -> bool:
    try:
        return httpx.get("http://127.0.0.1:8098/health", timeout=2).status_code == 200
    except httpx.HTTPError:
        return False


def write(path: Path, content: str, runner: hermes.Runner, mode: int = 0o644) -> None:
    runner.log.append(f"write {path}")
    if runner.dry:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    path.chmod(mode)


def start_service(system: str, label: str, path: Path, runner: hermes.Runner) -> None:
    if system == "macos":
        runner.run(["launchctl", "bootout", f"gui/{os.getuid()}/{label}"], check=False)  # an older copy, if any
        runner.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(path)])
    else:
        runner.run(["systemctl", "--user", "daemon-reload"])
        runner.run(["systemctl", "--user", "enable", "--now", label])


def venv(folder: Path, runner: hermes.Runner) -> None:
    if (folder / ".venv" / "bin" / "python").exists():
        return
    runner.run(["uv", "venv", "--python", "3.12", str(folder / ".venv")])
    runner.run(["uv", "pip", "install", "--python", str(folder / ".venv" / "bin" / "python"), "-r",
                str(folder / "requirements.txt")])


def wait_for(url: str, headers: Optional[dict] = None, seconds: int = 90) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if httpx.get(url, headers=headers or {}, timeout=3, verify=False).status_code < 500:
                return True
        except httpx.HTTPError:
            pass
        time.sleep(2)
    return False


def main(argv: Optional[List[str]] = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    if argv and argv[0] == "uninstall":
        return uninstall_main(argv[1:])
    parser = argparse.ArgumentParser(prog="bipinstall", description="Install BipAgents for this account "
                                     "(`bipinstall uninstall` removes it)")
    parser.add_argument("--dry-run", action="store_true", help="show every step without changing anything")
    parser.add_argument("--relay-url", help="the administrator's bridge, to send notifications without an APNs key")
    parser.add_argument("--relay-key", help="this install's relay key (given by the administrator)")
    parser.add_argument("--ports", type=int, help="the first port of this install's block (default: the first free one)")
    parser.add_argument("--languages", default="fr,en", help="voices to load: fr, en, es, de")
    parser.add_argument("--no-tailscale", action="store_true",
                        help="this machine only, nothing published on a tailnet (tests, or a setup done by hand)")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)

    machine = detect()
    say(f"BipAgents — installation pour {os.environ.get('USER', 'ce compte')}")
    say(f"Machine : {machine.summary}")
    if args.no_tailscale:
        machine.tailnet_name = None
    elif not machine.tailnet_name:
        say("Tailscale n'est pas connecté : installe-le et connecte-toi (https://tailscale.com/download), puis relance.")
        if not args.dry_run:
            return 1
    model, model_here = pick_model(machine)
    plan = make_plan(machine, model_here)
    ports = pick_block(wanted=args.ports)
    say(f"\nProfil {plan.profile} : environ {plan.memory_gib(1):.1f} Go pour toi, {plan.people} personne(s) au total sur cette machine.")
    for note in plan.notes:
        say(f"  · {note}")
    say(f"Ports {ports.base}–{ports.base + 9} : agents {ports.hermes}, bridge {ports.bridge}, réseau local {ports.lan}.")
    chosen = pick_agents()
    if ask("\nOn y va ? (o/n)", "o").lower() not in ("o", "oui", "y", "yes"):
        return 1

    inst = Install(repo=REPO, home=Path.home(), ports=ports, tailnet_name=machine.tailnet_name,
                   lan_address=(machine.lan_addresses or [None])[0], model=model,
                   relay_url=args.relay_url, relay_key=args.relay_key)
    say("\n1/5 Hermes…")
    hermes.install(runner, browser=plan.browser != "none")
    hub_key = hermes.setup_hub(runner, ports.hermes, model)
    for display_name, template in chosen:
        profile = hermes.profile_name(display_name, (a.name for a in inst.agents))
        soul = template["soul"].replace("{name}", display_name)
        key = hermes.create_agent(runner, profile, description=template["description"], soul=soul, model=model)
        inst.agents.append(AgentEntry(profile, display_name, key))
    hermes.install_service(runner)

    say("2/5 Voix des Bips…")
    if pocket_running():
        say("  déjà en place sur cette machine : je la partage.")
    else:
        venv(REPO / "pocket", runner)
        label, path, content = pocket_service(inst, machine.system, args.languages)
        write(path, content, runner)
        start_service(machine.system, label, path, runner)

    if machine.system == "macos":
        say("  macOS peut demander si Python peut accéder aux appareils du réseau local : accepte, sinon les agents\n"
            "  et le bridge ne joindront pas un modèle installé sur une autre machine de la maison.")
    say("3/5 Bridge…")
    venv(REPO / "bridge", runner)
    write(inst.bridge_config, bridge_toml(inst), runner, mode=0o600)
    label, path, content = bridge_service(inst, machine.system)
    write(path, content, runner)
    start_service(machine.system, label, path, runner)

    say("4/5 Tailscale…")
    commands = [] if args.no_tailscale else None
    if commands is None:
        commands = tailscale_commands(machine.tailscale or "tailscale", ports, desk=plan.browser == "permanent")
    published = True
    for command in commands:
        try:
            runner.run(command, shell=True)
        except RuntimeError:
            published = False
    if not published:
        say("  Publication refusée : demande à l'administrateur de la machine de lancer :")
        for command in commands:
            say(f"    sudo {command}")

    say("5/5 Vérifications…")
    if not runner.dry:
        hub = f"http://127.0.0.1:{ports.hermes}"
        for agent in inst.agents:
            if wait_for(f"{hub}/p/{agent.name}/v1/capabilities", {"Authorization": f"Bearer {agent.key}"}):
                httpx.post(f"{hub}/p/{agent.name}/api/sessions", json={"title": "Bot Chat"},
                           headers={"Authorization": f"Bearer {agent.key}"}, timeout=10)
        wait_for(f"http://127.0.0.1:{ports.bridge}/health")
    if args.dry_run:
        say("\nÀ blanc : voici ce qui aurait été fait.")
        for line in runner.log:
            say(f"  $ {re.sub(r'(_KEY )[^ ]+', lambda m: m.group(1) + '<clé>', line)}")
        say("\n--- bridge.toml ---")
        say(re.sub(r'(_key = )"[^"]*"', r'\1"<clé>"', bridge_toml(inst)))
        return 0
    if args.no_tailscale:
        say("\nC'est prêt, sur cette machine seulement (sans Tailscale, l'app ne peut pas encore la joindre).")
        return 0
    say("\nC'est prêt. Scanne ce code dans l'app BipAgents (Ajouter un agent › Scanner le QR code) :")
    subprocess.run([str(REPO / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "qr", "--config",
                    str(inst.bridge_config)], cwd=REPO / "bridge", check=False)
    return 0


def uninstall_main(argv: List[str]) -> int:
    from .uninstall import uninstall

    parser = argparse.ArgumentParser(prog="bipinstall uninstall", description="Remove BipAgents from this account")
    parser.add_argument("--all", action="store_true", help="also remove Hermes and the agents' data (conversations, memory)")
    parser.add_argument("--dry-run", action="store_true", help="show what would be removed")
    args = parser.parse_args(argv)
    if args.all and not args.dry_run:
        say("Ceci supprime aussi Hermes et toutes les données des agents : conversations, mémoire, fichiers.")
        if ask("Tape « supprimer » pour confirmer") != "supprimer":
            say("Rien n'a été supprimé.")
            return 1
    runner = hermes.Runner(dry=args.dry_run)
    uninstall(runner, everything=args.all)
    for line in runner.log:
        say(f"  {'$ ' if not line.startswith('remove') else ''}{line}")
    say("BipAgents est retiré de ce compte." + ("" if args.all else " Les données des agents sont gardées dans ~/.hermes "
                                                 "(`uninstall --all` pour les supprimer aussi)."))
    return 0


if __name__ == "__main__":
    sys.exit(main())
