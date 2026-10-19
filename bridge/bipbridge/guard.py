"""Wrong keys, counted by address: an address that presents `limit` wrong keys (the bridge's, an agent's through the
door, a pairing proof) within `window` seconds is refused for `block` seconds, and each refusal is logged. Mostly for
a bridge on the Internet (a server the app created, `cloud = true`), where anyone can knock; on the tailnet or a LAN it
costs nothing. The app's own requests never come close: one wrong key is a key that changed."""
from __future__ import annotations

import ipaddress
import logging
import time
from typing import Callable, Dict, List, Optional

from .logs import fields

log = logging.getLogger("bipbridge.guard")


class Guard:
    def __init__(self, limit: int = 20, window: float = 600, block: float = 900,
                 clock: Callable[[], float] = time.monotonic):
        self.limit, self.window, self.block, self.clock = limit, window, block, clock
        self._failures: Dict[str, List[float]] = {}
        self._blocked: Dict[str, float] = {}

    @staticmethod
    def _counted(address: Optional[str]) -> bool:
        """Not this machine itself (its own services, the main listener's proxy)."""
        if not address:
            return False
        try:
            return not ipaddress.ip_address(address).is_loopback
        except ValueError:
            return True

    def blocked(self, address: Optional[str]) -> bool:
        until = self._blocked.get(address or "")
        if until is None:
            return False
        if self.clock() >= until:
            self._blocked.pop(address or "", None)
            return False
        return True

    def fail(self, address: Optional[str], what: str) -> None:
        if not self._counted(address):
            return
        now = self.clock()
        recent = [t for t in self._failures.get(address, []) if now - t < self.window] + [now]
        self._failures[address] = recent
        log.warning("wrong key", extra=fields(address=address, what=what, count=len(recent)))
        if len(recent) >= self.limit:
            self._blocked[address] = now + self.block
            self._failures.pop(address, None)
            log.warning("address blocked", extra=fields(address=address, minutes=round(self.block / 60)))
        if len(self._failures) > 10_000:   # never grows without end: the oldest addresses are forgotten
            for old in sorted(self._failures, key=lambda a: self._failures[a][-1])[:5_000]:
                self._failures.pop(old, None)
