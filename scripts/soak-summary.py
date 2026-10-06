#!/usr/bin/env python3
"""Summarises a scripts/frame-soak.sh run: its sampler lines and the player's `Timing:` log."""
import re
import statistics as st
import sys

text = open(sys.argv[1], errors="replace").read()
samples_text, _, log = text.partition("---LOG---")
samples = [
    tuple(int(v) for v in re.findall(r"(\d+)", line.split(" ", 2)[2]))
    for line in samples_text.splitlines()
    if line.startswith("sample") and "rss_kib=" in line and "rss_kib= " not in line
]
lines = log.splitlines()


def total(pattern, group=1):
    return sum(float(m.group(group)) for l in lines if (m := re.search(pattern, l)))


def worst(pattern):
    values = [float(m.group(1)) for l in lines if (m := re.search(pattern, l))]
    return max(values, default=0)


def count(pattern):
    return sum(1 for l in lines if re.search(pattern, l))


frames = [l for l in lines if l.startswith("Timing: frames")]
print(f"windows            {len(frames)}")
print(f"pictures           uploaded {total(r'(\d+) pictures uploaded'):.0f}, skipped {total(r'(\d+) skipped'):.0f}, "
      f"display periods missed {total(r'(\d+) display periods missed,'):.0f}")
waits = [float(m.group(1)) for l in frames if (m := re.search(r"([\d.]+) ms longest", l))]
print(f"read wait          longest {max(waits, default=0):.0f} ms, windows with a wait >= 150 ms: "
      f"{sum(w >= 150 for w in waits)}, total {total(r'read wait ([\d.]+) ms total'):.0f} ms")
print(f"decode max         {worst(r'decode [\d.]+/([\d.]+) ms'):.1f} ms; queue depth min over windows "
      f"{min((int(m.group(1)) for l in frames if (m := re.search(r'queue depth min (\d+)', l))), default='-')}")
ahead = [int(m.group(1)) for l in frames if (m := re.search(r"buffered ahead min (\d+)", l))]
aim = [int(m.group(1)) for l in frames if (m := re.search(r"MiB of (\d+) aimed", l))]
if ahead:
    print(f"buffer             ahead min {min(ahead)} MiB (median of mins {st.median(ahead):.0f}); aim {min(aim)}-{max(aim)} MiB")
req = worst(r"took \d+/\d+/(\d+) ms")
print(f"request time max   {req:.0f} ms")
seeks = [float(m.group(1)) for l in lines if (m := re.search(r"Timing: seek .* ready in (\d+) ms", l))]
if seeks:
    print(f"seeks              {len(seeks)}, ready in median {st.median(seeks):.0f} ms, worst {max(seeks):.0f} ms")
print(f"decoder events     failed {count(r'Decoder failed')}, replaced {count(r'replaced in')}, "
      f"CPU fallback {count(r'Playing on the CPU')}, still held {count(r'still held')}, "
      f"nothing decoded {count(r'nothing decoded')}, no picture {count(r'no picture for')}, "
      f"waiting while stopped {count(r'frame loop is stopped')}, seek failed {count(r'Seek to .* failed')}, "
      f"replace failed {count(r'Replacing the decoder')}")
print(f"other              errors {count(r'(?i)^error|panick|SIGSEGV')}, audio nearly dry windows "
      f"{count(r'[1-9]\d* nearly dry')}, script actions {count(r'^Script:')}")
if samples:
    rss = [s[0] / 1024 for s in samples]
    # The first minute is the buffer filling; growth is judged from minute 2 on.
    late = [s[0] / 1024 for s in samples[6:]] or rss
    slope = (late[-1] - late[0]) / max(len(late) - 1, 1) * 6 if len(late) > 1 else 0
    print(f"RSS                start {rss[0]:.0f} MiB, end {rss[-1]:.0f}, max {max(rss):.0f}; "
          f"growth after the first minute {slope:+.1f} MiB/min")
    print(f"threads            {min(s[1] for s in samples)}-{max(s[1] for s in samples)}; "
          f"fds {min(s[2] for s in samples)}-{max(s[2] for s in samples)}")
bad = [l for l in lines if re.search(r"(?i)fail|error|panic|CPU|nothing decoded|held|Script:", l)
       and not l.startswith("Timing:")]
for l in bad[:25]:
    print("  >", l[:200])
