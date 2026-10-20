"""A place nobody uses goes to sleep (a server of the all-included offer only): no request from an app for
`IDLE_DAYS` and no scheduled task run for a week, its Hermes stops, which gives the server's others about 250 MB of
memory back. The bridge keeps answering; the next request from an app wakes Hermes and waits for it (a few seconds).
Nothing is lost: only Hermes' process stops. A place with routines never sleeps (they run in Hermes).

    data_dir/sleep.json                      {last, asleep}: the last request from an app, and whether Hermes sleeps
    ~/.local/state/bipagents/asleep          while it sleeps: the seat agent tells the guichet (not a stopped Hermes)
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Awaitable, Callable, List, Optional, Sequence

from .config import Config
from .logs import fields

log = logging.getLogger("bipbridge.sleep")

IDLE_DAYS = 14
ROUTINE_DAYS = 8          # a scheduled task run since then: the place has routines, it stays up
CHECK_SECONDS = 1800
WAKE_SECONDS = 90
MARKER = Path(".local") / "state" / "bipagents" / "asleep"
UNITS = ("hermes-gateway", "bipagents-hermes")   # Hermes' own service, or the installer's

Run = Callable[[Sequence[str]], Awaitable[str]]


async def _systemctl(args: Sequence[str]) -> str:
    process = await asyncio.create_subprocess_exec("systemctl", "--user", *args, stdout=asyncio.subprocess.PIPE,
                                                   stderr=asyncio.subprocess.DEVNULL)
    out, _ = await process.communicate()
    if process.returncode != 0:
        raise RuntimeError(f"systemctl {' '.join(args)}: {process.returncode}")
    return out.decode("utf-8", "replace")


class Sleep:
    def __init__(self, config: Config, routines: Callable[[], Awaitable[List[str]]],
                 hermes_up: Callable[[], Awaitable[bool]], run: Run = _systemctl, now: Callable[[], float] = time.time,
                 home: Optional[Path] = None):
        self.config = config
        self.routines = routines        # when each scheduled task last ran (ISO times, the store's cron_jobs)
        self.hermes_up = hermes_up
        self.run = run
        self.now = now
        self.home = home or Path(os.environ.get("HOME") or Path.home())
        self.path = Path(config.outbox.db_path).parent / "sleep.json"
        self.last, self.asleep = now(), False
        self._saved = 0.0
        self._waking: Optional[asyncio.Task] = None
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            self.last, self.asleep = float(data.get("last") or self.last), bool(data.get("asleep"))
        except (OSError, ValueError):
            pass

    @property
    def enabled(self) -> bool:
        return bool(self.config.cloud)

    def _save(self) -> None:
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.path.write_text(json.dumps({"last": self.last, "asleep": self.asleep}), encoding="utf-8")
            self._saved = self.now()
        except OSError:
            pass

    def _mark(self, asleep: bool) -> None:
        marker = self.home / MARKER
        try:
            if asleep:
                marker.parent.mkdir(parents=True, exist_ok=True)
                marker.write_text(str(int(self.now())), encoding="utf-8")
            else:
                marker.unlink(missing_ok=True)
        except OSError:
            pass

    async def _units(self) -> List[str]:
        out = await self.run(["list-units", "--all", "--type=service", "--plain", "--no-legend"])
        names = [line.split()[0] for line in out.splitlines() if line.strip()]
        return [name for name in names if name.startswith(UNITS)]

    async def awake(self) -> None:
        """A request from an app: noted, and Hermes woken first when it sleeps (concurrent requests wait together)."""
        if not self.enabled:
            return
        self.last = self.now()
        if self.asleep:
            if self._waking is None or self._waking.done():
                self._waking = asyncio.create_task(self._wake())
            await asyncio.shield(self._waking)
        elif self.last - self._saved > 600:
            self._save()

    async def _wake(self) -> None:
        try:
            for unit in await self._units():
                await self.run(["start", unit])
        except RuntimeError as exc:
            log.warning("hermes not woken", extra=fields(error=str(exc)))
        deadline = self.now() + WAKE_SECONDS
        while self.now() < deadline and not await self.hermes_up():
            await asyncio.sleep(1)
        self.asleep = False
        self._mark(False)
        self._save()
        log.info("hermes woken")

    async def check(self) -> bool:
        """Puts Hermes to sleep when the place is idle. Whether it did."""
        if not self.enabled or self.asleep or self.now() - self.last < IDLE_DAYS * 86400:
            return False
        since = datetime.now(timezone.utc) - timedelta(days=ROUTINE_DAYS)
        for seen in await self.routines():
            try:
                when = datetime.fromisoformat(seen.replace("Z", "+00:00"))
            except (TypeError, ValueError):
                continue
            if when.tzinfo is None:
                when = when.replace(tzinfo=timezone.utc)
            if when >= since:
                return False
        try:
            units = await self._units()
            for unit in units:
                await self.run(["stop", unit])
        except RuntimeError as exc:
            log.warning("hermes not put to sleep", extra=fields(error=str(exc)))
            return False
        if not units:
            return False
        self.asleep = True
        self._mark(True)
        self._save()
        log.info("hermes asleep", extra=fields(idle_days=int((self.now() - self.last) // 86400)))
        return True

    async def run_forever(self) -> None:
        if self.asleep and await self.hermes_up():   # the server restarted: its services came back up
            self.asleep = False
            self._mark(False)
            self._save()
        while True:
            await asyncio.sleep(CHECK_SECONDS)
            try:
                await self.check()
            except Exception:   # never the bridge's end
                log.exception("sleep check failed")
