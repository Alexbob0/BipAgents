"""The packages of build/voices/ as the stores deliver them: never at a public address (decided 2026-10-20).

- App Store: one Apple-hosted asset pack per language and one for the decoder they share (`voices-fr`, `voices-en`,
  `voices-de`, `voices-es`, `voices-mimi`), downloaded on demand, their files under voices/<code>/ and voices/mimi/:
  build/voices-store/apple/<id>.aar, made by `xcrun ba-package`, uploaded to App Store Connect with Transporter.
- Google Play: the same as on-demand asset packs of the App Bundle (`voices_fr`, …, `voices_mimi`): their files go to
  android/voicepacks/<pack>/src/main/assets/voices/<code>/ (gitignored, hard links), which `bundleRelease` packs.
- The offer's servers (their podcasts): the guichet serves build/voices as it is to its places
  (guichet/bipguichet/voicefiles.py), copied to its `voices` folder.

    voices/onnx/.venv/bin/python voices/onnx/store_packs.py [--apple] [--android]   (both by default)
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "build" / "voices"
APPLE = ROOT / "build" / "voices-store" / "apple"
ANDROID = ROOT / "android" / "voicepacks"
CODES = {"french": "fr", "english": "en", "german": "de", "spanish": "es"}


def packages(source: Path = SOURCE) -> dict:
    """{code: folder in build/voices}, the decoder as « mimi »."""
    index = json.loads((source / "index.json").read_text(encoding="utf-8"))
    found = {CODES[name]: entry["folder"] for name, entry in index["languages"].items() if name in CODES}
    found["mimi"] = index["mimi"]["folder"]
    return found


def _link_tree(source: Path, target: Path) -> None:
    """`source`'s files at `target` as hard links (copies across disks): 500 MB not written twice."""
    shutil.rmtree(target, ignore_errors=True)
    for path in sorted(source.rglob("*")):
        out = target / path.relative_to(source)
        if path.is_dir():
            out.mkdir(parents=True, exist_ok=True)
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(path, out)
        except OSError:
            shutil.copy2(path, out)


def android(found: dict, source: Path = SOURCE, target: Path = ANDROID) -> list:
    made = []
    for code, folder in found.items():
        pack = target / f"voices_{code}"
        if not (pack / "build.gradle.kts").is_file():
            raise SystemExit(f"{pack}: its Gradle module is missing")
        _link_tree(source / folder, pack / "src" / "main" / "assets" / "voices" / code)
        made.append(pack.name)
    return made


def manifest(code: str, folder: str) -> dict:
    """An Apple-hosted pack downloaded only when the app asks (a language's first use, Settings)."""
    return {"assetPackID": f"voices-{code}", "downloadPolicy": {"onDemand": {}},
            "fileSelectors": [{"directorySource": folder, "directoryDestination": f"voices/{code}"}],
            "platforms": ["iOS"]}


def apple(found: dict, source: Path = SOURCE, target: Path = APPLE) -> list:
    if shutil.which("xcrun") is None:
        raise SystemExit("the App Store's packs are made on a Mac with Xcode (xcrun ba-package)")
    target.mkdir(parents=True, exist_ok=True)
    made = []
    for code, folder in found.items():
        path = target / f"voices-{code}.json"
        path.write_text(json.dumps(manifest(code, folder), indent=2), encoding="utf-8")
        archive = target / f"voices-{code}.aar"
        archive.unlink(missing_ok=True)
        # The selectors' paths are relative to where the tool runs: build/voices.
        subprocess.run(["xcrun", "ba-package", str(path), "-o", str(archive)], cwd=source, check=True)
        made.append(archive.name)
    return made


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--apple", action="store_true", help="the App Store's packs only")
    parser.add_argument("--android", action="store_true", help="Google Play's packs only")
    args = parser.parse_args(argv)
    if not (SOURCE / "index.json").is_file():
        print("build/voices is empty: run voices/onnx/build.py first", file=sys.stderr)
        return 1
    found = packages()
    both = not args.apple and not args.android
    if args.android or both:
        print("Google Play:", ", ".join(android(found)))
    if args.apple or both:
        print("App Store:", ", ".join(apple(found)), f"in {APPLE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
