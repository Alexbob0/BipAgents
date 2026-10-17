import asyncio
import json

import pytest

from bipbridge import shares
from bipbridge.config import parse_config
from conftest import AUTH


@pytest.fixture
def setup(config_dict, tmp_path):
    hub = tmp_path / ".hermes"
    for name, terminal in (("vie", "docker"), ("wellness", "docker"), ("budget", "local")):
        folder = hub / "profiles" / name
        folder.mkdir(parents=True)
        volumes = '\n  docker_volumes:\n    - "/home/x/photos:/photos:ro"' if name == "vie" else ""
        (folder / "config.yaml").write_text(f"terminal:\n  backend: {terminal}{volumes}\n")
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642",
                             "isolated": True}
    config_dict["outbox"] = {"db_path": str(tmp_path / "data" / "bridge.db")}
    for name in ("vie", "wellness", "budget"):
        uploads = str(hub / "profiles" / name / "uploads")
        config_dict["agents"][name.title()] = {"hermes_url": f"http://127.0.0.1:8642/p/{name}", "hermes_key": "k" * 32,
                                               "upload_dir_host": uploads, "upload_dir_container": uploads}
    config = parse_config(config_dict)
    commands = []

    async def run(args, env):
        commands.append((list(args), dict(env)))
        return ""

    return config, hub, run, commands, tmp_path


def volumes_set(commands, agent):
    return next(json.loads(args[6]) for args, _ in reversed(commands)
                if args[1:6] == ["-p", agent, "config", "set", "terminal.docker_volumes"])


def test_an_agent_reads_what_another_shares_and_nothing_else(setup):
    config, hub, run, commands, tmp = setup
    root = tmp / "data" / "shares"
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))
    # Wellness writes in /partage; Vie reads it, read-only; the person's own mount is kept.
    assert volumes_set(commands, "wellness") == [f"{root}/wellness:/partage:z"]
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro", f"{root}/wellness:/lecture/wellness:ro,z"]
    assert (root / "wellness").is_dir() and oct((root / "wellness").stat().st_mode & 0o777) == "0o777"
    assert (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").exists()
    assert shares.load(config) == [("vie", "wellness")]
    # Taken back: the mounts go, the person's stays.
    commands.clear()
    asyncio.run(shares.apply(config, [], run))
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro"] and volumes_set(commands, "wellness") == []
    assert not (hub / "profiles" / "vie" / "skills" / "shared-data").exists()


def test_only_agents_in_a_sandbox_take_part(setup):
    config, hub, run, commands, tmp = setup
    assert shares.why_not(config, config.agent("budget")) == "no_sandbox"
    with pytest.raises(ValueError):
        asyncio.run(shares.apply(config, [("budget", "wellness")], run))
    with pytest.raises(ValueError):
        asyncio.run(shares.apply(config, [("vie", "vie")], run))
    assert commands == [] and shares.load(config) == []


def test_deleting_an_agent_stops_the_others_reading_it(setup):
    config, hub, run, commands, tmp = setup
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))
    config.agents.pop("wellness")
    commands.clear()
    asyncio.run(shares.forget(config, "wellness", run))
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro"]
    assert shares.load(config) == [] and not (tmp / "data" / "shares" / "wellness").exists()


def test_routes(client):
    listing = client.get("/v1/agent-shares", headers=AUTH).json()
    assert listing == {"agents": [{"id": "wellness", "name": "Wellness", "why_not": "not_managed", "external": False}],
                       "reads": []}
    assert client.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "wellness", "source": "x"}]}
                      ).status_code == 400


def test_an_agent_set_up_by_hand_mounts_its_folders_itself(setup, config_dict, tmp_path):
    """Vie in a container of its own: the bridge writes nothing in it, its start script mounts what
    `mounts` lists, and the administrator's command runs when that changed."""
    config, hub, run, commands, tmp = setup
    vie = config.agent("vie")
    vie.hermes_url, vie.shares, vie.shares_apply = "http://127.0.0.1:8644", "external", "restart-when-idle {agent}"
    assert shares.why_not(config, vie) is None
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))
    root = tmp / "data" / "shares"
    assert shares.mounts(config, "vie") == [f"{root}/wellness:/lecture/wellness:ro,z"]
    assert shares.mounts(config, "wellness") == [f"{root}/wellness:/partage:z"]
    assert (root / "wellness").is_dir()
    assert (["sh", "-c", "restart-when-idle vie"], {}) in commands
    assert not any(args[1:3] == ["-p", "vie"] for args, _ in commands)     # nothing written in its config
    commands.clear()
    asyncio.run(shares.apply(config, [("vie", "wellness")], run))           # unchanged: not restarted
    assert (["sh", "-c", "restart-when-idle vie"], {}) not in commands


def test_a_profile_without_its_own_terminal_setting_runs_in_the_hubs_sandbox(setup):
    """A profile created without --clone has no terminal setting: its gateway runs it with the hub's (TERMINAL_ENV in
    the hub's .env), so it can share."""
    config, hub, run, commands, tmp = setup
    (hub / "profiles" / "budget" / "config.yaml").write_text("model:\n  provider: custom\n")
    assert shares.why_not(config, config.agent("budget")) == "no_sandbox"
    (hub / ".env").write_text("API_SERVER_ENABLED=true\nTERMINAL_ENV=docker\n")
    assert shares.why_not(config, config.agent("budget")) is None
    (hub / "profiles" / "budget" / ".env").write_text("TERMINAL_ENV=local\n")
    assert shares.why_not(config, config.agent("budget")) == "no_sandbox"   # the profile's own word wins


def test_a_sandboxed_agent_opens_the_files_it_is_sent(setup):
    from io import BytesIO

    from starlette.datastructures import UploadFile

    from bipbridge import agents_admin, files
    config, hub, run, commands, tmp = setup
    uploads = hub / "profiles" / "vie" / "uploads"
    (uploads / "2026-10").mkdir(parents=True)
    old = uploads / "2026-10" / "0123456789ab-avant.pdf"
    old.write_bytes(b"%PDF")
    old.chmod(0o640)
    asyncio.run(agents_admin.mount_all_uploads(config, run))
    # Its sandbox sees nothing of this machine but its mounts: the uploads join the person's own, read-only.
    assert volumes_set(commands, "vie") == ["/home/x/photos:/photos:ro", f"{uploads}:/fichiers:ro,z"]
    assert config.agent("vie").upload_dir_container == "/fichiers"
    assert oct(uploads.stat().st_mode & 0o777) == "0o755" and oct(old.stat().st_mode & 0o777) == "0o644"
    # Tools that run here: nothing to mount, the path stays the machine's.
    assert config.agent("budget").upload_dir_container == str(hub / "profiles" / "budget" / "uploads")
    assert not any(args[2] == "budget" for args, _ in commands)
    # Again (each start), once Hermes holds them: nothing changes.
    for args, _ in commands:
        (hub / "profiles" / args[2] / "config.yaml").write_text(
            "terminal:\n  backend: docker\n  docker_volumes: " + args[6] + "\n")
    commands.clear()
    asyncio.run(agents_admin.mount_all_uploads(config, run))
    assert commands == []
    # A file sent now: named as the sandbox sees it, readable by the sandbox's user.
    upload = UploadFile(BytesIO(b"%PDF-1.7"), filename="statuts.pdf")
    stored = asyncio.run(files.store_upload(config.agent("vie"), upload, config.limits))
    assert stored["path"].startswith("/fichiers/") and stored["path"].endswith("-statuts.pdf")
    on_disk = uploads / stored["path"].removeprefix("/fichiers/")
    assert on_disk.read_bytes() == b"%PDF-1.7" and oct(on_disk.stat().st_mode & 0o777) == "0o644"
