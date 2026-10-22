"""The agents' browser kept up to date each night (bipinstall/browser_refresh.py)."""
import os
import time
from pathlib import Path

import pytest

from bipinstall import browser_refresh as refresh
from bipinstall import hermes

ENGINE = "/usr/bin/podman"
REPO = Path("/home/bip/BipAgents")


class Engine:
    """A fake podman: answers by the command's first words, records every command."""

    def __init__(self, check: str, build: int = 0, smoke=(0, "Google Chrome 156.0.8100.12"), ids=("old", "new")):
        self.check, self.build, self.smoke = check, build, smoke
        self.ids = list(ids)
        self.tagged = False
        self.ran = []
        self.running = {}   # container name -> image id

    def __call__(self, args, timeout=600):
        args = list(args)
        self.ran.append(args)
        if args[1:3] == ["image", "inspect"]:
            return (0, self.ids[1] if self.tagged else self.ids[0]) if self.ids[0] else (125, "no such image")
        if args[1] == "run" and "/bin/sh" in args and args[-1] == refresh.CHECK:
            return 0, self.check
        if args[1] == "build":
            return self.build, "" if self.build == 0 else "curl: (6) Could not resolve host: dl.google.com"
        if args[1] == "run" and args[-1] == refresh.SMOKE:
            return self.smoke
        if args[1] == "tag":
            self.tagged = True
            return 0, ""
        if args[1] == "ps":
            return 0, "\n".join(self.running)
        if args[1] == "inspect":
            return 0, "sha256:" + self.running[args[-1]]
        return 0, ""

    def did(self, *words):
        return [a for a in self.ran if a[1:1 + len(words)] == list(words)]


UP_TO_DATE = "browser google-chrome-stable 155.0.8059.84\nbrowser-update 0 \nsecurity 0\n"
NEW_CHROME = "browser google-chrome-stable 155.0.8059.84\nbrowser-update 100 156.0.8100.12-1\nsecurity 0\n"
SECURITY = "browser chromium 155.0.8059.84\nbrowser-update 0 \nsecurity 100\n"
OFFLINE = "browser google-chrome-stable 155.0.8059.84\nbrowser-update 1 \nsecurity 1\n"


def test_it_rebuilds_only_for_a_newer_browser_or_security_updates():
    assert refresh.decide(refresh.parse_check(UP_TO_DATE)) == (False, "Chrome 155.0.8059.84: up to date")
    rebuild, why = refresh.decide(refresh.parse_check(NEW_CHROME))
    assert rebuild is True and "156.0.8100.12-1 is out" in why
    rebuild, why = refresh.decide(refresh.parse_check(SECURITY))
    assert rebuild is True and why.startswith("Chromium 155.0.8059.84 is current; security updates")
    rebuild, why = refresh.decide(refresh.parse_check(OFFLINE))
    assert rebuild is None and "could not ask" in why                          # no network: the image stays
    assert refresh.decide(refresh.parse_check(""))[0] is None                   # the check did not even run
    # dnf without an answer on security only: the browser's own answer decides
    assert refresh.decide(refresh.parse_check(UP_TO_DATE.replace("security 0", "security 2")))[0] is False
    assert refresh.decide(refresh.parse_check(UP_TO_DATE), force=True)[0] is True


def test_up_to_date_builds_nothing_and_cleans_up(tmp_path, capsys):
    engine = Engine(UP_TO_DATE)
    assert refresh.refresh(ENGINE, REPO, tmp_path, run=engine) == 0
    assert not engine.did("build") and not engine.did("tag") and engine.did("image", "prune", "-f")
    check = engine.did("run")[0]
    assert check[:8] == [ENGINE, "run", "--rm", "--pull=never", "--user", "0", "--entrypoint", "/bin/sh"]
    assert check[8] == "bipagents-browser"
    assert "Chrome 155.0.8059.84: up to date" in capsys.readouterr().out


def test_a_new_chrome_is_built_aside_checked_then_named(tmp_path, capsys):
    engine = Engine(NEW_CHROME)
    assert refresh.refresh(ENGINE, REPO, tmp_path, run=engine, stamp="202610220130") == 0
    build = engine.did("build")[0]
    assert build == [ENGINE, "build", "--pull=newer", "--build-arg", "BROWSER_REFRESH=202610220130",
                     "-t", "bipagents-browser:refresh", "-f", str(REPO / "browser" / "Containerfile"),
                     str(REPO / "browser")]
    order = [a[1] for a in engine.ran]
    assert order.index("build") < order.index("tag")
    assert engine.did("tag")[0][2:] == ["bipagents-browser:refresh", "bipagents-browser"]
    assert engine.ran[-1][1:] == ["image", "prune", "-f"]                       # old layers gone
    assert "browser refreshed: Chrome 155.0.8059.84 → 156.0.8100.12" in capsys.readouterr().out


@pytest.mark.parametrize("engine", [Engine(NEW_CHROME, build=1), Engine(NEW_CHROME, smoke=(127, "not found"))])
def test_a_failed_build_keeps_the_working_image(tmp_path, capsys, engine):
    assert refresh.refresh(ENGINE, REPO, tmp_path, run=engine) == 1
    assert not engine.did("tag")                                               # bipagents-browser untouched
    assert [ENGINE, "rmi", "bipagents-browser:refresh"] in engine.ran and engine.did("image", "prune")
    assert "current image kept" in capsys.readouterr().out


def test_no_network_keeps_the_image_and_says_so(tmp_path, capsys):
    engine = Engine(OFFLINE)
    assert refresh.refresh(ENGINE, REPO, tmp_path, run=engine) == 1
    assert not engine.did("build") and "current image kept: could not ask" in capsys.readouterr().out


def test_no_image_no_refresh(tmp_path):
    engine = Engine(UP_TO_DATE, ids=("", ""))
    assert refresh.refresh(ENGINE, REPO, tmp_path, run=engine) == 0 and not engine.did("run")


def test_browsers_on_the_old_image_restart_only_when_unused(tmp_path):
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    root = tmp_path / ".local" / "share" / "hermes-bridge" / "browsers"
    for agent in ("vie", "lea", "max"):
        (units / f"bipagents-browser-{agent}.service").write_text("[Service]\n")
        (root / agent).mkdir(parents=True)
        (root / agent / "busy").touch()
    now = time.time()
    os.utime(root / "vie" / "busy", (now - 3600, now - 3600))                   # unused for an hour
    engine = Engine(NEW_CHROME, ids=("new", "new"))
    engine.running = {"bipagents-browser-vie": "old", "bipagents-browser-lea": "old",   # lea: in use
                      "bipagents-browser-max": "new", "bipagents-browser-zoe": "old"}   # zoe: another account's
    assert refresh.restart_old(ENGINE, tmp_path, engine, now=now) == ["vie"]
    assert [a for a in engine.ran if a[0] == "systemctl"] == [["systemctl", "--user", "restart",
                                                               "bipagents-browser-vie.service"]]


def test_the_browser_folder_follows_the_bridge_config(tmp_path):
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('port = 1\nroot = "/elsewhere"\n\n[browser]\nenabled = true\nroot = "/srv/browsers"\n')
    assert refresh.browsers_root(tmp_path) == Path("/srv/browsers")
    config.write_text('[browser]\nenabled = true\n\n[spaces]\nroot = "/x"\n')
    assert refresh.browsers_root(tmp_path) == tmp_path / ".local" / "share" / "hermes-bridge" / "browsers"


def test_one_refresh_at_a_time_on_a_server(tmp_path):
    lock = tmp_path / "refresh.lock"
    previous = os.umask(0o077)   # a strict umask (an offer place)
    first = refresh.server_lock(lock, wait=0)
    os.umask(previous)
    assert first is not None and lock.stat().st_mode & 0o777 == 0o644          # the other accounts open it too
    waited = []
    assert refresh.server_lock(lock, wait=0, sleep=waited.append) is None      # the other one waits, then gives up
    os.close(first)
    again = refresh.server_lock(lock, wait=0)
    assert again is not None
    os.close(again)


def test_the_timer_runs_each_night_spread_and_at_the_lowest_priority(tmp_path):
    runner = hermes.Runner()
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    python = REPO / "installer" / ".venv" / "bin" / "python"
    refresh.install_timer(runner, REPO, python, tmp_path)
    folder = tmp_path / ".config" / "systemd" / "user"
    service = (folder / "bipagents-refresh-browser.service").read_text()
    timer = (folder / "bipagents-refresh-browser.timer").read_text()
    assert f"ExecStart={python} -m bipinstall browser-refresh" in service
    assert "Type=oneshot" in service and "Nice=19" in service and "IOSchedulingClass=idle" in service
    assert "OnCalendar=*-*-* 01:00:00" in timer and "RandomizedDelaySec=2h" in timer and "Persistent=true" in timer
    assert ran == [["systemctl", "--user", "daemon-reload"],
                   ["systemctl", "--user", "enable", "--now", "bipagents-refresh-browser.timer"]]
    assert not list(folder.glob("bipagents-browser-*.service"))   # never taken for an agent's browser


def test_an_update_gives_existing_installs_the_refresh(tmp_path, monkeypatch):
    from bipinstall import migrations
    runner = hermes.Runner()
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: None)                 # a Mac: no browsers
    assert migrations.browser_refresh(runner, tmp_path) == "no agent browser here" and ran == []
    monkeypatch.setattr(hermes, "sandbox_runtime", lambda: ENGINE)
    monkeypatch.setattr(refresh, "run", Engine(UP_TO_DATE, ids=("", "")))        # Linux, browser never built
    assert migrations.browser_refresh(runner, tmp_path) == "no agent browser here"
    monkeypatch.setattr(refresh, "run", Engine(UP_TO_DATE))
    said = migrations.browser_refresh(runner, tmp_path)
    assert "each night" in said and (tmp_path / ".config" / "systemd" / "user" / "bipagents-refresh-browser.timer").is_file()
    assert ran[-1] == ["systemctl", "--user", "enable", "--now", "bipagents-refresh-browser.timer"]
    assert "2026-10-22-browser-refresh" in [mid for mid, _ in migrations.MIGRATIONS]


def test_a_rebuild_from_an_update_never_reuses_yesterday_s_packages():
    runner = hermes.Runner(dry=True)
    hermes.build_browser(runner, "/usr/bin/docker", REPO)
    assert runner.log[0].startswith("/usr/bin/docker build --pull --build-arg BROWSER_REFRESH=")
    containerfile = (Path(__file__).resolve().parents[2] / "browser" / "Containerfile").read_text()
    assert "FROM registry.fedoraproject.org/fedora:44" in containerfile
    packages = containerfile.index("ARG BROWSER_REFRESH")
    assert containerfile.index('RUN echo "packages of ${BROWSER_REFRESH}"') > packages
    assert containerfile.index("ARG STEALTH_COMMIT") < packages < containerfile.index("git clone")
