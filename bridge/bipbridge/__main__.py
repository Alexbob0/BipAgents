"""``python -m bipbridge [serve|check|genkey]``."""
from __future__ import annotations

import argparse
import secrets
import sys

from .config import ConfigError, config_path, load_config
from .logs import setup_logging


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="bipbridge", description="BipAgents bridge for Hermes agents")
    parser.add_argument("command", nargs="?", default="serve", choices=["serve", "check", "genkey"],
                        help="serve (default), check the config, or print a new random bridge key")
    parser.add_argument("--config", help="config path (default: $BRIDGE_CONFIG or ~/.config/hermes-ios/bridge.toml)")
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
        for agent in config.agents.values():
            print(f"  agent {agent.name}: hermes={agent.hermes_url} ntfy_topic={agent.ntfy_topic or '-'} "
                  f"uploads={agent.upload_dir_host or '-'} -> {agent.upload_dir_container or '-'}")
        return 0

    import uvicorn

    from .app import create_app

    uvicorn.run(create_app(config), host=config.host, port=config.port, log_config=None, access_log=False,
                proxy_headers=True, forwarded_allow_ips="127.0.0.1", timeout_graceful_shutdown=5)
    return 0


if __name__ == "__main__":
    sys.exit(main())
