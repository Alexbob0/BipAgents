"""The Bips' voice packages (build.py's `build/voices`) as the assets of a GitHub release of the public repository,
for the installs that have no guichet (installer/bipinstall/voices.py fetches them from there). Standard library only:

    python3 voices/onnx/release.py tag  build/voices          the release's name: voices-<engine>-<index hash>
    python3 voices/onnx/release.py pack build/voices <out>    the assets into <out>, the release's name printed

The assets, each well under GitHub's 2 GB a file:

    index.json                 build.py's, as it is (the installer checks it against SHA256SUMS)
    <folder>.tar.gz            one per package the index lists (each language, the decoder `mimi-…`): the folder's
                               files under `<folder>/`, the same bytes every time (sorted, no dates, no owners)
    CREDITS.txt                what each package is made from and under which licence (the manifests' `credit`)
    LICENSE-ASSETS             the Bips' voices are reserved assets: fetched by BipAgents' installer, not reused
    SHA256SUMS                 the sha256 of every asset above

Folders the index doesn't list (older builds, samples/) are left out. The name changes with the index, so with any
package: the same build gives the same name, and scripts/publish-voices.sh skips a release already out."""
from __future__ import annotations

import gzip
import hashlib
import json
import sys
import tarfile
from pathlib import Path
from typing import Dict, List

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
LIMIT = 2 * 1000 ** 3 - 50 * 1000 ** 2   # GitHub refuses an asset of 2 GB or more: a margin under it

CREDITS_HEAD = """The Bips' voices of BipAgents, as BipAgents' installer fetches them ({tag})

Each archive holds one package: a language's model and Bips, or the audio decoder they share. These packages are
Kyutai's Pocket TTS converted to ONNX (the language model quantized to 8 bits) with the Bips' voice states.

- Pocket TTS by Kyutai, https://github.com/kyutai-labs/pocket-tts, weights under CC-BY-4.0
  (https://creativecommons.org/licenses/by/4.0/). Changes: converted to ONNX, the language model quantized to 8 bits.
- The Bips' voices (the voices/*.kv.f16 states) are BipAgents' own, reserved assets (LICENSE-ASSETS): published here
  for BipAgents' installer, not for reuse elsewhere.

Each package's own credit, as its manifest.json says it:
"""


def index_of(src: Path) -> Dict:
    return json.loads((src / "index.json").read_text(encoding="utf-8"))


def packages(index: Dict) -> List[str]:
    """The folders the index lists: the decoder, then each language."""
    return [index["mimi"]["folder"]] + [entry["folder"] for entry in index["languages"].values()]


def tag(src: Path) -> str:
    """voices-<engine version>-<sha256 of index.json, 12 characters>: « voices-4.0.1-1a2b3c4d5e6f »."""
    engine = str(index_of(src).get("engine", "")).split()[-1:] or ["unknown"]
    digest = hashlib.sha256((src / "index.json").read_bytes()).hexdigest()[:12]
    return f"voices-{engine[0]}-{digest}"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def archive(folder: Path, out: Path) -> Path:
    """`folder` as `<out>/<folder name>.tar.gz`, reproducible: its files sorted, dated 0, owned by nobody, readable."""
    target = out / f"{folder.name}.tar.gz"
    paths = sorted(p for p in folder.rglob("*") if p.is_file() or p.is_dir())
    with open(target, "wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0,
                                                  compresslevel=6) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
            for path in [folder] + paths:
                if path.is_symlink():
                    raise SystemExit(f"{path}: a link in a package")
                arcname = folder.name if path == folder else f"{folder.name}/{path.relative_to(folder).as_posix()}"
                info = tar.gettarinfo(str(path), arcname=arcname)
                info.mtime, info.uid, info.gid, info.uname, info.gname = 0, 0, 0, "", ""
                info.mode = 0o755 if path.is_dir() else 0o644
                info.pax_headers = {}
                if path.is_dir():
                    tar.addfile(info)
                else:
                    with open(path, "rb") as fh:
                        tar.addfile(info, fh)
    return target


def credits(src: Path, index: Dict, name: str) -> str:
    lines = [CREDITS_HEAD.format(tag=name)]
    for folder in packages(index):
        manifest = json.loads((src / folder / "manifest.json").read_text(encoding="utf-8"))
        lines.append(f"{folder} ({manifest.get('engine', '?')}):\n  {manifest.get('credit', '(no credit line)')}\n")
    return "\n".join(lines)


def pack(src: Path, out: Path) -> str:
    """The release's assets into `out` (an empty folder): its name."""
    index = index_of(src)
    name = tag(src)
    out.mkdir(parents=True, exist_ok=True)
    assets: List[Path] = []
    for folder in packages(index):
        if not (src / folder / "manifest.json").is_file():
            raise SystemExit(f"{src / folder}: listed in index.json but not built")
        assets.append(archive(src / folder, out))
    (out / "index.json").write_bytes((src / "index.json").read_bytes())
    (out / "CREDITS.txt").write_text(credits(src, index, name), encoding="utf-8")
    (out / "LICENSE-ASSETS").write_bytes((REPO / "LICENSE-ASSETS").read_bytes())
    assets += [out / "index.json", out / "CREDITS.txt", out / "LICENSE-ASSETS"]
    for path in assets:
        if path.stat().st_size >= LIMIT:
            raise SystemExit(f"{path.name}: {path.stat().st_size} bytes, over GitHub's limit for an asset")
    (out / "SHA256SUMS").write_text("".join(f"{_sha256(p)}  {p.name}\n" for p in sorted(assets)), encoding="utf-8")
    return name


def main(argv: List[str]) -> int:
    if len(argv) == 2 and argv[0] == "tag":
        print(tag(Path(argv[1])))
        return 0
    if len(argv) == 3 and argv[0] == "pack":
        print(pack(Path(argv[1]), Path(argv[2])))
        return 0
    print(__doc__.split("\n\n")[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
