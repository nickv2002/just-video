#!/usr/bin/env bash
# Soak test of the real player on the headset with nobody wearing it: `play` with
# JUST_VIDEO_RENDER_UNSEEN (the real frame loop and renderer), an optional timed
# script of pause / jumps / A-B loop / headset off+on (JUST_VIDEO_SCRIPT, see
# src/xr/script.rs), optional Wi-Fi trouble, and a sampler of RSS, threads and open
# files. Prints a summary of the `Timing:` log lines (scripts/soak-summary.py).
#
# Usage: scripts/frame-soak.sh <smb-url> <seconds> [--start S] [--script "60:pause 65:pause ..."]
#                              [--binary PATH-ON-HEADSET] [--tag NAME]
#                              [--netem "0:delay 40ms 20ms loss 1%;120:loss 100%;130:delay 0ms"]
# The server's password is the one saved on the headset. Runs under flock on the
# shared headset lock, for `seconds` of wall-clock time (the player is stopped then).
#
# --netem: `SECONDS:PARAMS` steps (tc netem parameters) applied from that second of
# the run on, only to packets the headset sends to the server (a prio qdisc on the
# Wi-Fi interface with a filter on the server's address, so the ssh session stays
# alone; the headset kernel has no ifb, so the server's packets can't be shaped; the
# delay and loss hit the requests and the ACKs). `loss 100%` is an outage. Needs the
# headset sudo password in FRAME_SUDO_PASSWORD; it is kept in a mode-600 file on the
# headset for the run and deleted, and the qdisc is always removed.
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
cleanup() {
  if [ -n "$netem" ]; then
    ssh 'sudo -S -p "" tc qdisc del dev wlan0 root <$XDG_RUNTIME_DIR/.jv-sp 2>/dev/null; rm -f $XDG_RUNTIME_DIR/.jv-sp' || true
  fi
  ssh 'pkill -x just-video 2>/dev/null; wpctl set-mute @DEFAULT_AUDIO_SINK@ 0' || true
}
trap cleanup EXIT
exec 9>/tmp/headset.lock
flock 9
ssh 'wpctl set-mute @DEFAULT_AUDIO_SINK@ 1; pkill -x just-video; sleep 1; wpctl get-volume @DEFAULT_AUDIO_SINK@' | grep -q MUTED
nas=""
if [ -n "$netem" ]; then
  host=${url#smb://}; host=${host#*@}; host=${host%%[/:]*}
  nas=$(getent ahostsv4 "$host" | awk 'NR==1{print $1}')
  [ -n "$nas" ] || { echo "can't resolve $host" >&2; exit 1; }
  printf '%s\n' "${FRAME_SUDO_PASSWORD:?FRAME_SUDO_PASSWORD}" |
    ssh 'umask 077; cat >$XDG_RUNTIME_DIR/.jv-sp'
fi
# The remote part: start play, run the netem steps, sample its process every 10 s,
# wait for it to end.
ssh "SECS=$secs START=$start SCRIPT='$script' URL='$url' BIN=$binary NETEM='$netem' NAS='$nas' bash -s" >"$out/$tag.samples" <<'REMOTE'
log=$HOME/jv-test/soak.log
sp=$XDG_RUNTIME_DIR/.jv-sp
root() { sudo -S -p '' "$@" <"$sp"; }
if [ -n "$NETEM" ]; then
  root tc qdisc replace dev wlan0 root handle 1: prio bands 3 priomap 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  root tc qdisc add dev wlan0 parent 1:3 handle 30: netem delay 0ms
  root tc filter add dev wlan0 parent 1: protocol ip prio 1 u32 match ip dst "$NAS/32" flowid 1:3
fi
rm -f "$log.netem"
IFS=';' read -ra steps <<<"$NETEM"
(
  t0=$(date +%s)
  for step in "${steps[@]}"; do
    at=${step%%:*}; params=${step#*:}
    [ "$params" = clear ] && params="delay 0ms"
    while [ $(( $(date +%s) - t0 )) -lt "$at" ]; do sleep 0.5; done
    root tc qdisc change dev wlan0 parent 1:3 handle 30: netem $params
    echo "netem $at s: $params" >>"$log.netem"
  done
) >/dev/null 2>&1 &
stepper=$!
JUST_VIDEO_RENDER_UNSEEN=1 JUST_VIDEO_SCRIPT=$SCRIPT timeout -s INT "$SECS" \
  "$BIN" play "$URL" --start "$START" --duration 1000000 >/dev/null 2>"$log" &
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
kill "$stepper" 2>/dev/null
cat "$log.netem" 2>/dev/null
echo "---LOG---"
cat "$log"
REMOTE
python3 "$here/soak-summary.py" "$out/$tag.samples" | tee "$out/$tag.summary"
