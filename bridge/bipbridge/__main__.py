"""``python -m bipbridge [serve|check|genkey|qr|peer-token|browser]``."""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import os
import secrets
import sys

from .config import ConfigError, config_path, load_config
from .logs import setup_logging


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="bipbridge", description="BipAgents bridge for Hermes agents")
    parser.add_argument("command", nargs="?", default="serve", choices=["serve", "check", "genkey", "qr", "peer-token", "browser", "shares-mounts", "console", "spaces", "adopt"],
                        help="serve (default), check the config, print a new random bridge key, "
                             "show an agent's pairing QR code for the app, (peer-token --agent X) give an agent set "
                             "up by hand its token for messaging the others, or (browser add|remove --agent X|--all) "
                             "set up or remove agents' visible browsers, or (shares-mounts --agent X) print the "
                             "`-v` mounts an agent set up by hand needs for its shared folders, or (console) give a "
                             "one-time link that opens the web console in a browser, or (spaces start) start again "
                             "the agents' own spaces after an install was moved here, or (adopt) serve the agents "
                             "an export brought to a place as its own (bipinstall import-place)")
    parser.add_argument("--config", help="config path (default: $BRIDGE_CONFIG or ~/.config/hermes-ios/bridge.toml)")
    parser.add_argument("--agent", help="qr: one agent's own code (default: the install's code, which adds every agent)")
    parser.add_argument("--png", help="qr: also save the QR code as a PNG (one agent)")
    parser.add_argument("--json", action="store_true", help="qr: print the payload instead of the QR code")
    parser.add_argument("action", nargs="?", choices=["add", "remove", "start"],
                        help="browser: add or remove; spaces: start")
    parser.add_argument("--all", action="store_true", help="browser add: every agent this install manages")
    args = parser.parse_args(argv)

    if args.command == "genkey":
        print(secrets.token_urlsafe(32))
        return 0

    setup_logging("info")
    try:
        config = load_config(args.config)
    except ConfigError as exc:
        print(f"config error: {exc}", file=sys.stderr)
        return 2
    setup_logging(config.log_level)

    if args.command == "check":
        print(f"config OK: {args.config or config_path()}")
        print(f"  listen      {config.host}:{config.port}")
        print(f"  kyutai      {config.kyutai_url or 'off'}")
        print(f"  pocket      {config.pocket_url or 'off'}")
        print(f"  voice       {config.default_voice} (default)")
        print(f"  ntfy        {config.ntfy_url}")
        print(f"  apns        {'enabled' if config.apns.enabled else 'disabled'} ({config.apns.environment})")
        print(f"  outbox db   {config.outbox.db_path}")
        from .lan import lan_info
        lan = lan_info(config) if config.lan.enabled else None
        print(f"  lan         {lan['url'] if lan else ('enabled, address or certificate missing' if config.lan.enabled else 'off')}")
        for agent in config.agents.values():
            print(f"  agent {agent.name}: hermes={agent.hermes_url} ntfy_topic={agent.ntfy_topic or '-'} "
                  f"uploads={agent.upload_dir_host or '-'} -> {agent.upload_dir_container or '-'}")
        return 0

    if args.command == "qr":
        return _qr(config, args)

    if args.command == "peer-token":
        from .peers import PEER_NAME, Links, manual_token, peer_url
        if not args.agent:
            print("peer-token needs --agent", file=sys.stderr)
            return 2
        links = Links(os.path.join(os.path.dirname(config.outbox.db_path), "peers.json"))
        token = manual_token(config, links, args.agent)
        print(f"# In the HERMES_HOME of the gateway serving {args.agent} (the bridge must be reachable from there):")
        print(f"hermes config set bot_peers.{PEER_NAME}.url {peer_url(config)}")
        print(f"# In that agent's profile .env (its old token, if any, stops working):")
        print(f"HERMES_PEER_{PEER_NAME.upper()}_KEY={token}")
        return 0

    if args.command == "shares-mounts":
        # For the start script of an agent set up by hand (shares = "external"): one `-v` argument per line.
        from .shares import mounts
        if not args.agent or config.agent(args.agent) is None:
            print("shares-mounts needs --agent X (an agent of this bridge)", file=sys.stderr)
            return 2
        for mount in mounts(config, config.agent(args.agent).name):
            print(f"-v\n{mount}")
        return 0

    if args.command == "browser":
        return asyncio.run(_browser(config, args))

    if args.command == "spaces":
        return asyncio.run(_spaces(config))

    if args.command == "adopt":
        return asyncio.run(_adopt(config))

    if args.command == "console":
        from .console import LINK_MINUTES, ConsoleStore
        store = ConsoleStore(os.path.join(os.path.dirname(config.outbox.db_path), "console.json"))
        base = (config.public_url or f"http://127.0.0.1:{config.port}").rstrip("/")
        # The code goes after « # »: the browser keeps it, no server or proxy log ever sees it.
        print(f"Open this link in your computer's browser (once, within {LINK_MINUTES} minutes):\n\n"
              f"  {base}/console/#{store.new_link()}\n")
        return 0

    import uvicorn

    from .app import create_app

    app = create_app(config)
    if not config.lan.enabled:
        uvicorn.run(app, host=config.host, port=config.port, log_config=None, access_log=False,
                    proxy_headers=True, forwarded_allow_ips="127.0.0.1", timeout_graceful_shutdown=5)
        return 0
    asyncio.run(_serve_with_lan(app, config))
    return 0


async def _serve_with_lan(app, config) -> None:
    """The usual listener (127.0.0.1, published by `tailscale serve`) plus the LAN door over HTTPS, one process and
    one set of services: the door starts once the main server has run the app's startup, and stops with it."""
    import uvicorn

    from .lan import ensure_certificate

    class Door(uvicorn.Server):
        @contextlib.contextmanager
        def capture_signals(self):  # the main server handles SIGTERM / Ctrl-C and stops the door
            yield

    cert_path, key_path = ensure_certificate(config)
    main = uvicorn.Server(uvicorn.Config(app, host=config.host, port=config.port, log_config=None, access_log=False,
                                         proxy_headers=True, forwarded_allow_ips="127.0.0.1",
                                         timeout_graceful_shutdown=5))
    door = Door(uvicorn.Config(app, host=config.lan.host, port=config.lan.port, ssl_certfile=cert_path,
                               ssl_keyfile=key_path, lifespan="off", log_config=None, access_log=False,
                               timeout_graceful_shutdown=5))
    main_task = asyncio.create_task(main.serve())
    while not main.started and not main_task.done():
        await asyncio.sleep(0.05)
    door_task = asyncio.create_task(door.serve()) if not main_task.done() else None
    try:
        await main_task
    finally:
        if door_task:
            door.should_exit = True
            await door_task


async def _browser(config, args) -> int:
    """Agents' visible browsers (browsers.py), set up from the command line: the installer, or an administrator adding
    one to an agent that has none. Run it while the bridge is stopped, or restart the bridge after: it rewrites the
    agents' sections of the config file."""
    from . import browsers
    from .agents_admin import home_root, managed, run_hermes, save_agent

    if args.all:
        names = [a.name for a in config.agents.values() if _manages(config, a.name)]
    elif args.agent:
        names = [args.agent]
    else:
        print("browser needs --agent X or --all", file=sys.stderr)
        return 2
    failed = 0
    for name in names:
        try:
            agent = managed(config, name)
        except LookupError:
            print(f"{name}: not an agent this install manages", file=sys.stderr)
            failed += 1
            continue
        try:
            if args.action == "remove":
                await browsers.remove(config, name, run_hermes)
                agent.desktop_vnc = agent.desktop_vnc_password_file = None
            else:
                agent = await browsers.setup(config, agent, run_hermes, home_root(config, agent))
            save_agent(config, agent)
            print(f"{name}: browser {'removed' if args.action == 'remove' else 'ready'}")
        except (browsers.BrowserError, RuntimeError, OSError) as exc:
            print(f"{name}: {exc}", file=sys.stderr)
            failed += 1
    return 1 if failed else 0


async def _adopt(config) -> int:
    """After an export was put into this place (installer/bipinstall/to_place.py): each agent it brought served as
    the place's own are (Bot Mode, its files where its sandbox sees them, the bridge's tools), and each profile no
    agent serves yet (a Hermes alone's) claimed. Run it with the gateway up and the bridge stopped, or restart the
    bridge after: it rewrites the agents' sections of the config file."""
    import httpx

    from . import shares
    from .agents_admin import (claim_profile, give_tools, managed, mark_as_bot, mount_uploads, run_hermes,
                               unclaimed_profiles)

    failed = 0
    for agent in list(config.agents.values()):
        try:
            managed(config, agent.name)
        except LookupError:
            continue
        try:
            folder = shares.profile_dir(config, agent)
            if folder is not None and folder.is_dir():
                mark_as_bot(folder)
            agent = await mount_uploads(config, agent, run_hermes, save=True)
            config.agents[agent.name] = agent
            await give_tools(config, agent, run_hermes)
            print(f"{agent.name}: ready")
        except (RuntimeError, OSError) as exc:
            print(f"{agent.name}: {exc}", file=sys.stderr)
            failed += 1
    async with httpx.AsyncClient(timeout=30) as http:
        for profile in unclaimed_profiles(config):
            try:
                agent = await claim_profile(config, profile["id"], None, http)
                print(f"{agent.name}: claimed")
            except (LookupError, RuntimeError, OSError, httpx.HTTPError, asyncio.TimeoutError) as exc:
                print(f"{profile['id']}: {exc}", file=sys.stderr)
                failed += 1
    return 1 if failed else 0


async def _spaces(config) -> int:
    """Each agent with a space of its own (spaces.py) gets its gateway's service again: after `bipagents import`
    brought the install to this machine (the spaces' folders came with it, their services did not)."""
    from .agents_admin import run_hermes
    from .spaces import SpaceError, Spaces

    spaces, failed = Spaces(run_hermes), 0
    for agent in config.agents.values():
        if not agent.space:
            continue
        try:
            await spaces.start(config, agent)
            print(f"{agent.name}: space started")
        except (SpaceError, RuntimeError, OSError) as exc:
            print(f"{agent.name}: {exc}", file=sys.stderr)
            failed += 1
    return 1 if failed else 0


def _manages(config, name) -> bool:
    from .agents_admin import managed
    try:
        managed(config, name)
        return True
    except LookupError:
        return False


def _qr(config, args) -> int:
    """An agent's pairing QR code in the terminal (scan it in the app: Ajouter un agent › Scanner le QR code)."""
    from .lan import ensure_certificate, install_payload, pairing_payload

    if config.lan.enabled:
        ensure_certificate(config)  # the fingerprint goes in the payload
    if args.agent and config.agent(args.agent) is None:
        print(f"unknown agent: {args.agent}", file=sys.stderr)
        return 2
    try:
        if args.agent:
            agent = config.agent(args.agent)
            label, data = agent.display_name, pairing_payload(config, agent)
        else:
            label, data = "BipAgents", install_payload(config)
    except ValueError as exc:
        print(f"config error: {exc}", file=sys.stderr)
        return 2
    payload = json.dumps(data, separators=(",", ":"), ensure_ascii=False)
    if args.json:
        print(payload)
        return 0
    import segno

    code = segno.make(payload, error="m")
    print(f"\n{label} — contains keys: show it only to your own phone.\n")
    code.terminal(compact=True)
    if args.png:
        code.save(args.png, scale=8, border=2)
        print(f"saved: {args.png}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
