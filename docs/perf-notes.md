# Performance notes (night/perf)

Measured on the Frame over home Wi-Fi unless a line says PC. Test files are described by
format. Wi-Fi noise is large, so each comparison alternates old and new builds and quotes
medians or every run. Tools: `just-video bench-seek`, `bench-thumbnails`, `library-stress`,
`ls`, the `Timing:` log lines, and the ignored test `render_time`
(`cargo test --release --lib -- --ignored --nocapture render_time`).

## Kept

| Change | Before | After |
|---|---|---|
| Thumbnails: 3 maker threads (`JUST_VIDEO_THUMB_WORKERS`) | 36 videos, cold: 12.98 s (1 worker) | 10.3 s (11.3 s with 2) |
| SMB: remember a name lookup (10 min), look saved servers up at start | cold connect + listing: 1159 ms | 431 ms |
| Open: subtitle lookup can't hold the video back (0.5 s grace) | a stalled listing delayed the first frame by 15 s (seen once) | at most +0.5 s |
| Browser repaint: rounded rectangles write their interior directly | 9.5-10.9 ms per repaint (30 rows with thumbnails, Frame) | 0.7-1.9 ms; output bit-identical |
| `mallopt(M_TRIM_THRESHOLD)` at start (`JUST_VIDEO_NO_TRIM=1` turns it off) | thumbnails: 257-286 MB resident after, 287-330 MB peak | 30-32 MB after, 61-75 MB peak |
| Browser repaints into the previous canvas | with trimming on, a fresh 6.4 MB canvas per repaint cost 2.0-3.0 ms (page faults) | 0.65-1.7 ms, same as without trimming |

Trimming makes every allocation of 128 KiB or more a fresh mapping, so a per-frame buffer of
that size must be reused (the browser canvas is; the controls panel, 1.2 MB, repaints about once
a second and was left alone). 8K software playback showed no difference with or without it.

## Findings worth knowing

- **mDNS is the slowest part of connecting.** On the Frame a `.local` lookup takes 350-600 ms
  and sometimes fails for minutes (`Invalid address`), while a connect by IP address takes
  59 ms (`bench-seek` showed `connect 2007 ms` for two sequential connects by name against
  59 ms by IP). Saving the server by IP address, or by a name unicast DNS knows, avoids it.
  Using a remembered address *before* the lookup answers would also hide it, but a stale
  address would send the sign-in to whichever machine now has it, so it was not done.
- Playback is fine: 6K60 HEVC hardware at 72 Hz, 899 of 899 frames, 0 behind, copy 2.2 ms
  mean; 4K HEVC on the CPU at 60 Hz, 300 of 300 frames.
- Thumbnails read ~2.5 MiB per video (the index, then one keyframe), 4 jumps on average,
  waiting 400-1000 ms in round trips. Total traffic is ~70 Mbit/s, below the link's ceiling,
  so the cost is latency, not bandwidth. A thumbnail that reused the index bytes of the
  folder probe would save most of it, but the probe cache keeps facts, not bytes.
- Every jump in an HEVC 6K file after the first ends with `Decoder failed (... error -12)`
  and a 50-70 ms decoder replacement (the driver refuses the restart after a flush). Jump
  times are 130-310 ms, of which 70-190 ms is the network wait.

## Dead ends

- **Replace the hardware HEVC decoder up front at every jump** instead of after the failed
  flush: jumps 271 / 270 ms median against 248 / 262 ms for the existing path (open was also
  slower). The failed attempt costs about what replacing first does.
- **More than 3 thumbnail makers:** 5 and 8 were not faster (8.1-13.7 s, noisy) and held
  340-540 MB resident.
- **Short `analyzeduration` when opening a video for its thumbnail:** same total time, same
  per-video open time (270 ms median either way).
- **Caching glyph coverage without cloning it per drawn character:** no measurable change
  in a repaint (4.60 against 4.56 ms before the rectangle fix, 1.40 against 1.39 after), not kept.

## 8K stutter investigation (2026-10-05)

Report: a 1080p H.264 flat file plays perfectly; the 8K (7680x3840, 60 fps, 8-bit HEVC) 180°
side-by-side file and a 6K HEVC side-by-side file, both on the hardware decoder, show 70-180
pictures uploaded and 60-190 skipped per 5 s window. Rendering never missed a display period.

**What the headset log (build ae02651) shows, without any headset test:**

- 29 windows of 6K/8K playback, 23 with skips. Pictures delivered per second (uploaded + skipped)
  had a median of 58 (range 8.6-73): the decoder is bursty, not uniformly slow. Pictures actually
  shown, median 32 per second at a 60 Hz display of 60 fps video.
- Smooth windows (302 uploaded, 0 skipped, copy ahead ~3 ms) came after 30-40 s of playing the same
  file; the first minute of every play was the worst. Bad windows have copy ahead 0.0: the next
  picture was never ready between frames.
- The worst window (14 pictures, 69 audio writes, picture-vs-sound gap 2.4 s) is in the 6K file,
  not the 8K one. The decode thread also decodes the sound, so it was blocked for several seconds
  in one call: in the demuxer's read (the 10 s "Read-ahead: waited" message was not reached) or in
  the V4L2 driver. The log could not tell which; the new logging does (below).
- Of tonight's changes, none touches the playback path except `mallopt(M_TRIM_THRESHOLD)`
  (`git diff be8ad10 ae02651` has no change in `player.rs`, `readahead.rs` or `native/`; `media.rs`
  differs in a log line, `smb.rs` in the name cache used at connect, `app.rs` in the A-B loop,
  autoplay and the browser canvas, which does not run while playing). Thumbnail workers stop when
  a video opens. So `JUST_VIDEO_NO_TRIM=1` is the one comparison that can matter; the
  per-allocation cost it adds (a 1 MiB read block and each large packet is mapped fresh) was
  never measured with the hardware decoder.

**New logging (commit "Timing log: read wait, bytes, decode time and queue depth"):** each
`Timing: frames …` line ends with `decoder side: N pictures decoded, decode MEAN/MAX ms, read X MiB,
read wait TOTAL ms total / MAX ms longest, queue depth min A mean B`. Decode time is the decoder
call minus the time blocked on reads. Queue depth is the decoded pictures waiting (0-4) as the
frame loop saw them. Reading it: a long `read wait` max with decode ~ normal is the network;
decode max high with read wait ~0 is the decoder or driver; queue depth min 0 with a low decoded
count is the decoder not keeping ahead.

**Headset runs (app stopped first, headset muted, iris `CORE_INIT`, hardware decoder every time):**

- `bench --frames 600 --hw-only --replay`, 8K HEVC side-by-side file, 3 runs: network + decoder
  105.9 / 106.3 / 105.8 fps; decoder alone from memory 106.2 / 106.2 / 105.7 fps. The decoder
  does 1.77x real time and the network is not the limit at the start of the file.
- Old-build comparison, `bench-seek --from 1038 --play 40 --hz 60` (2398 pictures due), 4 rounds
  alternating, median shown / skipped: ae02651 (installed before) 2395 / 2; be8ad10 (before the
  night work) 2396.5 / 0.5; 2c20a1e (the merge) 2394 / 3; newest (logging added) 2394 / 2.5; ae02651 with
  `JUST_VIDEO_NO_TRIM=1` 2396 / 1.5; newest with `JUST_VIDEO_NO_TRIM=1` 2396 / 1. Every build plays
  the file with at most 11 skipped pictures in 40 s and `behind_ticks` 0 (one run 12): no difference
  between them, so none of tonight's changes shows up here. Thumbnails cannot be switched off or
  on in `bench-seek` (the workers belong to the browser), so that case was not tested.
- `play` with `JUST_VIDEO_RENDER_UNSEEN=1` (the real frame loop and renderer, nobody in the headset,
  72 Hz) on the 8K file (50 s) and the 6K file (30 s): every window after the first has 300 of 300
  pictures, 0 skipped; decode 16.6 ms mean (the decoder waits for the frame loop to free a buffer, so it
  runs at the pace of playback), read wait under 5 ms, queue depth mean 1.0 (8K) and 4.0 (6K). The
  first window after a resume point has 22 skipped, read wait 1.0-1.4 s total (longest 200-260 ms) and
  queue depth min 0: the read-ahead window starting from scratch.
- Not reproduced. The stutter in the user's log needs a condition these runs lack: the headset being
  worn (compositor and SteamVR busy), the app started from the browser with thumbnail workers
  running before, or Steam's helper holding a decoder session. Next time it happens, the `decoder
  side` figures in the log tell network (read wait longest) from decoder (decode max, queue depth
  min 0 with little read wait).
- Found on the way: a video started from the command line (`play`) did not get the new figures (fixed).

## Decoder recovery (2026-10-05)

Report: playing the 8K HEVC side-by-side file on the iris hardware decoder, the log showed
`capture: nothing decoded in 500 ms (3 of 6 buffers queued in the driver, 3 held by the application)`
about 8 times, `Hardware decoder returned no picture for 3004 ms`, `Decoder: 3 frames still held;
replacing it anyway`, and then on every jump a run of `Decoder failed (... -12); replaced in ~350 ms`
(`output VIDIOC_REQBUFS failed`, `can't configure decoder`), ending on the CPU (`Playing on the CPU`)
and back on the hardware a moment later.

**Cause.** The old session lives as long as one of its frames does (FFmpeg's V4L2 buffers
hold the context), and iris refuses a second 8K session beside it (`current session not
supported`, -12). The player drops its frames (`current`, `next`, the queue) only inside
`Playback::advance`, which the frame loop does not call when the OpenXR session is not rendering
(`should_render` false, or no running session): both failure runs in the headset log start
right after `OpenXR session: VISIBLE` then `SYNCHRONIZED` (headset off), and `Playing on the CPU`
(printed from `advance`) only appears after `FOCUSED`. With the loop stopped, the 3 held pictures
left the 6-buffer 8K decoder too few to decode into, so it returned nothing; 3 s later the
replacement started, waited its 300 ms for frames nobody would drop (the 350 ms), opened a
second session next to the first, was refused, and tried again twice more, the last on the CPU.
The one replacement in the log that took 28 ms (a failed flush at a jump, while rendering) is the
ordinary kind and is unchanged.

**Reproduced headlessly:** `bench-seek --idle N` (new) stops ticking `advance` for N seconds before each jump.
With 6 s the old build printed the headset log's sequence exactly (8-11 `nothing decoded`, `no picture
for 3004 ms`, `3 frames still held`, `-12`, `REQBUFS failed`, `Playing on the CPU`). Plain
`bench-seek` can't show it: it ticks `advance` all the time, so frames are always released.

**Changes.**
- The frame loop calls `Playback::keep_flowing` when it isn't drawing (headset off: takes the
  pictures that fall due, as drawing would; no session: only lets frames go on request), so the
  decoder keeps its buffers and nothing is replaced.
- A `Pulse` beaten by the frame loop tells the decode thread whether the consumer runs.
  `replace_decoder` (now over a small `Session` trait, with a mock in the tests) waits for a silent
  frame loop (up to 30 s) instead of racing it, and does not free-behind while frames are held;
  a running loop that still holds frames after 300 ms is leaking them, as before.
- A stall (`DecoderStalled`, a typed error) with a silent frame loop and frames held is waited out
  (`judge_stall`), not counted as a dead decoder, so it no longer costs restarts or the CPU.
- A decoder that returns nothing mid-playback (a picture decoded since the jump, the clock
  running, the frame loop drawing) is judged dead after 1.5 s instead of 3 s. Time spent reading
  doesn't count (the limit is on the wait inside the decoder only), so a stalled network read isn't
  taken for a dead decoder. Right after a jump the limit stays 3 s.
- Tests: the old session closes before the new opens; a silent frame loop is waited for; leaked
  frames don't hang the replacement; stop ends the wait; the CPU is used only after the hardware
  failed each time.

**Measured** (headset, 8K HEVC side-by-side file on the hardware decoder, muted, iris `CORE_INIT`,
17 jumps per run, 3 runs each alternating old/new, `bench-seek --random 10 --from 600 --idle 8`;
the old build is this commit's parent plus the `--idle` option):

| | jump shown, median of run medians | `-12` per run | `frames still held` | on the CPU | run |
|---|---|---|---|---|---|
| old | 536 ms (527, 536, 582; worst jump 2.5-2.8 s) | 10 | 12 | 2 times, 4 software opens | fails (a jump without a picture) |
| new | 270.5 ms (272, 257, 271; worst 436-484 ms) | 0, 0, 1 | 0 | never | passes |

With `--idle 4` (stall near the end of the idle) the old build was luckier, 341 ms vs 288 ms,
and in one of three runs went to the CPU and then crashed (SIGSEGV after `Playing on the CPU`,
kernel WARN in `vb2_start_streaming`); not seen with the new build. The new build prints
`Decoder: no picture while the frame loop is stopped; waiting for it` 14-15 times per run and
resumes with no replacement. Without idling (4 alternating runs each, 17 jumps): 287 ms old, 271 ms
new, no `-12` except the ordinary one failed flush in one old run, no false stall; 40 s steady
play at 60 Hz: 2396-2397 of 2398 pictures shown on both.

Not measured: a real headset taken off and put on (the app change that makes the frame loop
keep taking pictures is in `app.rs` and only exercised by reasoning and the bench stand-in);
Steam's web helper holding a session (`/proc/*/fd` showed none holding the decoder during the runs).

## Read buffer (2026-10-05)

Question: the worn-headset log showed 15-34 MiB read per 5 s window (about 6 MiB/s) with 0.8-3.4 s
of read wait, while 32 MiB were meant to be in flight. Why was the throughput so low?

**What limits it (headset over Wi-Fi to the NAS, 8K HEVC side-by-side file, 300-400 MiB `read-bench`):**

- The 6 MiB/s was what the demuxer consumed while it was blocked, not what the link carries. Unworn,
  the link gave 125-230 Mbit/s (15-28 MiB/s); the file needs ~6 MiB/s on average (`play` at 1038 s
  reads 29-30 MiB per 5 s) and about 13 MiB/s in its busiest parts. Headroom is only 1.3-2x, so a
  slow second empties the buffer, and a fresh play fills it at (link minus playback) only.
- The window does reach 32 blocks and stays there while playing straight on; it restarts at 4 only
  after a jump. A decoder replacement reopens the decoder on the same reader (a seek back to a
  keyframe that is still in the kept blocks). So the depth, not the mechanism, was short: 32 MiB is
  3 s at 10 MiB/s, and at most 3 reads (3 MiB) were requested at a time.
- More reads in flight do not carry more: 3 / 8 / 16 in flight over 2 connections, 300 MiB, 2 rounds
  each: 130-142 / 143-144 / 133-150 Mbit/s, but the median request time went 164-197 / 459 /
  918-1049 ms (the reads queue on the link). Over 4 connections 3 / 4 / 6 / 8 in flight (4 rounds,
  medians): 201 / 217 / 202 / 209 Mbit/s. SMB credits (1024 backlog, 16 per 1 MiB read), the
  negotiated read size (Samba allows 8 MiB, the client asks 1 MiB) and the single decode thread are
  not limits.
- Each TCP connection is what is limited. Medians of 4 alternating rounds, 300 MiB, 3 in flight:
  2 connections 150 Mbit/s, 3: 186, 4: 202 (1 connection ~127 in two runs). On the server (`ss -ti`,
  read only) each connection to the headset had cwnd ~30 and ssthresh 23-32 (CUBIC backing off),
  delivery rate 70-85 Mbit/s, and 330-370 retransmitted segments within one 20 s run: loss on the
  Wi-Fi path holds each flow down. On the headset the SMB threads used 1-5 % CPU, rtt 4 ms, receive
  window ~1 MiB: neither the client CPU nor the receive window.

**Changes:**

- The playing video's buffer aims at 10 s of the stream (file size over duration) between 16 and
  160 MiB. A stall of 150 ms or more in steady reading grows the aim by half (up to 3x, the 160 MiB
  cap still holds); the growth halves back every 60 s without one. After a jump the window still
  starts at 4 blocks and doubles per block, with the old 3 reads in flight (4 at once after a jump
  made jumps ~10 % slower in one comparison, and nothing was gained).
- A playing video is read over 4 connections instead of 2 (+35 % over 2 in `read-bench`).
- The `decoder side:` log part gains `N requests took P50/P99/MAX ms, in flight mean X, buffered
  ahead min A mean B MiB of T aimed`. Reading it: buffered ahead near 0 with a long read wait is
  the link falling behind playback; ahead near the aim with a long request time is harmless.
- `read-bench` and `play` take `--max-in-flight N` (the old fixed policy), `--buffer-seconds`,
  `--buffer-mib` and `--buffer-in-flight`, to compare.
- Tests with a source whose requests 100-109 take 1 s: with the old policy the consumer waits
  ~0.6 s, with a 3 s buffer it never waits more than 100 ms. The memory cap (50 blocks held), the
  aim following the rate, the growth after stalls and the minimum for slow streams are tested too.

**Measured (headset, nobody wearing it, hardware decoder, muted; installed build against this one,
alternating, medians):**

- `read-bench` 400 MiB, 4 rounds: 114 -> 146 Mbit/s (installed 134 / 124 / 77 / 105, new 141 / 170 /
  114 / 151; another process was using the link in rounds 3 and 4).
- `bench-seek --from 1038 --play 40 --hz 60`, 4 rounds: shown 2395.5 -> 2396 of 2398 due, skipped
  1.5 -> 1, behind ticks 0 / 0. In its first round the installed build alone had a real stall (open
  1051 ms, 149 skipped, 615 behind ticks); the new build had none in its four. One event, not a result.
- `bench-seek` jumps (11 per run, sum of shown_ms; 10 installed runs, 6 new runs): 4369 -> 4239 ms,
  open 270 -> 291 ms, first frame 385 -> 410 ms: no change within the noise (a batch of 4 pairs on
  a busy link was left out: the installed open took up to 3.2 s in it).
- `play` with `JUST_VIDEO_RENDER_UNSEEN=1`, 60 s from 1038 s, 2 runs each: first 5 s window, skipped
  18 / 11 installed against 10 / 5 new, read wait 788 / 673 ms against 544 / 593 ms; every later
  window 300 of 300 shown with both. The new log shows the buffer reaching its 59 MiB aim after
  ~15 s (min buffered 0, 23, 50, 54, 56, 57 MiB in successive windows), request time 80-200 ms,
  max 733 ms once.
- `bench --replay` decodes from memory, so it does not touch the reader and was not repeated.
- Not reproduced: the worn-headset stutter itself (Wi-Fi spikes cannot be made on demand). The
  buffer is meant for it: at 6-13 MiB/s, 10 s is 60-130 MiB, against 3 s (32 MiB) before.
- Memory: `VmRSS` of `play` after 50 s: 1.23 GB with the new build, 1.17 GB installed, so +60 MiB
  at this file's 6 MiB/s (59 MiB aimed). At 13 MiB/s the aim is the 160 MiB cap. The blocks kept
  behind the reader (up to 192 MiB, unchanged) come on top. The headset had 11 GB available.
- The rate comes from the average bitrate; a variable-bitrate peak above it is what the growth
  after stalls is for.

**Server side (n5, read only `testparm -sv`, `ss`; nothing was changed):** Samba is not the limit:
`smb2 max credits = 8192`, `smb2 max read = 8 MiB`, `aio read size = 1`, `use sendfile = No`
(default), `socket options = TCP_NODELAY`; cold 1 MiB reads were 300+ MiB/s. What would help is the
TCP congestion control for the Wi-Fi path: `tcp_bbr` exists as a module but only reno and cubic
are available, and CUBIC cuts its window at every loss. Suggested (needs root, untested):
`modprobe tcp_bbr`, `sysctl -w net.core.default_qdisc=fq net.ipv4.tcp_congestion_control=bbr`, then
compare `read-bench --connections 1`. Also worth checking: the access point's channel, and that the
headset's retries stay at 0 under load (reported link: -55 dBm, 360 Mbit/s).

## Transport on a weak link (2026-10-06, perf/transport-night2)

Aim: keep playing when the Wi-Fi is jittery. The link could not be made weak on demand (the headset
sat at -45 dBm, rx 865 Mbit/s, 0 tx retries), so the measurements below are of a good link plus
injected faults; they say nothing about the weak position itself.

**Server BBR (route to the headset has `congctl bbr`) changes the picture.** `read-bench`, 8K HEVC
file, 300 MiB, medians of 3-4 alternating rounds, 3 reads in flight:

| connections | 1 | 2 | 3 | 4 | 6 | 8 |
|---|---|---|---|---|---|---|
| Mbit/s | 405 | 387 (372 in an earlier set) | 324 | 369 | 362 | 361 |

Before (CUBIC, farther from the AP): 1 connection ~127, 2: 150, 4: 202. Now one connection carries
what four do, and more connections no longer help (noise is ~10 %: 280-420 within one setting).
`ss -ti` on the server during a run: cwnd 500-580 (CUBIC sat near 30), 1.2-1.6 % of bytes
retransmitted, `rwnd_limited` 0.1 %, so the headset's receive window (autotuning max 32 MiB) is not
a limit and a bigger `SO_RCVBUF` was not tried. `TCP_NODELAY` was already set. The position and
BBR changed together, so how much of 127 -> 405 is BBR cannot be measured here (the headset has no IPv6 or
other route to the server to get a CUBIC path, and changing its network overnight was not worth the risk).
Inference from the figures: 1.2-1.6 % retransmission is about 5x what CUBIC saw before (0.2-0.3 %), and a
loss-driven controller at that loss and ~10 ms rtt would be held to tens of Mbit/s per flow, so on this
path BBR's window is not being cut by loss. Whether CUBIC would also do well at -45 dBm is unmeasured.
Kept at 4 connections: the cost with
BBR is within noise, they carry 35 % more under CUBIC, and the second-chance read (below) needs
more than one. 6 and 8 gained nothing at either position.

**Reads in flight and read size, same link (4 rounds, medians):** 3 / 6 / 12 in flight: 346 / 389 /
410 Mbit/s (request p50 82 / 115 / 229 ms); 2 MiB reads with 3 / 6 in flight: 371 / 401. With BBR
the link is no longer the limit at 3 in flight (3 MiB over 66 ms is ~380 Mbit/s), unlike with CUBIC
(3 = 8 = 16 there). Not adopted: the stream needs ~50-100 Mbit/s, the buffer is full at this link,
and every read in flight is one more a jump has to wait behind (4 in flight made jumps ~10 % slower
earlier). If a faster refill after a long stall is ever wanted, raise the cap only while the buffer
is below half its aim.

**Changes kept:**

- **Second request for a slow block** (`ReadAheadReader::with_hedge`). A read that has waited for its
  block for longer than 200 ms and 3x what requests lately take (a running average of request
  times that one stall does not move) sends the request again; the source picks the least busy
  connection, so it goes over another one. The first answer wins, the slower request is left to
  finish (never cancelled: credits), and it stops counting among the requests in flight at once
  (otherwise each stalled request held one of the 3 places for its whole 1.5 s). If one of the two
  fails, the other still answers. Costs one block per slow block (about 4 % extra bytes in the runs
  below, none in clean runs). Only with more than one connection. `Timing: frames` shows `N slow
  blocks asked again` when it happened; `--hedge-ms` (0 = off) and `JUST_VIDEO_HEDGE_MS` set the
  least wait.
- **A connection whose read failed is not chosen again.** A dead connection fails at once, so it
  always had the fewest bytes queued: by the code every retry of a block (3, 0.2-0.6 s apart) would
  go back to it and playback would stop with an error although three connections were fine (not
  observed on the headset, only argued from the code). Unit-tested
  (`pick_lane`); with `JUST_VIDEO_SIM_RESET=30` the second connection fails from the 30th read on
  and `read-bench` still carries 315-375 Mbit/s over the other three.
- Test hooks in `SmbFile` (do nothing unless set): `JUST_VIDEO_SIM_STALL=every,ms` holds back every
  n-th read (a flow stalled by loss or a timeout), `JUST_VIDEO_SIM_RESET=n` as above, `JUST_VIDEO_SIM_FLOW=every,ms` holds up the whole connection the
  n-th read went over (it and the reads after it wait out the ms: what a loss or a timeout does to a TCP flow).

**Measured, headset over Wi-Fi, 8K file, alternating, medians (stall = every 20th read held 1.5 s,
read-bench 300 MiB, 3 rounds; every 7th for jumps):**

| | no stalls | stalls, no second request | stalls, 150 ms | stalls, 250 ms (first version, 200 ms now) |
|---|---|---|---|---|
| read-bench Mbit/s | 374 (build of 6d6396e: 376) | 88 | 273 | 243 |
| `bench-seek` jumps, sum of 11 shown_ms | 3234 (build of 6d6396e: 3349) | 12952 | - | 4429 (200 ms) |

So with stalls the second request restores 2.8-3.1x of the throughput and cuts the jumps' total
time to a third (one flow's stall no longer holds a jump up for 1.5 s); without stalls it costs
nothing measurable (read-bench 385 against 374, jumps within the noise of the 6d6396e build).
The first version without the "stops counting in flight" part reached only 216 / 182 (150 / 400 ms).
A stalled whole connection is the harsher and more realistic model (`JUST_VIDEO_SIM_FLOW=15,1500`; the
second request still has to wait its 200 ms and the block behind it on the stalled connection is
avoided only through the least-busy choice): `read-bench` 300 MiB, 3 rounds, off / on / no fault:
68 / 153 / 360 Mbit/s (2.2x). Not as good as for single slow reads: each stall still costs the wait
before the second request (~0.27 s per stall against ~0.35 s of data at 360 Mbit/s); a lower wait
would give more (150 ms against 250 ms: 273 against 243 above) but is tuned to the simulation, so
it was left at 200 ms and 3x the typical request.

`play` (`JUST_VIDEO_RENDER_UNSEEN=1`, from 1038 s, 30 s, `SIM_FLOW=15,1500`, 3 alternating rounds of
second request off / on with the normal buffer and with a shallow one, 1 s / 16 MiB), the `read wait`
figures of the timing log summed over the run: normal buffer 9.2 / 8.6 / 9.5 s total, longest
1.5-1.6 s without; 2.4 / 3.8 / 3.4 s, longest 255-387 ms with. Shallow:
8.8 / 9.0 / 9.4 s, longest 1.5-1.8 s without; 3.1 / 4.0 / 4.3 s, longest 252-277 ms with; 6-12 slow
blocks asked again per run. The fault is far worse than any real link would give (a stall every 2.5 s),
and in those runs the hardware decoder itself was slow (decode 25-36 ms mean, a quarter of the
pictures shown; an earlier batch of the same test had 1860-1900 pictures shown in the first round and
read wait 1.7 s against 0.65 s), so pictures shown and skipped are not comparable between rounds
and are left out; the wait for data is what the change acts on. The boost after stalls grew the aim
to the 160 MiB cap in these runs, as designed.

**Not done / unproven:** the weak position itself (nothing here shows how much of its stutter these
changes remove); a lane that hangs without failing is only avoided by the least-busy choice and
rescued per block (a suspected lane could be avoided for some seconds); lost connections are not
re-established (the video goes on over the rest, or fails when all are gone); `bench --replay`
decodes from memory and does not touch the reader, so it was not repeated.
