#!/usr/bin/env bash
# Soak test of the real player on the headset with nobody wearing it: `play` with
# JUST_VIDEO_RENDER_UNSEEN (the real frame loop and renderer), an optional timed
# script of pause / jumps / A-B loop / headset off+on (JUST_VIDEO_SCRIPT, see
# src/xr/script.rs), and a sampler of RSS, threads and open files. Prints a summary
# of the `Timing:` log lines (scripts/soak-summary.py).
#
# Usage: scripts/frame-soak.sh <smb-url> <seconds> [--start S] [--script "60:pause 65:pause ..."]
#                              [--binary PATH-ON-HEADSET] [--tag NAME] [--netem "delay 40ms loss 2%"]
# The server's password is the one saved on the headset. Runs under flock on the
# shared headset lock. --netem needs the headset sudo password in FRAME_SUDO_PASSWORD;
# it is applied to the headset's Wi-Fi interface for the whole run and always removed.
set -euo pipefail
here=$(dirname "$0")
url=$1 secs=$2; shift 2
start=0 script="" binary='$HOME/jv-test/soak/just-video' tag=soak netem=""
while [ $# -gt 0 ]; do
  case $1 in
    --start) start=$2 ;; --script) script=$2 ;; --binary) binary=$2 ;;
    --tag) tag=$2 ;; --netem) netem=$2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift 2
done
out=${SOAK_OUT:-/tmp/frame-soak}
mkdir -p "$out"
ssh() { bash "$here/frame-ssh.sh" "$@"; }
dev=""
cleanup() {
  if [ -n "$netem" ] && [ -n "$dev" ]; then
    printf '%s\n' "${FRAME_SUDO_PASSWORD:-}" | ssh "sudo -S -p '' tc qdisc del dev $dev root 2>/dev/null || true"
  fi
  ssh 'pkill -x just-video 2>/dev/null; wpctl set-mute @DEFAULT_AUDIO_SINK@ 0' || true
}
trap cleanup EXIT
exec 9>/tmp/headset.lock
flock 9
ssh 'wpctl set-mute @DEFAULT_AUDIO_SINK@ 1; pkill -x just-video; sleep 1; wpctl get-volume @DEFAULT_AUDIO_SINK@' | grep -q MUTED
if [ -n "$netem" ]; then
  dev=$(ssh "ip route show default | awk '{for(i=1;i<NF;i++) if(\$i==\"dev\") {print \$(i+1); exit}}'")
  printf '%s\n' "${FRAME_SUDO_PASSWORD:?FRAME_SUDO_PASSWORD}" |
    ssh "sudo -S -p '' tc qdisc replace dev $dev root netem $netem"
  echo "netem on $dev: $netem" >&2
fi
# The remote part: start play, sample its process every 10 s, wait for it to end.
ssh "SECS=$secs START=$start SCRIPT='$script' URL='$url' BIN=$binary bash -s" >"$out/$tag.samples" <<'REMOTE'
log=$HOME/jv-test/soak.log
JUST_VIDEO_RENDER_UNSEEN=1 JUST_VIDEO_SCRIPT=$SCRIPT timeout -s INT $((SECS + 30)) \
  "$BIN" play "$URL" --start "$START" --duration "$SECS" >/dev/null 2>"$log" &
sleep 1
pid=$(pgrep -x just-video | head -1)
t0=$(date +%s)
while kill -0 "$pid" 2>/dev/null; do
  rss=$(awk '/VmRSS/{print $2}' /proc/$pid/status 2>/dev/null)
  thr=$(awk '/Threads/{print $2}' /proc/$pid/status 2>/dev/null)
  fds=$(ls /proc/$pid/fd 2>/dev/null | wc -l)
  echo "sample $(( $(date +%s) - t0 )) rss_kib=$rss threads=$thr fds=$fds"
  sleep 10
done
echo "---LOG---"
cat "$log"
REMOTE
python3 "$here/soak-summary.py" "$out/$tag.samples" | tee "$out/$tag.summary"
