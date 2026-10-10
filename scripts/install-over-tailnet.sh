#!/usr/bin/env bash
# Install the development build on your registered iPhone from anywhere, over your tailnet (no cable, no same Wi-Fi):
# builds and signs it, serves it over HTTPS on this Mac's tailnet address only, prints the link to open in Safari on
# the iPhone (Tailscale on), and stops serving after the first install or after 15 minutes.
#   scripts/install-over-tailnet.sh            # port 8447
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${PORT:-8447}"
TS="$(command -v tailscale || echo /Applications/Tailscale.app/Contents/MacOS/Tailscale)"
NAME="$("$TS" status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
TEAM="$(sed -n 's/^DEVELOPMENT_TEAM *= *//p' Config/Local.xcconfig)"
HOST="https://$NAME:$PORT"
OUT="$(mktemp -d)"
trap '"$TS" serve --https="$PORT" off >/dev/null 2>&1 || true; kill "${SERVER:-0}" 2>/dev/null || true; rm -rf "$OUT"' EXIT

echo "Building…"
xcodebuild archive -project BipAgents.xcodeproj -scheme BipAgents -configuration Debug -destination 'generic/platform=iOS' \
  -archivePath "$OUT/app.xcarchive" -allowProvisioningUpdates -quiet
cat > "$OUT/options.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>debugging</string><key>teamID</key><string>$TEAM</string>
  <key>signingStyle</key><string>automatic</string><key>thinning</key><string>&lt;none&gt;</string>
  <key>manifest</key><dict><key>appURL</key><string>$HOST/BipAgents.ipa</string>
    <key>displayImageURL</key><string>$HOST/icon.png</string><key>fullSizeImageURL</key><string>$HOST/icon.png</string></dict>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$OUT/app.xcarchive" -exportPath "$OUT/site" -exportOptionsPlist "$OUT/options.plist" \
  -allowProvisioningUpdates -quiet
ICON="$(find "$OUT/app.xcarchive" -name 'AppIcon60x60@2x.png' | head -1)"
sips -z 512 512 "$ICON" --out "$OUT/site/icon.png" >/dev/null
cat > "$OUT/site/index.html" <<HTML
<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Install BipAgents</title>
<body style="font-family:-apple-system;background:#f2f3f7;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px">
<div style="background:#fff;border-radius:28px;padding:32px 24px;max-width:360px;text-align:center">
<img src="icon.png" width="96" style="border-radius:22px"><h1>BipAgents</h1>
<a style="display:block;margin-top:24px;background:#16181d;color:#fff;text-decoration:none;padding:16px;border-radius:999px;font-weight:700"
 href="itms-services://?action=download-manifest&amp;url=$HOST/manifest.plist">Install</a>
<p style="color:#5b6070">Then go to the Home Screen: the app updates in place, your agents stay.</p></div>
HTML

(cd "$OUT/site" && python3 -m http.server "$((PORT + 1))" --bind 127.0.0.1 >"$OUT/access.log" 2>&1) & SERVER=$!
"$TS" serve --bg --https="$PORT" "http://127.0.0.1:$((PORT + 1))" >/dev/null
echo
echo "Open in Safari on the iPhone (Tailscale on):  $HOST/"
echo "Waiting for the install (15 min max)…"
for _ in $(seq 1 180); do
  if grep -q '"GET /BipAgents.ipa' "$OUT/access.log" 2>/dev/null; then sleep 20; echo "Downloaded by the iPhone. Done."; exit 0; fi
  sleep 5
done
echo "Nobody installed it; stopped serving."
