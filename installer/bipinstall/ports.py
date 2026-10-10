"""Each install (one person) gets a block of ten ports, the same layout in every block, so an administrator can
publish a person's block on the tailnet once and for all."""
from __future__ import annotations

import socket
from dataclasses import dataclass
from typing import Callable, Iterable, Optional

FIRST_BLOCK = 8640          # the first install keeps the familiar 864x ports
NEXT_BLOCKS = 9200          # the others: 9200, 9210, 9220…
BLOCK = 10


@dataclass(frozen=True)
class Ports:
    base: int

    @property
    def hermes(self) -> int:     # the multiplex gateway: every agent of the person under /p/<agent>/
        return self.base + 2

    @property
    def bridge(self) -> int:
        return self.base + 3

    @property
    def lan(self) -> int:        # the bridge's door on the local network
        return self.base + 4

    @property
    def desk(self) -> int:       # the agents' browser view (noVNC), when installed
        return self.base + 5

    @property
    def tailnet(self) -> Iterable[int]:
        """What is published on the tailnet (`tailscale serve`); the LAN door stays on the local network."""
        return (self.hermes, self.bridge, self.desk)


def port_free(port: int, host: str = "127.0.0.1") -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind((host, port))
        except OSError:
            return False
    return True


def candidate_bases() -> Iterable[int]:
    yield FIRST_BLOCK
    for i in range(100):
        yield NEXT_BLOCKS + i * BLOCK


def pick_block(is_free: Callable[[int], bool] = port_free, wanted: Optional[int] = None) -> Ports:
    """The first block whose ports are all free (or `wanted`, if given and free)."""
    bases = [wanted] if wanted else candidate_bases()
    for base in bases:
        ports = Ports(base)
        if all(is_free(p) for p in (ports.hermes, ports.bridge, ports.lan, ports.desk)):
            return ports
    raise RuntimeError(f"no free port block{f' at {wanted}' if wanted else ''}")
