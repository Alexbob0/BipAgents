#!/bin/sh
# The demo page's clips (site/src/bipagents/demo/): the real app on an iPhone simulator, playing its own demo.
#
#   site/demo-clips.sh record <simulator udid> [scenario…]   # raw recordings into site/demo-raw/ (not for git: delete it when done)
#   site/demo-clips.sh encode <scenario>-<lang> <start s> <end s> [speed]   # one raw clip → demo/<name>.mp4 + .jpg (needs ffmpeg)
#
# record: a Debug build of the app (it has `-demoRecord`, App/Features/Demo/DemoRecording.swift) installed on a booted
# simulator; each scenario of App/Resources/Demo/demo.json is typed, played and its cards answered by the app itself
# (first choice, approvals accepted), in the four languages; the status bar is set to 9:41, light appearance.
# The app must have the demo greeting's fix (its bubble written to the end, then the request typed): without it the
# greeting stops at its first words until the request is sent (the 10/11 clips were made with that fix applied).
# The Lisbon weekend's list is a web page: recordVideo alone filmed it blank; with `simctl io screenshot` taken in a
# loop during the recording (every ~0.4 s from 12 s on) the recording shows it. Its clip ends on that list (1.4×).
# On iOS, take a simulator slot first (/private/tmp/claude-501/ios-slot.sh on the owner's Mac).
# encode: 590 px wide, 30 fps, H.264 without sound, faststart; the last frame held 1.5 s so the answer can be read
# before the loop; the poster is that last frame. Speed > 1 shortens a long one (the Lisbon weekend: 1.25).
# The clips the site shows are build.py's CLIPS (one per Bip of « Build your team »).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
raw=$here/demo-raw
out=$here/src/bipagents/demo
app=com.kerfystudio.bipagents
case ${1:-} in
record)
    udid=$2; shift 2
    scenarios=${*:-book_table sleep_week subscriptions dinner weekend_lisbon}
    mkdir -p "$raw"
    xcrun simctl status_bar "$udid" override --time 9:41 --batteryState charged --batteryLevel 100 --cellularBars 4 --wifiBars 3
    xcrun simctl ui "$udid" appearance light
    for lang in fr en de es; do
        case $lang in fr) loc=fr_FR;; en) loc=en_US;; de) loc=de_DE;; es) loc=es_ES;; esac
        for s in $scenarios; do
            xcrun simctl terminate "$udid" $app >/dev/null 2>&1 || true
            sleep 1
            xcrun simctl io "$udid" recordVideo --codec=h264 --force "$raw/$s-$lang.mp4" >/dev/null 2>&1 &
            rec=$!
            sleep 1
            xcrun simctl launch "$udid" $app -demoRecord "$s" -AppleLanguages "($lang)" -AppleLocale $loc >/dev/null
            sleep 45   # the longest (the Lisbon weekend) ends in about 35 s; the recording stops at the last change
            kill -INT $rec; wait $rec || true
            echo "$raw/$s-$lang.mp4"
        done
    done ;;
encode)
    name=$2; ss=$3; to=$4; speed=${5:-1}
    mkdir -p "$out"
    ffmpeg -v error -y -ss "$ss" -to "$to" -i "$raw/$name.mp4" -an \
        -vf "setpts=PTS/$speed,fps=30,scale=590:-2:flags=lanczos,tpad=stop_mode=clone:stop_duration=1.5,format=yuv420p" \
        -c:v libx264 -preset veryslow -crf 30 -profile:v high -level 4.0 -tune animation -movflags +faststart "$out/$name.mp4"
    ffmpeg -v error -y -sseof -0.1 -i "$out/$name.mp4" -frames:v 1 -q:v 5 "$out/$name.jpg"
    ls -l "$out/$name.mp4" "$out/$name.jpg" ;;
*)
    sed -n '2,15p' "$0"; exit 1 ;;
esac
