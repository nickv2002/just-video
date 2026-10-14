#!/usr/bin/env bash
# Installs the Steam Frame build into ~/Applications/VideoBox on the headset
# and adds "Video Box" to the Steam library as a non-Steam game (first run only).
# Build first: scripts/build-frame-media.sh (once), then scripts/build-frame.sh.
# The entry must be marked as a VR app, or SteamVR keeps its own menu in front
# and the app starts "in the background". Steam only reads that flag at startup
# and rewrites the file while running, so setting it needs one Steam restart:
# FRAME_RESTART_STEAM=1 does that (the headset's interface restarts briefly).
# Library art (assets/steam, made by `video-box steam-art`) goes into each
# Steam user's config/grid on every run; the shortcut's icon field is only
# edited while Steam is stopped, for the same reason as the VR flag.
# A SteamVR manifest (videobox.vrmanifest) is registered too: it gives the
# "Now Playing" panel its picture, which Steam's own manifest entry cannot.
set -euo pipefail
cd "$(dirname "$0")/.."
binary=target/aarch64-unknown-linux-gnu/release/video-box
[ -x "$binary" ] || { echo "Build first: scripts/build-frame.sh" >&2; exit 1; }
host=steamos@${FRAME_HOST:-frame.local}
ssh_opts=(-i "$HOME/.ssh/steam_frame_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes)
# Best effort, once: the app used to be "Just Video" (~/Applications/JustVideo).
# Remove that install and its SteamVR manifest registration. Its Steam library
# entry (shortcuts.vdf, which Steam rewrites while running) is left for the user
# to remove from the Steam UI; it points at the deleted launcher.
ssh "${ssh_opts[@]}" "$host" 'rm -rf ~/Applications/JustVideo; python3 - <<"PY" || true
import json, os
path = os.path.expanduser("~/.config/openvr/config/appconfig.json")
conf = json.load(open(path))
old = os.path.expanduser("~/Applications/JustVideo/justvideo.vrmanifest")
conf["manifest_paths"] = [p for p in conf.get("manifest_paths", []) if p != old]
json.dump(conf, open(path, "w"), indent=3)
PY' || true
# No copy of the old binary is kept: to roll back, build and install an older commit.
ssh "${ssh_opts[@]}" "$host" 'mkdir -p ~/Applications/VideoBox'
rsync -a -e "ssh ${ssh_opts[*]}" "$binary" "$host:Applications/VideoBox/video-box"
rsync -a -e "ssh ${ssh_opts[*]}" scripts/steam-shortcut.py "$host:Applications/VideoBox/"
rsync -a --delete -e "ssh ${ssh_opts[*]}" assets/steam/ "$host:Applications/VideoBox/art/"
ssh "${ssh_opts[@]}" "$host" "RESTART_STEAM=${FRAME_RESTART_STEAM:-0} bash -s" <<'REMOTE'
set -euo pipefail
dir=$HOME/Applications/VideoBox
launcher="$dir/Video Box"
# Steam names the library entry after this file. Messages go to video-box.log
# (the previous run's to video-box.log.1): Steam doesn't keep them.
cat > "$launcher" <<'SH'
#!/bin/sh
dir=$(dirname "$0")
[ -f "$dir/video-box.log" ] && mv -f "$dir/video-box.log" "$dir/video-box.log.1"
exec "$dir/video-box" "$@" 2>>"$dir/video-box.log"
SH
chmod +x "$launcher" "$dir/video-box"
if grep -rqs --text "Applications/VideoBox/Video Box" ~/.local/share/Steam/userdata/*/config/shortcuts.vdf; then
    echo "Updated Video Box (already in the Steam library)."
else
    steamos-add-to-steam "$launcher"
    echo "Added Video Box to the Steam library (Non-Steam)."
fi

# Mark the entry as a VR app (OpenVR = 1). Prints the value it found.
openvr() {
    python3 - "$1" "$2" <<'PY'
import glob, os, sys
mode, marker = sys.argv[1], b"Applications/VideoBox/Video Box"
for path in glob.glob(os.path.expanduser("~/.local/share/Steam/userdata/*/config/shortcuts.vdf")):
    data = bytearray(open(path, "rb").read())
    start = data.find(marker)
    flag = data.find(b"\x02OpenVR\x00", start)
    if start < 0 or flag < 0 or flag - start > 2000:
        continue
    at = flag + len(b"\x02OpenVR\x00")
    print(int.from_bytes(data[at:at + 4], "little"))
    if mode == "set":
        data[at:at + 4] = (1).to_bytes(4, "little")
        open(path, "wb").write(data)
PY
}
# Library art: copies it into config/grid; with --set-icon also sets the icon.
art() {
    python3 "$dir/steam-shortcut.py" "$dir/art" "$@" ||
        echo "Warning: couldn't install the Steam library art." >&2
}
if [ "$(openvr check x | sort -u)" = 1 ] && python3 "$dir/steam-shortcut.py" "$dir/art" --icon-ok; then
    :
elif [ "$RESTART_STEAM" = 1 ]; then
    echo "Restarting Steam to mark Video Box as a VR app and set its icon…"
    steam -shutdown >/dev/null 2>&1 || true
    for _ in $(seq 60); do pgrep -x steam >/dev/null || break; sleep 1; done
    openvr set x >/dev/null
    pgrep -x steam >/dev/null || art --set-icon >/dev/null
    echo "Done. Steam restarts on its own; if it doesn't, restart the headset."
elif [ "$(openvr check x | sort -u)" != 1 ]; then
    echo "Note: Video Box isn't marked as a VR app yet, so it starts in the background."
    echo "      Run again with FRAME_RESTART_STEAM=1 to fix (restarts Steam once)."
fi
if pgrep -x steam >/dev/null; then
    art
    echo "Steam may need a restart to show new library art (FRAME_RESTART_STEAM=1 also sets its icon)."
else
    art --set-icon
fi
REMOTE
