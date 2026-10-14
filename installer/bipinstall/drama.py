"""The Bips' expressive voices: Fish Audio Drama 3 on audio.cpp (Vulkan), one server per machine, shared like Pocket.

The first account that can run it brings it up (`bipinstall drama`, or the question during an install): audio.cpp
built at a pinned commit with our patches (`tts/drama3/patches`), for NVIDIA through CUDA (an image built from
NVIDIA's own, `tts/drama3/Containerfile.cuda`, amd64 and arm64) or for other GPUs through Vulkan (a pinned image),
the model downloaded
from Hugging Face at a pinned revision and checked, its « tower » requantized (`tts/drama3/tour-q8-tenseurs.txt`),
the voices of `voices/drama/`, and a systemd user service: a rootless container on an internal network (no way out)
publishing 127.0.0.1:8099 only, its voices warmed up at start. Every bridge of the machine then gets `[drama] url`.

Model license: Fish Audio Community License (personal and small commercial use, no hosted service for others).
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
import re
from typing import Callable, List, Optional, Tuple

import httpx

from . import hermes, ui

PORT = 8099
URL = f"http://127.0.0.1:{PORT}"
SERVICE = "bipagents-drama"
NETWORK = "bipagents-drama3"
AUDIOCPP_REPO = "https://github.com/JarodMica/audio.cpp.git"
AUDIOCPP_COMMIT = "27e92a31737382c9564f0f991666ab9169ca8438"
CUDA_IMAGE = "localhost/bipagents-drama-cuda"
IMAGE = ("docker.io/kyuz0/amd-strix-halo-toolboxes:vulkan-radv"
         "@sha256:e62a1631b48b3190c91e3013110e02a325c69eede4c6c7e60ca2146f531a3a7e")
MODEL_URL = ("https://huggingface.co/Jmica/Drama-3-GGUF/resolve/bcb065b8b0b98535a5c002b39bcceeb8bf065513/"
             "drama-3-backbone-8bit-experts-Q4_K_S.gguf")
MODEL_SIZE = 24_596_497_888
MODEL_SHA256 = "05af716baf78e7c20863b9ccebb3ebdeca9607dd3ea7d0b72c35079e675b212b"
SERVED = "drama-3-q4k-q8-tour-q8.gguf"
SERVED_SIZE = 23_631_152_352
SERVED_SHA256 = "f3678f54c89769fd4c81c09814508f0553e76a496a2ca806aab9837b75d7ad59"
NEEDED_GIB = 26          # ~22 GiB of GPU memory + RAM for the voices + the loading peak
DISK_GIB = 50            # the downloaded model and its requantized copy, for a moment


def answers(url: str = URL) -> bool:
    """A Drama 3 server already runs there (`/v1/models` answers once the model is loaded)."""
    try:
        return httpx.get(f"{url}/v1/models", timeout=2).status_code == 200
    except httpx.HTTPError:
        return False


def _read_int(path: Path) -> int:
    try:
        return int(path.read_text().strip())
    except (OSError, ValueError):
        return 0


def gpu_free_gib(drm: Path = Path("/sys/class/drm")) -> float:
    """What the GPUs can still take: free VRAM, plus free GTT (system memory the GPU maps: most of it on a unified
    memory chip like Strix Halo)."""
    free = 0
    for device in sorted(drm.glob("card*/device")):
        for kind in ("vram", "gtt"):
            total = _read_int(device / f"mem_info_{kind}_total")
            used = _read_int(device / f"mem_info_{kind}_used")
            free += max(0, total - used)
    return free / 2**30


def mem_available_gib(meminfo: Path = Path("/proc/meminfo")) -> float:
    try:
        for line in meminfo.read_text().splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) / 2**20
    except (OSError, ValueError, IndexError):
        pass
    return 0.0


@dataclass
class Gpu:
    """How Drama 3 runs here: through CUDA on an NVIDIA GPU, else through Vulkan."""
    backend: str                      # "cuda" or "vulkan"
    free_gib: float
    cuda: Optional[str] = None        # NVIDIA's image version (fits the driver)
    cuda_arch: Optional[str] = None   # the GPU's compute capability, e.g. "89", "121" (built for it alone)

    @property
    def image(self) -> str:
        return f"{CUDA_IMAGE}:{AUDIOCPP_COMMIT[:12]}-cuda{self.cuda}" if self.backend == "cuda" else IMAGE

    @property
    def bin(self) -> str:
        """Where audio.cpp's programs are, inside the container."""
        return "/opt/audiocpp/bin" if self.backend == "cuda" else "/srv/drama3/audio.cpp/build/vulkan/bin"

    @property
    def spec(self) -> str:
        return ("/opt/audiocpp/model_specs/fish_drama3.json" if self.backend == "cuda"
                else "/srv/drama3/audio.cpp/model_specs/fish_drama3.json")

    @property
    def devices(self) -> List[str]:
        # CUDA: the GPU through podman's CDI (nvidia-ctk); Vulkan: the render nodes.
        return ["--device", "nvidia.com/gpu=all"] if self.backend == "cuda" else ["--device", "/dev/dri", "--group-add", "keep-groups"]


def _run(args: List[str]) -> str:
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=20).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def nvidia(run: Callable[[List[str]], str] = _run) -> Optional[dict]:
    """The NVIDIA GPU, as `nvidia-smi` sees it: free memory (None on a unified-memory chip like DGX Spark's GB10,
    which reports none), compute capability, and the newest CUDA its driver supports. None without one."""
    rows = run(["nvidia-smi", "--query-gpu=memory.free,compute_cap", "--format=csv,noheader,nounits"]).strip()
    if not rows:
        return None
    free_raw, _, cap = rows.splitlines()[0].partition(",")
    found = re.search(r"CUDA Version:\s*(\d+)\.(\d+)", run(["nvidia-smi"]))
    free = float(free_raw.strip()) / 1024 if free_raw.strip().replace(".", "", 1).isdigit() else None
    return {"free_gib": free, "arch": cap.strip().replace(".", "") or None,
            "driver_cuda": (int(found.group(1)), int(found.group(2))) if found else (0, 0)}


def cuda_version(driver_cuda: Tuple[int, int]) -> Optional[str]:
    """NVIDIA's image for this driver: CUDA 13 when it supports it (Blackwell, DGX Spark), else 12.8."""
    if driver_cuda >= (13, 0):
        return "13.0.1"
    if driver_cuda >= (12, 8):
        return "12.8.1"
    return None


def choose(system: str, engine: Optional[str], data_dir: Path, drm: Path = Path("/sys/class/drm"),
           meminfo: Path = Path("/proc/meminfo"), nv: Optional[dict] = None, probe: bool = True,
           cdi: Optional[bool] = None) -> Tuple[Optional[Gpu], Optional[str]]:
    """How this machine would run Drama 3 for the Bips, or why it can't (to tell the person)."""
    if system != "linux":
        return None, "Linux only for now"
    if not engine or Path(engine).name != "podman":
        return None, "it runs in a rootless podman container: podman is missing"
    nv = nv if nv is not None or not probe else nvidia()
    ram = mem_available_gib(meminfo)
    if nv:
        version = cuda_version(nv["driver_cuda"])
        if version is None:
            return None, "the NVIDIA driver is too old for CUDA 12.8: update it"
        ready = cdi if cdi is not None else "nvidia.com/gpu=all" in _run(["nvidia-ctk", "cdi", "list"])
        if not ready:
            return None, ("podman can't reach the NVIDIA GPU yet: run once "
                          "`sudo nvidia-ctk cdi generate --output=/etc/cdi/nvidia.yaml` (NVIDIA Container Toolkit)")
        free = nv["free_gib"] if nv["free_gib"] is not None else ram   # unified memory: the GPU takes the RAM
        if free < NEEDED_GIB:
            return None, f"not enough GPU memory ({free:.0f} GiB free, {NEEDED_GIB} needed)"
        gpu = Gpu("cuda", free, version, nv["arch"])
    else:
        if not any(drm.glob("renderD*")) and not any(Path("/dev/dri").glob("renderD*")):
            return None, "no GPU usable through CUDA or Vulkan"
        free = gpu_free_gib(drm)
        if free < NEEDED_GIB:
            return None, f"not enough GPU memory ({free:.0f} GiB free, {NEEDED_GIB} needed)"
        if ram and ram < NEEDED_GIB and free - ram < NEEDED_GIB:   # unified memory: the GPU takes it from the RAM
            return None, f"not enough free memory ({ram:.0f} GiB available, {NEEDED_GIB} needed)"
        gpu = Gpu("vulkan", free)
    probe_dir = data_dir if data_dir.exists() else data_dir.parent if data_dir.parent.exists() else Path.home()
    disk = shutil.disk_usage(probe_dir).free / 2**30
    if disk < DISK_GIB:
        return None, f"not enough disk space ({disk:.0f} GB free, {DISK_GIB} needed while preparing the model)"
    return gpu, None


def why_not(system: str, engine: Optional[str], data_dir: Path, **kw) -> Optional[str]:
    return choose(system, engine, data_dir, **kw)[1]


@dataclass
class Layout:
    root: Path        # ~/.local/share/bipagents/drama3

    @property
    def src(self) -> Path:
        return self.root / "audio.cpp"

    @property
    def models(self) -> Path:
        return self.root / "models"

    @property
    def voices(self) -> Path:
        return self.root / "voices"

    @property
    def config(self) -> Path:
        return self.root / "server.json"

    @property
    def prewarm(self) -> Path:
        return self.root / "prewarm.sh"


def voices_catalog(repo: Path) -> List[dict]:
    return json.loads((repo / "voices" / "drama" / "voices.json").read_text(encoding="utf-8"))["voices"]


def server_config(repo: Path, gpu: Gpu) -> dict:
    """audio.cpp's config: the model, its 14 voices (reference clip + its transcript) kept warm, real-time settings,
    and a memory guard that refuses to start cleanly rather than crash."""
    presets = {v["id"]: {"voice_ref": f"/srv/drama3/voices/{v['reference']}", "reference_text": v["reference_text"]}
               for v in voices_catalog(repo)}
    return {
        "host": "0.0.0.0", "port": PORT,   # inside the container; podman publishes 127.0.0.1 only
        "backend": gpu.backend, "threads": 8, "lazy_load": False, "max_loaded_models": 1, "ui": False,
        "min_free_memory_mb": 1024,
        "models": [{
            "id": "drama-3", "family": "fish_drama3", "path": f"/srv/drama3/models/{SERVED}", "task": "tts",
            "mode": "streaming", "model_spec_override": gpu.spec,
            "voice_presets": presets,
            "session_options": {"fish_drama3.cpu_expert_layers": 0, "fish_drama3.context_length": 2048,
                                "fish_drama3.stream_block": 4, "fish_drama3.codec_backend": "same",
                                "fish_drama3.prefix_cache": 1, "fish_drama3.prefix_slots": 16},
            "default_request_options": {"max_tokens": 1024, "text_chunk_size": 80},
        }],
    }


def container(engine: str, command: List[str], network: str = "none", publish: bool = False,
              gpu: Optional[Gpu] = None, name: Optional[str] = None) -> List[str]:
    args = [engine, "run", "--rm"] + (["--name", name] if name else []) + ["--network", network]
    if publish:
        args += ["-p", f"127.0.0.1:{PORT}:{PORT}"]
    args += ["--userns", "keep-id", "--user", f"{os.getuid()}:{os.getgid()}", "--security-opt", "label=disable"]
    if gpu:
        args += gpu.devices
    return args + command


def prewarm_script(repo: Path) -> str:
    """Each voice once at start (its reference encoded, its prefix kept): ~1 s each, then 0.3 s to the first audio
    instead of 1 s. « . » warms nothing: a real word is needed."""
    lines = ["#!/usr/bin/env bash", "# Warms Drama 3's voices once the server answers (BipAgents).",
             f"for i in $(seq 1 90); do curl -sf {URL}/v1/models >/dev/null && break; sleep 2; done"]
    for voice in voices_catalog(repo):
        word = "Bonjour." if voice["language"] == "fr" else "Hello."
        body = json.dumps({"model": "drama-3", "voice": voice["id"], "input": word, "max_tokens": 1,
                           "response_format": "pcm", "stream_format": "audio"})
        lines.append(f"curl -sf -o /dev/null -H 'Content-Type: application/json' -d '{body}' {URL}/v1/audio/speech || true")
    return "\n".join(lines) + "\n"


def service_unit(engine: str, layout: Layout, gpu: Gpu) -> str:
    mounts = ["-v", f"{layout.models}:/srv/drama3/models:ro", "-v", f"{layout.voices}:/srv/drama3/voices:ro",
              "-v", f"{layout.config}:/srv/drama3/server.json:ro"]
    if gpu.backend == "vulkan":   # its programs were built in this folder (the CUDA image carries its own)
        mounts += ["-v", f"{layout.src}:/srv/drama3/audio.cpp:ro"]
    run = container(engine, mounts + [gpu.image, f"{gpu.bin}/audiocpp_server", "--config", "/srv/drama3/server.json",
                                      "--no-ui"], network=NETWORK, publish=True, gpu=gpu, name=SERVICE)
    return f"""[Unit]
Description=BipAgents voices (Fish Audio Drama 3 on audio.cpp)
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=600
StartLimitBurst=3

[Service]
Type=simple
ExecStartPre=-{engine} rm -f {SERVICE}
ExecStart={" ".join(run)}
ExecStartPost=/bin/bash {layout.prewarm}
ExecStop={engine} stop -t 5 {SERVICE}
Restart=on-failure
RestartSec=15
TimeoutStartSec=600

[Install]
WantedBy=default.target
"""


def sha256(path: Path, progress: Optional[Callable[[str], None]] = None) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1 << 24), b""):
            digest.update(block)
    return digest.hexdigest()


def install(runner: hermes.Runner, repo: Path, data_dir: Path, engine: str, gpu: Gpu,
            say: Callable[[str], None] = print, served_model: Optional[Path] = None) -> bool:
    """Builds and starts the server in this account. False when a step fails (the Bips keep Pocket).
    `served_model`: the prepared model already on this machine (checked), instead of downloading and preparing it."""
    layout = Layout(data_dir / "drama3")
    try:
        if gpu.backend == "cuda":
            say(f"  audio.cpp for your NVIDIA GPU (CUDA {gpu.cuda}{', sm_' + gpu.cuda_arch if gpu.cuda_arch else ''}), "
                "with our patches: NVIDIA's images are downloaded once (a few GB), then ~10 minutes of building…")
            if runner.dry or subprocess.run([engine, "image", "exists", gpu.image], capture_output=True).returncode != 0:
                runner.run([engine, "build", "--build-arg", f"CUDA={gpu.cuda}", "--build-arg", f"CUDA_ARCH={gpu.cuda_arch or ''}",
                            "-t", gpu.image, "-f", str(repo / "tts" / "drama3" / "Containerfile.cuda"),
                            str(repo / "tts" / "drama3")])
        else:
            say("  audio.cpp, at its pinned version with our patches…")
            if runner.dry or not (layout.src / ".git").exists():
                runner.run(["git", "clone", "--quiet", AUDIOCPP_REPO, str(layout.src)])
            runner.run(["git", "-C", str(layout.src), "am", "--abort"], check=False)   # one left half-done, if any
            runner.run(["git", "-C", str(layout.src), "checkout", "--quiet", "--force", AUDIOCPP_COMMIT])
            runner.run(["git", "-C", str(layout.src), "submodule", "update", "--init", "--recursive", "--quiet"])
            patches = sorted(str(p) for p in (repo / "tts" / "drama3" / "patches").glob("*.patch"))
            runner.run(["git", "-C", str(layout.src), "-c", "user.name=BipAgents", "-c", "user.email=bipagents@localhost",
                        "am", "--quiet", *patches])
            say("  Building it for Vulkan (a minute or two; the image is downloaded once, ~2 GB)…")
            build = ["-e", "LDFLAGS=-fuse-ld=bfd", "-e", f"HOME={layout.root / '.home'}", "-v",
                     f"{layout.root}:{layout.root}", "-w", str(layout.src), IMAGE, "scripts/build_linux.sh", "--backend",
                     "vulkan", "--build-type", "Release", "--model-set", "custom", "--models", "fish_drama3",
                     "--build-dir", "build/vulkan", "--target", "audiocpp_server", "--target", "audiocpp_gguf",
                     "-j", str(os.cpu_count() or 8)]
            if not runner.dry:
                (layout.root / ".home").mkdir(parents=True, exist_ok=True)
            runner.run([engine, "pull", "--quiet", IMAGE])
            runner.run(container(engine, build))

        served = layout.models / SERVED
        if served_model and not runner.dry and not served.exists():
            say("  The prepared model already on this machine: checking it…")
            if served_model.stat().st_size != SERVED_SIZE or sha256(served_model) != SERVED_SHA256:
                raise RuntimeError(f"{served_model} is not the expected model (size or checksum)")
            layout.models.mkdir(parents=True, exist_ok=True)
            try:
                os.link(served_model, served)        # same disk: no second copy
            except OSError:
                shutil.copy2(served_model, served)
        if runner.dry or not (served.exists() and served.stat().st_size == SERVED_SIZE):
            source = layout.models / "drama-3-source.gguf"
            say("  The model: 24.6 GB from Hugging Face (Fish Audio Community License), resumed if cut…")
            if not runner.dry:
                layout.models.mkdir(parents=True, exist_ok=True)
            runner.run(["curl", "-fL", "--retry", "5", "-C", "-", "-o", str(source), MODEL_URL])
            if not runner.dry and sha256(source) != MODEL_SHA256:
                source.unlink(missing_ok=True)
                raise RuntimeError("the downloaded model is not the expected file (checksum): removed, try again")
            say("  Preparing it for real-time speech (its « tower » in 8 bits)…")
            names = (repo / "tts" / "drama3" / "tour-q8-tenseurs.txt").read_text().split()
            keep = [arg for name in names for arg in ("--keep-type", f"{name}=q8_0")]
            tool = (f"{gpu.bin}/audiocpp_gguf" if gpu.backend == "cuda"
                    else str(layout.src / "build" / "vulkan" / "bin" / "audiocpp_gguf"))
            runner.run(container(engine, ["-v", f"{layout.root}:{layout.root}", "-w", str(layout.root), gpu.image, tool,
                                          "--input", str(source), "--output", str(served), "--type", "orig", *keep]))
            if not runner.dry:
                if sha256(served) != SERVED_SHA256:
                    served.unlink(missing_ok=True)
                    raise RuntimeError("the prepared model is not the expected file (checksum)")
                source.unlink(missing_ok=True)   # 24.6 GB back

        runner.log.append(f"copy voices/drama -> {layout.voices}")
        if not runner.dry:
            for language in ("fr", "en"):
                target = layout.voices / language
                target.mkdir(parents=True, exist_ok=True)
                for clip in (repo / "voices" / "drama" / language).glob("*.wav"):
                    shutil.copy2(clip, target / clip.name)
        from .__main__ import start_service, write
        write(layout.config, json.dumps(server_config(repo, gpu), ensure_ascii=False, indent=1) + "\n", runner)
        write(layout.prewarm, prewarm_script(repo), runner, mode=0o755)
        if runner.dry or subprocess.run([engine, "network", "exists", NETWORK], capture_output=True).returncode != 0:
            # Internal: the server reaches nothing (no Internet, no LAN, no host service); 127.0.0.1:8099 only.
            runner.run([engine, "network", "create", "--internal", NETWORK])
        unit = Path.home() / ".config" / "systemd" / "user" / f"{SERVICE}.service"
        write(unit, service_unit(engine, layout, gpu), runner)
        start_service("linux", SERVICE, unit, runner)
        say("  Loading the model and warming the 14 voices (~30 s)…")
        if not runner.dry:
            from .__main__ import wait_for
            if not wait_for(f"{URL}/v1/models", seconds=120):
                raise RuntimeError(f"the server did not come up: journalctl --user -u {SERVICE}")
        return True
    except RuntimeError as exc:
        say(ui.fail("Drama 3 couldn't be set up: the Bips keep their quick voices (Pocket)."))
        say(ui.note(str(exc)[-300:]))
        return False


def connect(config_path: Path, runner: hermes.Runner, url: str = URL) -> bool:
    """This account's bridge speaks with Drama 3's voices: `[drama] url` in its config (once)."""
    if not config_path.exists():
        return False
    text = config_path.read_text(encoding="utf-8")
    if "\n[drama]" in "\n" + text:
        return True
    runner.log.append(f"add [drama] to {config_path}")
    if not runner.dry:
        config_path.write_text(text.rstrip("\n") + f'\n\n[drama]\nurl = "{url}"\n', encoding="utf-8")
    return True


def main(argv: List[str]) -> int:
    """`bipinstall drama`: Drama 3 for this machine's Bips (set up here if it isn't running yet), and this account's
    bridge connected to it. Run it in each account whose bridge should use it."""
    import argparse
    from .__main__ import REPO, venv
    from .detect import detect
    parser = argparse.ArgumentParser(prog="bipinstall drama", description=main.__doc__)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--served-model", type=Path,
                        help=f"the prepared model ({SERVED_SIZE} bytes, SHA-256 {SERVED_SHA256[:12]}…) already on "
                             "this machine, instead of downloading 24.6 GB")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)
    machine = detect()
    data_dir = Path.home() / ".local" / "share" / "bipagents"
    print(ui.bip("happy", "The Bips' expressive voices (Fish Audio Drama 3)."))
    if answers():
        print(ui.done(f"Drama 3 already runs on this machine ({URL}): I'll use it."))
    else:
        gpu, reason = choose(machine.system, hermes.sandbox_runtime(), data_dir)
        if gpu is None:
            print(ui.fail(f"Not on this machine: {reason}. The Bips keep their quick voices (Pocket)."))
            return 1
        print(ui.note(f"Through {'CUDA' if gpu.backend == 'cuda' else 'Vulkan'}, {gpu.free_gib:.0f} GiB of GPU memory free."))
        if not install(runner, REPO, data_dir, hermes.sandbox_runtime() or "podman", gpu, served_model=args.served_model):
            return 1
        print(ui.done("Drama 3 is running, for every account on this machine."))
    config = Path.home() / ".config" / "bipagents" / "bridge.toml"
    if connect(config, runner):
        venv(REPO / "bridge", runner)   # its resampler (soxr) comes with the bridge's requirements
        runner.run(["systemctl", "--user", "restart", "bipagents-bridge"], check=False)
        print(ui.done("Your bridge uses it: pick the new voices in the app (agent › Voice)."))
    else:
        print(ui.note("No BipAgents bridge in this account: run `install.sh drama` in each account that has one."))
    if runner.dry:
        print("\nCommands it would run:")
        for line in runner.log:
            print(f"  $ {line}")
    return 0
