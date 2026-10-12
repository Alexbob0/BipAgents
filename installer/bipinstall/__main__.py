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
from .ui import tr
from .detect import Machine, detect
from .plan import make_plan
from .ports import pick_block
from .stack import AgentEntry, Install, bridge_service, bridge_toml, hermes_service, pocket_service, tailscale_commands

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "bridge"))
from bipbridge import souls  # noqa: E402  (kinds of agents, shared with the bridge)


def templates() -> List[dict]:
    """The kinds of agents, in the installer's language (their SOUL.md too)."""
    return souls.templates(ui.LANG)


def say(text: str = "") -> None:
    print(text, flush=True)


def ask(question: str, default: str = "") -> str:
    shown = f" [{default}]" if default else ""
    try:
        after = " : " if ui.LANG == "fr" else (" " if question.endswith("?") and not shown else ": ")
        answer = input(f"{ui.question(question)}{shown}{after}").strip()
    except EOFError:  # no one to answer (answers given in advance ran out): stop rather than ask forever
        say(tr("\nPas de réponse : installation interrompue, rien d'autre n'a été modifié.",
               "\nNo answer: install stopped, nothing else was changed."))
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

PROFILES_EN = {"léger": "Light", "local": "Local", "costaud": "Heavy-duty"}   # the plan's profiles, as said in English


def pick_model(machine: Machine) -> tuple[hermes.Model, bool]:
    say(tr("\nOù tourne le modèle (le « cerveau » des agents) ?", "\nWhere does the model (the agents' \"brain\") run?"))
    first = tr("Sur une autre machine de mon tailnet (PC avec GPU, Spark, Mac à la maison…)",
               "On another machine on my tailnet (PC with a GPU, Spark, Mac at home…)") if machine.cloud \
        else tr("Sur une autre machine de mon réseau (PC avec GPU, Spark, Mac…)",
                "On another machine on my network (PC with a GPU, Spark, Mac…)")
    where = choose(tr("Choix", "Choice"), [first, tr("Dans le cloud, par API (OpenRouter, OpenAI, Anthropic, Mistral)",
                                                     "In the cloud, through an API (OpenRouter, OpenAI, Anthropic, Mistral)"),
                                           tr("Sur cette machine (Ollama ou LM Studio déjà installé)",
                                              "On this machine (Ollama or LM Studio already installed)")],
                   default=2 if machine.cloud else 1)
    if where == 2:
        provider = models.CLOUD[choose(tr("Fournisseur", "Provider"), [p.label for p in models.CLOUD]) - 1]
        say(tr(f"Ta clé se crée ici : {provider.key_hint}", f"Create your key here: {provider.key_hint}"))
        key = ask(tr("Clé d'API", "API key"))
        name = ask(tr("Modèle (identifiant exact)", "Model (exact ID)"), {"openrouter": "anthropic/claude-sonnet-5.5", "openai": "gpt-5.1",
                                                   "anthropic": "claude-sonnet-5-5", "mistral": "mistral-large-latest"}
                   .get(provider.key, ""))
        hermes_provider = {"openrouter": "openrouter", "openai": "openai-api", "anthropic": "anthropic"}.get(provider.key, "custom")
        model = hermes.Model(hermes_provider, name, base_url=provider.base_url, api_key=key)
        return verify(model), False
    if where == 1 and machine.cloud:
        found = []   # no home network around a rented server: the address on the tailnet is asked below
    else:
        say(tr("Je cherche les serveurs de modèles sur ton réseau…", "Looking for model servers on your network…"))
        found = asyncio.run(models.discover(machine.lan_addresses if where == 1 else []))
    found = [s for s in found if (s.base_url.startswith("http://127.0.0.1")) == (where == 3)]
    if not found:
        if not (where == 1 and machine.cloud):
            say(tr("Aucun trouvé automatiquement.", "None found automatically."))
        example = "http://pc-maison.tail1234.ts.net:11434/v1" if machine.cloud else "http://192.168.1.20:11434/v1"
        base = ""
        while not base.startswith(("http://", "https://")):
            base = ask(tr(f"Adresse du serveur (ex. {example})", f"Server address (e.g. {example})"))
        base = base.rstrip("/")
        server = models.Server(base_url=base if base.endswith("/v1") else base + "/v1", kind="manuel")
    else:
        server = found[choose(tr("Serveur", "Server"), [tr(f"{s.kind} — {s.base_url} ({len(s.models)} modèle(s))",
                                                            f"{s.kind} — {s.base_url} ({len(s.models)} model(s))")
                                                         for s in found]) - 1]
    key = ask(tr("Clé d'API du serveur (vide s'il n'en a pas)", "The server's API key (empty if it has none)"))
    names = server.models or asyncio.run(models.list_models(server.base_url, key or None))
    name = names[choose(tr("Modèle", "Model"), names) - 1] if len(names) > 1 \
        else (names[0] if names else ask(tr("Modèle", "Model")))
    provider = "lmstudio" if server.kind == "LM Studio" else "custom"
    return verify(hermes.Model(provider, name, base_url=server.base_url, api_key=key or None)), where == 3


def verify(model: hermes.Model) -> hermes.Model:
    say(tr(f"Je teste {model.name}…", f"Testing {model.name}…"))
    result = asyncio.run(models.check(model.base_url or "", model.name, model.api_key))
    if not result.ok:
        say(ui.sad(tr(f"Le modèle ne répond pas ({result.error}).", f"The model isn't answering ({result.error}).")))
        say(tr("Vérifie l'adresse, la clé ou le nom, puis relance.", "Check the address, the key or the name, then run this again."))
        sys.exit(1)
    model.vision = bool(result.vision)
    sees = tr("Il lit les images : les photos lui seront envoyées telles quelles.",
              "It reads images: photos will be sent to it as they are.") if model.vision \
        else tr("Il ne lit pas les images : Hermes les lui décrira.", "It doesn't read images: Hermes will describe them to it.")
    say(ui.done(tr(f"Il répond en {result.seconds:.1f} s.", f"It answers in {result.seconds:.1f} s.")) + f" {sees}")
    return model


# -- the run -------------------------------------------------------------------------------------------

def colours() -> List[tuple]:
    """(category, label) of each Bip colour an « Autre » agent can take: the kinds' own, in the installer's language."""
    return [(t["category"], t["label"]) for t in templates() if t["category"] != "custom"]


def custom_agent(name: str) -> dict:
    """« Autre » : the person writes the agent's role, tone and limits; they become its SOUL.md."""
    say(tr(f"\nOn construit {name} sur mesure (Entrée pour passer une question).",
           f"\nLet's build {name} just for you (press Enter to skip a question)."))
    role = ""
    while not role:
        role = ask(tr(f"Que fait {name} pour toi ? (ex. « suit mes plantes et me dit quand les arroser »)",
                      f"What does {name} do for you? (e.g. \"keeps an eye on my plants and tells me when to water them\")"))
    tone = ask(tr("Son ton ? (ex. direct, chaleureux, drôle, formel)", "Its tone? (e.g. direct, warm, funny, formal)"),
               tr("clair et bienveillant", "clear and kind"))
    rules = ask(tr("Ce qu'il doit toujours faire ou ne jamais faire ?", "Anything it must always or never do?"), "")
    language = ask(tr("Dans quelle langue répond-il ?", "Which language does it answer in?"), tr("français", "English"))
    say(tr("Quelle couleur de Bip ?", "Which Bip color?"))
    options = colours()
    category = options[choose(tr("Couleur", "Color"), [label for _, label in options], default=len(options)) - 1][0]
    return {"category": category, "label": tr("Autre", "Other"), "description": role[:120],
            "soul": souls.custom_soul(name, role, tone, rules, language, lang=ui.LANG)}


def pick_agents() -> List[tuple]:
    """(the name the person gives, its template) for each agent to create. No name is imposed."""
    say(tr("\nQuels genres d'agents créer pour commencer ? (numéros séparés par des virgules ; d'autres s'ajoutent depuis l'app)",
           "\nWhich kinds of agents should we create first? (numbers separated by commas; you can add more from the app)"))
    kinds = templates()
    for i, t in enumerate(kinds, 1):
        say(f"  {i}. {t['label']} — {t['description']}")
    picked = ask(tr("Genres", "Kinds"), "1")
    chosen = [kinds[int(x) - 1] for x in picked.replace(" ", "").split(",") if x.isdigit() and 1 <= int(x) <= len(kinds)]
    named = []
    for template in chosen or [kinds[0]]:
        name = ""
        while not name:
            name = ask(tr(f"Comment s'appelle ton agent « {template['label']} » ?",
                          f"What's the name of your \"{template['label']}\" agent?")).strip()
        named.append((name, custom_agent(name) if template["category"] == "custom" else template))
    return named


def pocket_running() -> bool:
    return voice_engine_answers("http://127.0.0.1:8098")


KYUTAI_URL = "http://127.0.0.1:8097"


def voice_engine_answers(base_url: str) -> bool:
    """Whether a voice engine (Pocket, Kyutai: same `/health`) already runs there."""
    try:
        return httpx.get(f"{base_url.rstrip('/')}/health", timeout=2).status_code == 200
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
    argv = take_language(list(argv))
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
    parser.add_argument("--kyutai-url", help="a Kyutai voice server on another machine (default: the one running on "
                                             "this machine, if any)")
    parser.add_argument("--no-tailscale", action="store_true",
                        help="this machine only, nothing published on a tailnet (tests, or a setup done by hand)")
    parser.add_argument("--lang", choices=["fr", "en"], help="the installer's language (default: the system's)")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)

    machine = detect()
    say(ui.banner(os.environ.get("USER", ""), machine.summary))
    if machine.system == "linux" and not args.dry_run:
        from .account import ensure_user_services
        if not ensure_user_services(say):
            return 1
    if machine.cloud:
        say(ui.note(tr(f"Serveur loué chez {machine.cloud} : pas de porte sur un réseau local (elle serait sur Internet),\n"
                       "    tout passe par Tailscale. Ton hébergeur a accès au disque : chiffre ce qui compte, et garde le "
                       "système à jour.",
                       f"Server rented from {machine.cloud}: no door on a local network (it would be on the Internet),\n"
                       "    everything goes through Tailscale. Your host can read the disk: encrypt what matters, and keep "
                       "the system up to date.")))
        machine.lan_addresses = []
    if args.no_tailscale:
        machine.tailnet_name = None
    elif not machine.tailnet_name:
        say(ui.sad(tr("Tailscale n'est pas connecté.", "Tailscale isn't connected.")))
        say(tr("Installe-le et connecte-toi (https://tailscale.com/download), puis relance.",
               "Install it and sign in (https://tailscale.com/download), then run this again."))
        if not args.dry_run:
            return 1
    from .existing import detect_existing
    existing = detect_existing()
    if existing:
        return install_on_existing(args, runner, machine, existing)
    model, model_here = pick_model(machine)
    plan = make_plan(machine, model_here)
    ports = pick_block(wanted=args.ports)
    say(tr(f"\n{ui.BOLD}Profil {plan.profile}{ui.RESET} : environ {plan.memory_gib(1):.1f} Go pour toi, "
           f"{plan.people} personne(s) au total sur cette machine.",
           f"\n{ui.BOLD}{PROFILES_EN.get(plan.profile, plan.profile)} setup{ui.RESET}: about {plan.memory_gib(1):.1f} GB for you, "
           f"{plan.people} person(s) in all on this machine."))
    for line in plan.notes:
        say(ui.note(line))
    sandbox = hermes.sandbox_runtime()
    if sandbox:
        say(ui.note(tr(f"Bac à sable : les outils des agents tournent dans un conteneur ({Path(sandbox).name}).",
                       f"Sandbox: the agents' tools run in a container ({Path(sandbox).name}).")))
    elif machine.system == "linux":
        say(tr("  · Pas de bac à sable : tes agents n'auront ni terminal ni accès aux fichiers par défaut, et pas de coffre.\n"
               "    Pour les deux, fais installer podman une fois (sudo apt install podman, ou dnf), puis relance.",
               "  · No sandbox: your agents won't have a terminal or file access by default, and no vault.\n"
               "    For both, have podman installed once (sudo apt install podman, or dnf), then run this again."))
    else:
        say(tr("  · Sur Mac, pas encore de bac à sable : tes agents n'auront ni terminal ni accès aux fichiers par défaut,\n"
               "    et pas de coffre de mots de passe.",
               "  · No sandbox on a Mac yet: your agents won't have a terminal or file access by default,\n"
               "    and no password vault."))
    say(ui.note(tr(f"Ports {ports.base}–{ports.base + 9} : agents {ports.hermes}, bridge {ports.bridge}, réseau local {ports.lan}.",
                   f"Ports {ports.base}–{ports.base + 9}: agents {ports.hermes}, bridge {ports.bridge}, local network {ports.lan}.")))
    browser = False
    if sandbox and plan.browser != "none":
        say(tr("\nChaque agent peut avoir son navigateur visible : un vrai Chrome sur un bureau à lui, que tu regardes en\n"
               "direct et reprends en main depuis l'app (Bureau). Il se connecte aux sites avec le coffre, sans voir tes\n"
               "mots de passe. Environ 0,5 Go de mémoire par agent quand il navigue.",
               "\nEach agent can have its own visible browser: a real Chrome on a desktop of its own, that you watch live\n"
               "and take over from the app (Desktop). It signs in to websites with the vault, without seeing your\n"
               "passwords. About 0.5 GB of memory per agent while it browses."))
        browser = yes(ask(tr("Donner un navigateur visible à tes agents ? (o/n)", "Give your agents a visible browser? (y/n)"),
                          tr("o", "y")))
    chosen = pick_agents()
    if not yes(ask(tr("\nOn y va ? (o/n)", "\nReady to go? (y/n)"), tr("o", "y"))):
        return 1

    inst = Install(repo=REPO, home=Path.home(), ports=ports, tailnet_name=machine.tailnet_name,
                   lan_address=(machine.lan_addresses or [None])[0], model=model,
                   relay_url=args.relay_url, relay_key=args.relay_key, isolated=bool(sandbox), browser=browser)
    steps = iter(range(1, 7 if browser else 6))
    total = 6 if browser else 5
    say(ui.step(next(steps), total, tr("Hermes, le cerveau des agents", "Hermes, the agents' brain")))
    hermes.install(runner, browser=plan.browser != "none")
    skipped = hermes.apply_patches(runner, REPO / "hermes" / "patches")
    if skipped:
        say(tr(f"  Patches non appliqués à cette version d'Hermes : {', '.join(skipped)} (l'app marche, sans ces fonctions).",
               f"  Patches not applied to this Hermes version: {', '.join(skipped)} (the app works, without these features)."))
    hub_key = hermes.setup_hub(runner, ports.hermes, model, sandbox=bool(sandbox))
    if plan.browser != "none" and not browser:   # the visible browser brings its own desktop
        if hermes.desktop_ready() or sandbox:
            hermes.setup_desktop(runner, plan.browser)
        elif machine.system == "linux":
            say(tr("  Bureau des agents : il manque TigerVNC et Xfce. Pour que l'app montre leur navigateur en direct,\n"
                   "  lance une fois (avec sudo) : hermes computer-use screen install, puis relance cet installeur.",
                   "  Agents' desktop: TigerVNC and Xfce are missing. For the app to show their browser live,\n"
                   "  run once (with sudo): hermes computer-use screen install, then run this installer again."))
    for display_name, template in chosen:
        profile = souls.profile_name(display_name, (a.name for a in inst.agents))
        soul = template["soul"].replace("{name}", display_name)
        key = hermes.create_agent(runner, profile, description=template["description"], soul=soul, model=model)
        inst.agents.append(AgentEntry(profile, display_name, key))
    gateway_service(inst, machine, runner)

    say(ui.done(tr("Hermes et tes agents sont en place.", "Hermes and your agents are set up.")))
    return finish(inst, machine, runner, args, steps, total, sandbox, desk=plan.browser == "permanent")


def gateway_service(inst: Install, machine: Machine, runner: hermes.Runner) -> None:
    try:
        hermes.install_service(runner)
    except RuntimeError:
        # Hermes declines its own service in some setups (inside a container): ours runs the same command.
        say(tr("  Service Hermes : j'installe le mien.", "  Hermes service: installing my own."))
        label, path, content = hermes_service(inst, machine.system, hermes.hermes_bin(), hermes.hermes_home())
        write(path, content, runner)
        start_service(machine.system, label, path, runner)


def yes(answer: str) -> bool:
    return answer.strip().lower() in ("o", "oui", "y", "yes")


def take_language(argv: List[str]) -> List[str]:
    """`--lang fr|en` (anywhere, for every command): set before anything is said, then taken out of the arguments."""
    rest: List[str] = []
    args = iter(argv)
    for arg in args:
        if arg == "--lang":
            ui.set_language(next(args, ui.LANG))
        elif arg.startswith("--lang="):
            ui.set_language(arg.split("=", 1)[1])
        else:
            rest.append(arg)
    return rest


def install_on_existing(args, runner: hermes.Runner, machine: Machine, existing) -> int:
    """BipAgents on the person's own Hermes: their model, tools, platforms, agents and conversations stay as they are.
    Only the local API and each shown agent's key are added (existing.py), and written down for `uninstall`."""
    from .existing import RECORD, apply_changes, key_of, plan_changes, profile_label, same_version

    say(tr(f"\nTu as déjà Hermes ({existing.version}) : je m'y branche sans toucher à tes réglages, ton modèle "
           f"({existing.model or 'non précisé'}), tes outils ni tes agents.",
           f"\nYou already have Hermes ({existing.version}): I'll plug into it without touching your settings, your model "
           f"({existing.model or 'not set'}), your tools or your agents."))
    if not existing.multiplex:
        say(ui.sad(tr("Cette version d'Hermes ne sert pas plusieurs agents à la même adresse.",
                      "This Hermes version doesn't serve several agents at the same address.")))
        say(tr("Mets-la à jour (hermes update), puis relance cet installeur.",
               "Update it (hermes update), then run this installer again."))
        return 1
    if (Path.home() / ".config" / "bipagents" / "bridge.toml").exists():
        say(ui.sad(tr("BipAgents est déjà installé sur ce compte.", "BipAgents is already installed on this account.")))
        say(tr("Pour le refaire : bash ~/BipAgents/install.sh uninstall, puis relance.",
               "To do it again: bash ~/BipAgents/install.sh uninstall, then run this again."))
        return 1
    ports = pick_block(wanted=args.ports)
    hub_port = existing.api_port if existing.api_enabled and existing.api_port else ports.hermes

    say(tr("\nQuels agents veux-tu voir dans l'app ? (numéros séparés par des virgules)",
           "\nWhich agents do you want to see in the app? (numbers separated by commas)"))
    for i, profile in enumerate(existing.profiles, 1):
        about = f" — {profile.description}" if profile.description else ""
        main_one = tr(" (ton profil principal)", " (your main profile)") if profile.name == hermes.HUB_PROFILE else ""
        say(f"  {i}. {profile_label(profile.name)}{main_one}{about}")
    every = ",".join(str(i) for i in range(1, len(existing.profiles) + 1))
    picked = ask("Agents", every)
    numbers = [int(x) for x in picked.replace(" ", "").split(",") if x.isdigit() and 1 <= int(x) <= len(existing.profiles)]
    shown = [existing.profiles[n - 1] for n in dict.fromkeys(numbers)] or existing.profiles
    names = {p.name: (ask(tr(f"Son nom dans l'app ({p.name})", f"Its name in the app ({p.name})"), profile_label(p.name))
                      or profile_label(p.name)) for p in shown}

    sandbox = hermes.sandbox_runtime()
    if existing.protected:
        say(ui.note(tr("Les outils de tous tes agents tournent dans un bac à sable : le coffre de mots de passe est disponible.",
                       "All your agents' tools run in a sandbox: the password vault is available.")))
    else:
        exposed = ", ".join(profile_label(p.name) for p in existing.profiles if not p.sandboxed)
        say(ui.note(tr(f"{exposed} : terminal sur cette machine, donc accès à tes fichiers et aux clés du bridge. Je ne change "
                       "pas leurs outils,\n    mais le bridge ne gardera pas de mots de passe (pas de coffre).",
                       f"{exposed}: terminal on this machine, so access to your files and the bridge's keys. I won't change "
                       "their tools,\n    but the bridge won't keep any passwords (no vault).")))
    browser = False
    if existing.protected and sandbox and machine.system == "linux":
        say(tr("\nChaque agent peut avoir son navigateur visible : un vrai Chrome sur un bureau à lui, que tu regardes en\n"
               "direct et reprends en main depuis l'app (Bureau). Environ 0,5 Go de mémoire par agent quand il navigue.",
               "\nEach agent can have its own visible browser: a real Chrome on a desktop of its own, that you watch live\n"
               "and take over from the app (Desktop). About 0.5 GB of memory per agent while it browses."))
        browser = yes(ask(tr("Donner un navigateur visible à tes agents ? (o/n)", "Give your agents a visible browser? (y/n)"),
                          tr("o", "y")))
    patch = False
    if same_version(existing, hermes.HERMES_COMMIT):
        patch = yes(ask(tr("Appliquer les correctifs BipAgents à ton Hermes (questions de l'agent dans l'app, photos gardées,\n"
                           "  coffre après un redémarrage du navigateur) ? `hermes update` les retire. (o/n)",
                           "Apply the BipAgents fixes to your Hermes (the agent's questions in the app, photos kept,\n"
                           "  vault after a browser restart)? `hermes update` takes them off. (y/n)"), tr("o", "y")))
    else:
        say(ui.note(tr(f"Ton Hermes ({existing.commit or 'version inconnue'}) n'est pas la version testée "
                       f"({hermes.HERMES_COMMIT}) : pas de correctifs BipAgents, l'app marche sans ces fonctions.",
                       f"Your Hermes ({existing.commit or 'unknown version'}) isn't the tested version "
                       f"({hermes.HERMES_COMMIT}): no BipAgents fixes, the app works without these features.")))

    changes = plan_changes(existing, [p.name for p in shown], hub_port)
    say(tr("\nCe que je change dans ton Hermes :", "\nWhat I change in your Hermes:"))
    for profile, key, value, _ in changes:
        new_key = tr("<nouvelle clé>", "<new key>")
        say(ui.note(f"{profile_label(profile)}{tr(' : ', ': ')}{key} = {new_key if key.endswith('KEY') else value}"))
    if not changes:
        say(ui.note(tr("rien (ton API locale et les clés de tes agents sont déjà là)",
                       "nothing (your local API and your agents' keys are already there)")))
    if patch:
        say(ui.note(tr("les correctifs BipAgents, dans le code d'Hermes", "the BipAgents fixes, in Hermes' code")))
    restart = bool(changes or patch)
    if restart:
        say(ui.note({"service": tr("ta gateway sera redémarrée une fois", "your gateway will be restarted once"),
                     "manual": tr("ta gateway tourne à la main : tu la redémarreras à la fin",
                                  "your gateway runs by hand: you'll restart it at the end"),
                     "stopped": tr("ta gateway ne tourne pas : je l'installe en service, qui démarre seul",
                                   "your gateway isn't running: I'll install it as a service, which starts by itself")}
                    [existing.gateway]))
    say(ui.note(tr("Tout ça s'annule avec la désinstallation de BipAgents.", "Uninstalling BipAgents undoes all of this.")))
    if not yes(ask(tr("\nOn y va ? (o/n)", "\nReady to go? (y/n)"), tr("o", "y"))):
        return 1

    inst = Install(repo=REPO, home=Path.home(), ports=ports, tailnet_name=machine.tailnet_name,
                   lan_address=(machine.lan_addresses or [None])[0], relay_url=args.relay_url, relay_key=args.relay_key,
                   isolated=existing.protected, browser=browser, hermes_port=hub_port)
    total = 6 if browser else 5
    steps = iter(range(1, total + 1))
    say(ui.step(next(steps), total, tr("Ton Hermes", "Your Hermes")))
    patched: List[str] = []
    if patch:
        skipped = hermes.apply_patches(runner, REPO / "hermes" / "patches", applied=patched)
        if skipped:
            say(ui.note(tr(f"Correctifs non appliqués : {', '.join(skipped)} (l'app marche, sans ces fonctions).",
                           f"Fixes not applied: {', '.join(skipped)} (the app works, without these features).")))
    apply_changes(runner, changes, inst.config_dir / RECORD, existing.gateway, patched)
    for profile in shown:
        inst.agents.append(AgentEntry(profile.name, names[profile.name], key_of(existing, changes, profile.name) or "",
                                      root=profile.name == hermes.HUB_PROFILE))
    if restart and existing.gateway == "service":
        runner.run([hermes.hermes_bin(), "gateway", "restart"])
    elif restart and existing.gateway == "stopped":
        gateway_service(inst, machine, runner)
    elif restart:
        say(ui.note(tr("Redémarre ta gateway maintenant (arrête-la, puis hermes gateway run) : elle prendra ces réglages.",
                       "Restart your gateway now (stop it, then hermes gateway run): it will pick up these settings.")))
        if not runner.dry:
            ask(tr("Entrée quand c'est fait", "Press Enter when it's done"))
    say(ui.done(tr("Ton Hermes est prêt pour l'app.", "Your Hermes is ready for the app.")))
    return finish(inst, machine, runner, args, steps, total, sandbox, bot_chat=False)


def finish(inst: Install, machine: Machine, runner: hermes.Runner, args, steps, total: int, sandbox: Optional[str],
           desk: bool = False, bot_chat: bool = True) -> int:
    """What every install ends with: the voices, the bridge, the visible browser when chosen, Tailscale, the checks
    and the QR code."""
    say(ui.step(next(steps), total, tr("Les voix des Bips", "The Bips' voices")))
    if pocket_running():
        say(tr("  déjà en place sur cette machine : je la partage.", "  already set up on this machine: I'll share it."))
    else:
        venv(REPO / "pocket", runner)
        label, path, content = pocket_service(inst, machine.system, args.languages)
        write(path, content, runner)
        start_service(machine.system, label, path, runner)

    # Kyutai (the human voice, the podcasts) is never installed here: it is shared when it already runs, on whatever
    # GPU it was set up (an administrator's, or another machine given with --kyutai-url).
    kyutai = getattr(args, "kyutai_url", None) or KYUTAI_URL
    if voice_engine_answers(kyutai):
        inst.kyutai_url = kyutai.rstrip("/")
        say(tr(f"  Kyutai (voix humaine, podcasts) répond sur {inst.kyutai_url} : je le partage.",
               f"  Kyutai (human voice, podcasts) answers at {inst.kyutai_url}: I'll share it."))
    elif getattr(args, "kyutai_url", None):
        say(ui.note(tr(f"Kyutai ne répond pas sur {kyutai} : voix des Bips seulement (Pocket).",
                       f"Kyutai isn't answering at {kyutai}: Bip voices only (Pocket).")))

    if machine.system == "macos":
        say(tr("  macOS peut demander si Python peut accéder aux appareils du réseau local : accepte, sinon les agents\n"
               "  et le bridge ne joindront pas un modèle installé sur une autre machine de la maison.",
               "  macOS may ask whether Python can reach devices on the local network: allow it, or the agents\n"
               "  and the bridge won't reach a model installed on another machine at home."))
    say(ui.step(next(steps), total, tr("Le bridge, qui relie l'app à tes agents", "The bridge, which links the app to your agents")))
    venv(REPO / "bridge", runner)
    write(inst.bridge_config, bridge_toml(inst), runner, mode=0o600)
    label, path, content = bridge_service(inst, machine.system)
    write(path, content, runner)
    start_service(machine.system, label, path, runner)

    say(ui.done(tr("Bridge démarré.", "Bridge started.")))
    if inst.browser:
        say(ui.step(next(steps), total, tr("Le navigateur visible de tes agents", "Your agents' visible browser")))
        say(ui.note(tr("Je prépare son image : Chrome, son bureau et le serveur qui le pilote (stealth-browser-mcp, récupéré "
                       "sur son dépôt).\n    Compte quelques minutes la première fois.",
                       "Preparing its image: Chrome, its desktop and the server that drives it (stealth-browser-mcp, fetched "
                       "from its repository).\n    Allow a few minutes the first time.")))
        try:
            hermes.build_browser(runner, sandbox, REPO)
            runner.run([str(REPO / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "browser", "add", "--all",
                        "--config", str(inst.bridge_config)])
            if machine.system == "linux":
                runner.run(["systemctl", "--user", "restart", label])   # the bridge reads the agents' desktops
            say(ui.done(tr("Chaque agent a son navigateur, sur son propre bureau.",
                           "Each agent has its browser, on its own desktop.")))
        except RuntimeError as exc:
            say(ui.fail(tr("Le navigateur n'a pas pu être installé : tes agents fonctionnent sans.",
                           "The browser couldn't be installed: your agents work without it.")))
            say(ui.note(str(exc)[-300:]))
            say(ui.note(tr("Pour réessayer plus tard :", "To try again later:")))
            say(f"    {sandbox} build -t {hermes.BROWSER_IMAGE} {REPO / 'browser'}")
            say(f"    {REPO / 'bridge' / '.venv' / 'bin' / 'python'} -m bipbridge browser add --all --config {inst.bridge_config}")
            say(f"    systemctl --user restart {label}")
    say(ui.step(next(steps), total, tr("Tailscale, pour joindre tes agents de partout",
                                       "Tailscale, to reach your agents from anywhere")))
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
        say(tr("  Publication refusée : demande à l'administrateur de la machine de lancer :",
               "  Publishing was refused: ask the machine's administrator to run:"))
        for command in commands:
            say(f"    sudo {command}")

    say(ui.step(next(steps), total, tr("Vérifications", "Checks")))
    if not runner.dry:
        hub = f"http://127.0.0.1:{inst.hub_port}"
        for agent in inst.agents:
            url = hub if agent.root else f"{hub}/p/{agent.name}"
            answers = wait_for(f"{url}/v1/capabilities", {"Authorization": f"Bearer {agent.key}"})
            state = tr("répond", "answers") if answers else tr("ne répond pas encore", "isn't answering yet")
            say((ui.done if answers else ui.fail)(f"{agent.display_name} {state}."))
            if answers and bot_chat:
                httpx.post(f"{url}/api/sessions", json={"title": "Bot Chat"},
                           headers={"Authorization": f"Bearer {agent.key}"}, timeout=10)
        wait_for(f"http://127.0.0.1:{inst.ports.bridge}/health")
    if args.dry_run:
        say(tr("\nÀ blanc : voici ce qui aurait été fait.", "\nDry run: here is what would have been done."))
        hidden = tr("<clé>", "<key>")
        for line in runner.log:
            say(f"  $ {re.sub(r'(_KEY )[^ ]+', lambda m: m.group(1) + hidden, line)}")
        say("\n--- bridge.toml ---")
        say(re.sub(r'(_key = )"[^"]*"', lambda m: f'{m.group(1)}"{hidden}"', bridge_toml(inst)))
        return 0
    if args.no_tailscale:
        say(ui.finale(tr("C'est prêt, sur cette machine seulement.", "It's ready, on this machine only.")))
        say(tr("Sans Tailscale, l'app ne peut pas encore la joindre.", "Without Tailscale, the app can't reach it yet."))
        return 0
    say(ui.finale(tr("C'est prêt ! Scanne ce code dans l'app.", "It's ready! Scan this code in the app.")))
    say(tr("Dans l'app BipAgents : « + » › Scanner le QR code.", "In the BipAgents app: \"+\" › Scan QR code."))
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
        say(tr("Ceci supprime aussi Hermes et toutes les données des agents : conversations, mémoire, fichiers.",
               "This also deletes Hermes and all the agents' data: conversations, memory, files."))
        word = tr("supprimer", "delete")
        if ask(tr("Tape « supprimer » pour confirmer", "Type \"delete\" to confirm")) != word:
            say(tr("Rien n'a été supprimé.", "Nothing was deleted."))
            return 1
    runner = hermes.Runner(dry=args.dry_run)
    uninstall(runner, everything=args.all)
    for line in runner.log:
        say(f"  {'$ ' if not line.startswith('remove') else ''}{line}")
    say(tr("BipAgents est retiré de ce compte.", "BipAgents is removed from this account.")
        + ("" if args.all else tr(" Les données des agents sont gardées dans ~/.hermes "
                                  "(`uninstall --all` pour les supprimer aussi).",
                                  " The agents' data is kept in ~/.hermes (`uninstall --all` to delete it too).")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
