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
| `mallopt(M_TRIM_THRESHOLD)` at start | thumbnails: 257-286 MB resident after, 287-330 MB peak | 30-32 MB after, 61-75 MB peak |

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
