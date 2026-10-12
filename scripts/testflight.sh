#!/bin/bash
# Archives the app (with its notification and widget extensions) and uploads it to App Store Connect for TestFlight.
# The build number is the date and time, so every upload is a new build of the same version (MARKETING_VERSION).
#   scripts/testflight.sh          archive and upload
#   scripts/testflight.sh export   archive and export the .ipa only (build/export-<build>), to check the signing
# Needs: Config/Local.xcconfig with DEVELOPMENT_TEAM, Xcode signed in to that team, and the app created in App Store
# Connect with the same bundle ID.
set -euo pipefail
cd "$(dirname "$0")/.."

destination="${1:-upload}"
team=$(sed -n 's/^DEVELOPMENT_TEAM *= *//p' Config/Local.xcconfig)
build=$(date +%Y%m%d.%H%M)
archive="build/BipAgents-$build.xcarchive"
options=$(mktemp -t exportoptions).plist

xcodebuild -project BipAgents.xcodeproj -scheme BipAgents -configuration Release -destination 'generic/platform=iOS' \
    -archivePath "$archive" -allowProvisioningUpdates CURRENT_PROJECT_VERSION="$build" archive | grep -E "error:|warning: .*sign|ARCHIVE (SUCCEEDED|FAILED)"

cat > "$options" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key><string>app-store-connect</string>
    <key>destination</key><string>$destination</string>
    <key>signingStyle</key><string>automatic</string>
    <key>teamID</key><string>$team</string>
    <key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
PLIST

xcodebuild -exportArchive -archivePath "$archive" -exportOptionsPlist "$options" -exportPath "build/export-$build" \
    -allowProvisioningUpdates | grep -E "error|Upload succeeded|EXPORT (SUCCEEDED|FAILED)"
echo "Build $build ($destination)."
