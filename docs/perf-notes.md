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
