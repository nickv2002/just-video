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

**Headset benchmarks and the old-build comparison were not run.** The player app (PID started
16:58, idle in the file browser since 17:02) was still running at the end of the 20-minute wait
(17:07-17:32), and the rules for this task forbid disturbing a running session. What is ready:
`just-video bench <url> --frames 600 --replay` now decodes the same frames a second time from the
bytes the first run read (memory, no network); the report has `decode` (network + decoder) and
`replay_decode` (decoder only). Planned next, with the app closed and the headset muted: per
round `bench --replay --hw-only` and `bench-seek --play 40 --from 1000 --hz 60 --json` with the
installed build, `be8ad10` and `2c20a1e` alternating, plus `JUST_VIDEO_NO_TRIM=1`, and a
`read-bench` that prints a line per second (it only prints every 512 MiB). The new build's lines
need `JUST_VIDEO_RENDER_UNSEEN=1 just-video play … --hw auto --duration 60`, as `bench-seek` does
not print them.
