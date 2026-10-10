"""What the machine is: system, memory, processor, GPU, Tailscale. Read only, nothing installed."""
from __future__ import annotations

import ipaddress
import json
import os
import platform
import shutil
import socket
import subprocess
from dataclasses import dataclass, field
from typing import List, Optional

GiB = 1024 ** 3
TAILSCALE_MAC_APP = "/Applications/Tailscale.app/Contents/MacOS/Tailscale"


@dataclass
class Machine:
    system: str              # "macos" | "linux"
    arch: str                # "arm64" | "x86_64"
    memory_gib: float
    cores: int
    cpu: str
    gpu: Optional[str]       # "apple", "nvidia", "amd" or None
    tailscale: Optional[str] = None       # path of the CLI
    tailnet_name: Optional[str] = None    # this machine's MagicDNS name, e.g. "aibox.tail38a3d9.ts.net"
    lan_addresses: List[str] = field(default_factory=list)

    @property
    def summary(self) -> str:
        gpu = {"apple": "Apple Silicon", "nvidia": "GPU Nvidia", "amd": "GPU AMD"}.get(self.gpu or "", "sans GPU")
        return f"{self.cpu}, {self.memory_gib:.0f} Go, {self.cores} cœurs, {gpu}"


def _run(*args: str, timeout: float = 5) -> Optional[str]:
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=True).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None


def memory_bytes() -> int:
    if platform.system() == "Darwin":
        value = _run("sysctl", "-n", "hw.memsize")
        return int(value) if value and value.isdigit() else 0
    try:
        with open("/proc/meminfo", encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("MemTotal:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        pass
    return 0


def cpu_name() -> str:
    if platform.system() == "Darwin":
        return _run("sysctl", "-n", "machdep.cpu.brand_string") or platform.processor() or "Mac"
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or platform.machine()


def gpu_kind() -> Optional[str]:
    if platform.system() == "Darwin":
        return "apple" if platform.machine() == "arm64" else None
    if shutil.which("nvidia-smi") and _run("nvidia-smi", "-L"):
        return "nvidia"
    if os.path.exists("/dev/kfd") or (os.path.isdir("/dev/dri") and _run("sh", "-c", "lspci 2>/dev/null | grep -i 'vga.*amd'")):
        return "amd"
    return None


def tailscale_cli() -> Optional[str]:
    found = shutil.which("tailscale")
    if found:
        return found
    return TAILSCALE_MAC_APP if os.path.exists(TAILSCALE_MAC_APP) else None


def tailnet_name(cli: Optional[str]) -> Optional[str]:
    """This machine's name on the tailnet, from `tailscale status --json` (None when logged out or absent)."""
    if not cli:
        return None
    raw = _run(cli, "status", "--json")
    try:
        name = json.loads(raw or "")["Self"]["DNSName"]
    except (ValueError, KeyError, TypeError):
        return None
    return name.rstrip(".") or None


def lan_addresses() -> List[str]:
    """Private IPv4 addresses of this machine (not the tailnet's 100.64/10, not loopback)."""
    found: List[str] = []
    for target in ("192.168.0.1", "10.0.0.1", "172.16.0.1"):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
                probe.connect((target, 9))  # nothing is sent: only picks the route
                address = probe.getsockname()[0]
        except OSError:
            continue
        ip = ipaddress.ip_address(address)
        if ip.is_private and not ip.is_loopback and ip not in ipaddress.ip_network("100.64.0.0/10") and address not in found:
            found.append(address)
    return found


def detect() -> Machine:
    cli = tailscale_cli()
    return Machine(
        system="macos" if platform.system() == "Darwin" else "linux",
        arch="arm64" if platform.machine() in ("arm64", "aarch64") else platform.machine(),
        memory_gib=memory_bytes() / GiB,
        cores=os.cpu_count() or 1,
        cpu=cpu_name(),
        gpu=gpu_kind(),
        tailscale=cli,
        tailnet_name=tailnet_name(cli),
        lan_addresses=lan_addresses(),
    )
