#!/usr/bin/env bash
# Runs `video-box bench` on the headset over SMB and prints one summary line.
# Usage: VIDEO_BOX_SMB_PASSWORD=... scripts/frame-bench.sh <smb-url> [bench args...]
# The password is passed through ssh stdin, never on a command line.
set -euo pipefail
url=$1; shift
printf '%s\n' "${VIDEO_BOX_SMB_PASSWORD:?set VIDEO_BOX_SMB_PASSWORD}" |
  bash "$(dirname "$0")/frame-ssh.sh" "read -r P; export VIDEO_BOX_SMB_PASSWORD=\$P; ${FRAME_PREFIX:-} ~/video-box/bin/video-box bench '$url' $*" |
  jq -r '"\(.decode.decoder)\thw=\(.decode.hardware_frames) sw=\(.decode.software_frames)\t\(.decode.frames_per_second*10|round/10) fps\t\(.realtime_factor*100|round/100)x realtime\t\(.decode.error // "")"'
