"""``python -m bipbridge [serve|check|genkey|qr]``."""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import json
import secrets
import sys

from .config import ConfigError, config_path, load_config
from .logs import setup_logging


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="bipbridge", description="BipAgents bridge for Hermes agents")
    parser.add_argument("command", nargs="?", default="serve", choices=["serve", "check", "genkey", "qr"],
                        help="serve (default), check the config, print a new random bridge key, "
                             "or show an agent's pairing QR code for the app")
    parser.add_argument("--config", help="config path (default: $BRIDGE_CONFIG or ~/.config/hermes-ios/bridge.toml)")
    parser.add_argument("--agent", help="qr: one agent's own code (default: the install's code, which adds every agent)")
    parser.add_argument("--png", help="qr: also save the QR code as a PNG (one agent)")
    parser.add_argument("--json", action="store_true", help="qr: print the payload instead of the QR code")
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
