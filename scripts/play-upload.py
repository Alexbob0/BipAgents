#!/usr/bin/env python3
"""Builds the Android app and uploads it to a Google Play testing track (internal testing by default).

The Android counterpart of testflight.sh. Every upload is a new build of the same version (versionName). The version code is yy + day of the year + hour and minute (yyDDDHHmm), given
to Gradle at each build (its configuration cache would otherwise keep an older one).

    scripts/play-upload.py                  build the signed bundle and upload it to internal testing
    scripts/play-upload.py --track alpha    another testing track (production stays manual, in the Play Console)
    scripts/play-upload.py --no-build       upload the last bundle built

Needs: android/local.properties with the upload key (upload.*) and Firebase (firebase.*); the Play publishing service
account's JSON key at ~/.config/bipagents/play-publisher.json (or --key), that account invited in the Play Console with
« Release to testing tracks » for this app; the app's first bundle uploaded by hand (Google requires it).
Runs with the bridge's virtualenv (httpx, PyJWT): bridge/.venv/bin/python scripts/play-upload.py
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import httpx
import jwt

ROOT = Path(__file__).resolve().parent.parent
ANDROID = ROOT / "android"
BUNDLE = ANDROID / "app/build/outputs/bundle/release/app-release.aab"
PACKAGE = "io.github.bipagents"
API = f"https://androidpublisher.googleapis.com/androidpublisher/v3/applications/{PACKAGE}"
UPLOAD = f"https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/{PACKAGE}"
JBR = "/Applications/Android Studio.app/Contents/jbr/Contents/Home"


def access_token(key_path: Path) -> str:
    account = json.loads(key_path.read_text())
    now = int(time.time())
    assertion = jwt.encode({"iss": account["client_email"], "scope": "https://www.googleapis.com/auth/androidpublisher",
                            "aud": account["token_uri"], "iat": now, "exp": now + 3600}, account["private_key"],
                           algorithm="RS256")
    resp = httpx.post(account["token_uri"], data={"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                                                  "assertion": assertion}, timeout=30)
    resp.raise_for_status()
    return resp.json()["access_token"]


def retried(send, attempts: int = 4):
    """Google Play answers 500/503 now and then: the same request again, a little later."""
    for attempt in range(attempts):
        resp = send()
        if resp.status_code < 500 or attempt == attempts - 1:
            return resp
        time.sleep(5 * (attempt + 1))


def check(resp: httpx.Response) -> dict:
    if resp.status_code >= 400:
        sys.exit(f"Google Play refused ({resp.status_code}): {resp.text[:500]}")
    return resp.json() if resp.content else {}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--track", default="internal", choices=["internal", "alpha", "beta"])
    parser.add_argument("--key", type=Path, default=Path.home() / ".config/bipagents/play-publisher.json")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--notes", default="", help="what's new, shown to testers")
    parser.add_argument("--lang", default="en-US", help="the notes' language: one of the app's store listing languages")
    args = parser.parse_args()
    if not args.key.exists():
        sys.exit(f"No publishing key at {args.key} (see this script's help).")

    if not args.no_build:
        env = dict(os.environ, JAVA_HOME=os.environ.get("JAVA_HOME", JBR))
        code = time.strftime("%y%j%H%M")
        subprocess.run(["./gradlew", ":app:bundleRelease", "-q", f"-PversionCode={int(code)}"], cwd=ANDROID, env=env, check=True)
    if not BUNDLE.exists():
        sys.exit(f"No bundle at {BUNDLE}.")

    headers = {"Authorization": f"Bearer {access_token(args.key)}"}
    with httpx.Client(headers=headers, timeout=httpx.Timeout(60, write=600)) as http:
        edit = check(retried(lambda: http.post(f"{API}/edits")))["id"]
        print(f"Uploading {BUNDLE.name} ({BUNDLE.stat().st_size // 1_000_000} MB)…")
        bundle = check(retried(lambda: http.post(f"{UPLOAD}/edits/{edit}/bundles", params={"uploadType": "media"},
                                                 content=BUNDLE.read_bytes(), headers={"Content-Type": "application/octet-stream"})))
        code = bundle["versionCode"]
        release = {"versionCodes": [str(code)], "status": "completed"}
        if args.notes:
            release["releaseNotes"] = [{"language": args.lang, "text": args.notes[:500]}]
        check(retried(lambda: http.put(f"{API}/edits/{edit}/tracks/{args.track}", json={"track": args.track, "releases": [release]})))
        check(retried(lambda: http.post(f"{API}/edits/{edit}:commit")))
    print(f"Version {code} is on the « {args.track} » track: testers get it from the Play Store in a few minutes.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
