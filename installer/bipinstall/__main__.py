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

from . import hermes, models, ui
from .detect import Machine, detect
from .plan import make_plan
from .ports import pick_block
from .stack import AgentEntry, Install, bridge_service, bridge_toml, hermes_service, pocket_service, tailscale_commands

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "bridge"))
from bipbridge import souls  # noqa: E402  (kinds of agents, shared with the bridge)

TEMPLATES = souls.templates()


def say(text: str = "") -> None:
    print(text, flush=True)


def ask(question: str, default: str = "") -> str:
    shown = f" [{default}]" if default else ""
    try:
        answer = input(f"{ui.question(question)}{shown} : ").strip()
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
    first = "Sur une autre machine de mon tailnet (PC avec GPU, Spark, Mac à la maison…)" if machine.cloud \
        else "Sur une autre machine de mon réseau (PC avec GPU, Spark, Mac…)"
    where = choose("Choix", [first, "Dans le cloud, par API (OpenRouter, OpenAI, Anthropic, Mistral)",
                             "Sur cette machine (Ollama ou LM Studio déjà installé)"], default=2 if machine.cloud else 1)
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
    if where == 1 and machine.cloud:
        found = []   # no home network around a rented server: the address on the tailnet is asked below
    else:
        say("Je cherche les serveurs de modèles sur ton réseau…")
        found = asyncio.run(models.discover(machine.lan_addresses if where == 1 else []))
    found = [s for s in found if (s.base_url.startswith("http://127.0.0.1")) == (where == 3)]
    if not found:
        if not (where == 1 and machine.cloud):
            say("Aucun trouvé automatiquement.")
        example = "http://pc-maison.tail1234.ts.net:11434/v1" if machine.cloud else "http://192.168.1.20:11434/v1"
        base = ""
        while not base.startswith(("http://", "https://")):
            base = ask(f"Adresse du serveur (ex. {example})")
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
        say(ui.sad(f"Le modèle ne répond pas ({result.error})."))
        say("Vérifie l'adresse, la clé ou le nom, puis relance.")
        sys.exit(1)
    model.vision = bool(result.vision)
    say(ui.done(f"Il répond en {result.seconds:.1f} s.") + f" {'Il lit les images : les photos lui seront envoyées telles quelles.' if model.vision else 'Il ne lit pas les images : Hermes les lui décrira.'}")
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
    return {"category": category, "label": "Autre", "description": role[:120],
            "soul": souls.custom_soul(name, role, tone, rules, language)}


def pick_agents() -> List[tuple]:
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
    if argv and argv[0] == "hermes-update":
        from .update import main as update_main
        return update_main(argv[1:])
    if argv and argv[0] == "famille":
        from .family import main as family_main
        return family_main(argv[1:])
    parser = argparse.ArgumentParser(prog="bipinstall", description="Install BipAgents for this account "
                                     "(`bipinstall uninstall` removes it; `bipinstall famille ajouter <nom>` prepares a "
                                     "family member's install on this server; `bipinstall hermes-update` updates Hermes "
                                     "and checks the agents)")
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
    say(ui.banner(os.environ.get("USER", ""), machine.summary))
    if machine.system == "linux" and not args.dry_run:
        from .account import ensure_user_services
        if not ensure_user_services(say):
            return 1
    if machine.cloud:
        say(ui.note(f"Serveur loué chez {machine.cloud} : pas de porte sur un réseau local (elle serait sur Internet),\n"
                    "    tout passe par Tailscale. Ton hébergeur a accès au disque : chiffre ce qui compte, et garde le "
                    "système à jour."))
        machine.lan_addresses = []
    if args.no_tailscale:
        machine.tailnet_name = None
    elif not machine.tailnet_name:
        say(ui.sad("Tailscale n'est pas connecté."))
        say("Installe-le et connecte-toi (https://tailscale.com/download), puis relance.")
        if not args.dry_run:
            return 1
    from .existing import detect_existing
    existing = detect_existing()
    if existing:
        return install_on_existing(args, runner, machine, existing)
    model, model_here = pick_model(machine)
    plan = make_plan(machine, model_here)
    ports = pick_block(wanted=args.ports)
    say(f"\n{ui.BOLD}Profil {plan.profile}{ui.RESET} : environ {plan.memory_gib(1):.1f} Go pour toi, "
        f"{plan.people} personne(s) au total sur cette machine.")
    for line in plan.notes:
        say(ui.note(line))
    sandbox = hermes.sandbox_runtime()
    if sandbox:
        say(ui.note(f"Bac à sable : les outils des agents tournent dans un conteneur ({Path(sandbox).name})."))
    elif machine.system == "linux":
        say("  · Pas de bac à sable : tes agents n'auront ni terminal ni accès aux fichiers par défaut, et pas de coffre.\n"
            "    Pour les deux, fais installer podman une fois (sudo apt install podman, ou dnf), puis relance.")
    else:
        say("  · Sur Mac, pas encore de bac à sable : tes agents n'auront ni terminal ni accès aux fichiers par défaut,\n"
            "    et pas de coffre de mots de passe.")
    say(ui.note(f"Ports {ports.base}–{ports.base + 9} : agents {ports.hermes}, bridge {ports.bridge}, réseau local {ports.lan}."))
    browser = False
    if sandbox and plan.browser != "none":
        say("\nChaque agent peut avoir son navigateur visible : un vrai Chrome sur un bureau à lui, que tu regardes en\n"
            "direct et reprends en main depuis l'app (Bureau). Il se connecte aux sites avec le coffre, sans voir tes\n"
            "mots de passe. Environ 0,5 Go de mémoire par agent quand il navigue.")
        browser = ask("Donner un navigateur visible à tes agents ? (o/n)", "o").lower() in ("o", "oui", "y", "yes")
    chosen = pick_agents()
    if ask("\nOn y va ? (o/n)", "o").lower() not in ("o", "oui", "y", "yes"):
        return 1

    inst = Install(repo=REPO, home=Path.home(), ports=ports, tailnet_name=machine.tailnet_name,
                   lan_address=(machine.lan_addresses or [None])[0], model=model,
                   relay_url=args.relay_url, relay_key=args.relay_key, isolated=bool(sandbox), browser=browser)
    steps = iter(range(1, 7 if browser else 6))
    total = 6 if browser else 5
    say(ui.step(next(steps), total, "Hermes, le cerveau des agents"))
    hermes.install(runner, browser=plan.browser != "none")
    skipped = hermes.apply_patches(runner, REPO / "hermes" / "patches")
    if skipped:
        say(f"  Patches non appliqués à cette version d'Hermes : {', '.join(skipped)} (l'app marche, sans ces fonctions).")
    hub_key = hermes.setup_hub(runner, ports.hermes, model, sandbox=bool(sandbox))
    if plan.browser != "none" and not browser:   # the visible browser brings its own desktop
        if hermes.desktop_ready() or sandbox:
            hermes.setup_desktop(runner, plan.browser)
        elif machine.system == "linux":
            say("  Bureau des agents : il manque TigerVNC et Xfce. Pour que l'app montre leur navigateur en direct,\n"
                "  lance une fois (avec sudo) : hermes computer-use screen install, puis relance cet installeur.")
    for display_name, template in chosen:
        profile = souls.profile_name(display_name, (a.name for a in inst.agents))
        soul = template["soul"].replace("{name}", display_name)
        key = hermes.create_agent(runner, profile, description=template["description"], soul=soul, model=model)
        inst.agents.append(AgentEntry(profile, display_name, key))
    gateway_service(inst, machine, runner)

    say(ui.done("Hermes et tes agents sont en place."))
    return finish(inst, machine, runner, args, steps, total, sandbox, desk=plan.browser == "permanent")


def gateway_service(inst: Install, machine: Machine, runner: hermes.Runner) -> None:
    try:
        hermes.install_service(runner)
    except RuntimeError:
        # Hermes declines its own service in some setups (inside a container): ours runs the same command.
        say("  Service Hermes : j'installe le mien.")
        label, path, content = hermes_service(inst, machine.system, hermes.hermes_bin(), hermes.hermes_home())
        write(path, content, runner)
        start_service(machine.system, label, path, runner)


def yes(answer: str) -> bool:
    return answer.lower() in ("o", "oui", "y", "yes")


def install_on_existing(args, runner: hermes.Runner, machine: Machine, existing) -> int:
    """BipAgents on the person's own Hermes: their model, tools, platforms, agents and conversations stay as they are.
    Only the local API and each shown agent's key are added (existing.py), and written down for `uninstall`."""
    from .existing import RECORD, apply_changes, key_of, plan_changes, profile_label, same_version

    say(f"\nTu as déjà Hermes ({existing.version}) : je m'y branche sans toucher à tes réglages, ton modèle "
        f"({existing.model or 'non précisé'}), tes outils ni tes agents.")
    if not existing.multiplex:
        say(ui.sad("Cette version d'Hermes ne sert pas plusieurs agents à la même adresse."))
        say("Mets-la à jour (hermes update), puis relance cet installeur.")
        return 1
    if (Path.home() / ".config" / "bipagents" / "bridge.toml").exists():
        say(ui.sad("BipAgents est déjà installé sur ce compte."))
        say("Pour le refaire : bash ~/BipAgents/install.sh uninstall, puis relance.")
        return 1
    ports = pick_block(wanted=args.ports)
    hub_port = existing.api_port if existing.api_enabled and existing.api_port else ports.hermes

    say("\nQuels agents veux-tu voir dans l'app ? (numéros séparés par des virgules)")
    for i, profile in enumerate(existing.profiles, 1):
        about = f" — {profile.description}" if profile.description else ""
        main_one = " (ton profil principal)" if profile.name == hermes.HUB_PROFILE else ""
        say(f"  {i}. {profile_label(profile.name)}{main_one}{about}")
    every = ",".join(str(i) for i in range(1, len(existing.profiles) + 1))
    picked = ask("Agents", every)
    numbers = [int(x) for x in picked.replace(" ", "").split(",") if x.isdigit() and 1 <= int(x) <= len(existing.profiles)]
    shown = [existing.profiles[n - 1] for n in dict.fromkeys(numbers)] or existing.profiles
    names = {p.name: (ask(f"Son nom dans l'app ({p.name})", profile_label(p.name)) or profile_label(p.name)) for p in shown}

    sandbox = hermes.sandbox_runtime()
    if existing.protected:
        say(ui.note("Les outils de tous tes agents tournent dans un bac à sable : le coffre de mots de passe est disponible."))
    else:
        exposed = ", ".join(profile_label(p.name) for p in existing.profiles if not p.sandboxed)
        say(ui.note(f"{exposed} : terminal sur cette machine, donc accès à tes fichiers et aux clés du bridge. Je ne change "
                    "pas leurs outils,\n    mais le bridge ne gardera pas de mots de passe (pas de coffre)."))
    browser = False
    if existing.protected and sandbox and machine.system == "linux":
        say("\nChaque agent peut avoir son navigateur visible : un vrai Chrome sur un bureau à lui, que tu regardes en\n"
            "direct et reprends en main depuis l'app (Bureau). Environ 0,5 Go de mémoire par agent quand il navigue.")
        browser = yes(ask("Donner un navigateur visible à tes agents ? (o/n)", "o"))
    patch = False
    if same_version(existing, hermes.HERMES_COMMIT):
        patch = yes(ask("Appliquer les correctifs BipAgents à ton Hermes (questions de l'agent dans l'app, photos gardées,\n"
                        "  coffre après un redémarrage du navigateur) ? `hermes update` les retire. (o/n)", "o"))
    else:
        say(ui.note(f"Ton Hermes ({existing.commit or 'version inconnue'}) n'est pas la version testée "
                    f"({hermes.HERMES_COMMIT}) : pas de correctifs BipAgents, l'app marche sans ces fonctions."))

    changes = plan_changes(existing, [p.name for p in shown], hub_port)
    say("\nCe que je change dans ton Hermes :")
    for profile, key, value, _ in changes:
        say(ui.note(f"{profile_label(profile)} : {key} = {'<nouvelle clé>' if key.endswith('KEY') else value}"))
    if not changes:
        say(ui.note("rien (ton API locale et les clés de tes agents sont déjà là)"))
    if patch:
        say(ui.note("les correctifs BipAgents, dans le code d'Hermes"))
    restart = bool(changes or patch)
    if restart:
        say(ui.note({"service": "ta gateway sera redémarrée une fois",
                     "manual": "ta gateway tourne à la main : tu la redémarreras à la fin",
                     "stopped": "ta gateway ne tourne pas : je l'installe en service, qui démarre seul"}[existing.gateway]))
    say(ui.note("Tout ça s'annule avec la désinstallation de BipAgents."))
    if not yes(ask("\nOn y va ? (o/n)", "o")):
        return 1

    inst = Install(repo=REPO, home=Path.home(), ports=ports, tailnet_name=machine.tailnet_name,
                   lan_address=(machine.lan_addresses or [None])[0], relay_url=args.relay_url, relay_key=args.relay_key,
                   isolated=existing.protected, browser=browser, hermes_port=hub_port)
    total = 6 if browser else 5
    steps = iter(range(1, total + 1))
    say(ui.step(next(steps), total, "Ton Hermes"))
    patched: List[str] = []
    if patch:
        skipped = hermes.apply_patches(runner, REPO / "hermes" / "patches", applied=patched)
        if skipped:
            say(ui.note(f"Correctifs non appliqués : {', '.join(skipped)} (l'app marche, sans ces fonctions)."))
    apply_changes(runner, changes, inst.config_dir / RECORD, existing.gateway, patched)
    for profile in shown:
        inst.agents.append(AgentEntry(profile.name, names[profile.name], key_of(existing, changes, profile.name) or "",
                                      root=profile.name == hermes.HUB_PROFILE))
    if restart and existing.gateway == "service":
        runner.run([hermes.hermes_bin(), "gateway", "restart"])
    elif restart and existing.gateway == "stopped":
        gateway_service(inst, machine, runner)
    elif restart:
        say(ui.note("Redémarre ta gateway maintenant (arrête-la, puis hermes gateway run) : elle prendra ces réglages."))
        if not runner.dry:
            ask("Entrée quand c'est fait")
    say(ui.done("Ton Hermes est prêt pour l'app."))
    return finish(inst, machine, runner, args, steps, total, sandbox, bot_chat=False)


def finish(inst: Install, machine: Machine, runner: hermes.Runner, args, steps, total: int, sandbox: Optional[str],
           desk: bool = False, bot_chat: bool = True) -> int:
    """What every install ends with: the voices, the bridge, the visible browser when chosen, Tailscale, the checks
    and the QR code."""
    say(ui.step(next(steps), total, "Les voix des Bips"))
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
    say(ui.step(next(steps), total, "Le bridge, qui relie l'app à tes agents"))
    venv(REPO / "bridge", runner)
    write(inst.bridge_config, bridge_toml(inst), runner, mode=0o600)
    label, path, content = bridge_service(inst, machine.system)
    write(path, content, runner)
    start_service(machine.system, label, path, runner)

    say(ui.done("Bridge démarré."))
    if inst.browser:
        say(ui.step(next(steps), total, "Le navigateur visible de tes agents"))
        say(ui.note("Je prépare son image : Chrome, son bureau et le serveur qui le pilote (stealth-browser-mcp, récupéré "
                    "sur son dépôt).\n    Compte quelques minutes la première fois."))
        try:
            hermes.build_browser(runner, sandbox, REPO)
            runner.run([str(REPO / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "browser", "add", "--all",
                        "--config", str(inst.bridge_config)])
            if machine.system == "linux":
                runner.run(["systemctl", "--user", "restart", label])   # the bridge reads the agents' desktops
            say(ui.done("Chaque agent a son navigateur, sur son propre bureau."))
        except RuntimeError as exc:
            say(ui.fail("Le navigateur n'a pas pu être installé : tes agents fonctionnent sans."))
            say(ui.note(str(exc)[-300:]))
            say(ui.note("Pour réessayer plus tard :"))
            say(f"    {sandbox} build -t {hermes.BROWSER_IMAGE} {REPO / 'browser'}")
            say(f"    {REPO / 'bridge' / '.venv' / 'bin' / 'python'} -m bipbridge browser add --all --config {inst.bridge_config}")
            say(f"    systemctl --user restart {label}")
    say(ui.step(next(steps), total, "Tailscale, pour joindre tes agents de partout"))
    commands = [] if args.no_tailscale else None
    if commands is None:
        commands = tailscale_commands(machine.tailscale or "tailscale", inst.ports, desk=desk, hermes_port=inst.hermes_port)
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

    say(ui.step(next(steps), total, "Vérifications"))
    if not runner.dry:
        hub = f"http://127.0.0.1:{inst.hub_port}"
        for agent in inst.agents:
            url = hub if agent.root else f"{hub}/p/{agent.name}"
            answers = wait_for(f"{url}/v1/capabilities", {"Authorization": f"Bearer {agent.key}"})
            say((ui.done if answers else ui.fail)(f"{agent.display_name} {'répond' if answers else 'ne répond pas encore'}."))
            if answers and bot_chat:
                httpx.post(f"{url}/api/sessions", json={"title": "Bot Chat"},
                           headers={"Authorization": f"Bearer {agent.key}"}, timeout=10)
        wait_for(f"http://127.0.0.1:{inst.ports.bridge}/health")
    if args.dry_run:
        say("\nÀ blanc : voici ce qui aurait été fait.")
        for line in runner.log:
            say(f"  $ {re.sub(r'(_KEY )[^ ]+', lambda m: m.group(1) + '<clé>', line)}")
        say("\n--- bridge.toml ---")
        say(re.sub(r'(_key = )"[^"]*"', r'\1"<clé>"', bridge_toml(inst)))
        return 0
    if args.no_tailscale:
        say(ui.finale("C'est prêt, sur cette machine seulement."))
        say("Sans Tailscale, l'app ne peut pas encore la joindre.")
        return 0
    say(ui.finale("C'est prêt ! Scanne ce code dans l'app."))
    say("Dans l'app BipAgents : « + » › Scanner le QR code.")
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
