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
    tailnet_name: Optional[str] = None    # this machine's MagicDNS name, e.g. "aibox.tail1234.ts.net"
    lan_addresses: List[str] = field(default_factory=list)
    cloud: Optional[str] = None           # the provider of a rented server (VPS), e.g. "Hetzner"

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


# What a rented server's firmware calls its maker (/sys/class/dmi/id/sys_vendor, then product_name).
CLOUD_VENDORS = (("hetzner", "Hetzner"), ("digitalocean", "DigitalOcean"), ("amazon ec2", "AWS"), ("amazon", "AWS"),
                 ("google", "Google Cloud"), ("ovh", "OVHcloud"), ("scaleway", "Scaleway"), ("vultr", "Vultr"),
                 ("linode", "Akamai Linode"), ("akamai", "Akamai Linode"), ("oracle", "Oracle Cloud"),
                 ("contabo", "Contabo"), ("ionos", "IONOS"), ("upcloud", "UpCloud"), ("exoscale", "Exoscale"),
                 ("infomaniak", "Infomaniak"), ("microsoft corporation", "Azure"))


def cloud_vendor(dmi: str = "/sys/class/dmi/id") -> Optional[str]:
    """The provider when this machine is a rented server (Linux reads it from the firmware), else None."""
    seen = []
    for name in ("sys_vendor", "product_name", "chassis_vendor", "bios_vendor"):
        try:
            with open(os.path.join(dmi, name), encoding="utf-8", errors="replace") as fh:
                seen.append(fh.read().strip().lower())
        except OSError:
            continue
    text = " ".join(seen)
    if "microsoft corporation" in text and "virtual machine" not in text:
        return None   # a Surface, not an Azure VM
    return next((label for key, label in CLOUD_VENDORS if key in text), None)


def local_timezone() -> Optional[str]:
    """« Europe/Paris »: from TZ, /etc/localtime's link or /etc/timezone (None when unknown)."""
    if os.environ.get("TZ", "").count("/") >= 1:
        return os.environ["TZ"].lstrip(":")
    try:
        target = os.path.realpath("/etc/localtime")
        if "zoneinfo/" in target:
            return target.split("zoneinfo/", 1)[1]
    except OSError:
        pass
    try:
        with open("/etc/timezone", encoding="utf-8") as fh:
            name = fh.read().strip()
            return name or None
    except OSError:
        return None


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
        cloud=cloud_vendor() if platform.system() == "Linux" else None,
    )
