# Overnight changes, 2026-10-03: what to test

All work is on branch `overnight/integration` (checked out). `main` is unchanged.
Feature branches, each merged into the integration branch: `feature/steam-art`, `feature/browser-ux`,
`feature/controls-settings`, `perf/start-and-seek`, `fix/bug-bash`.

Tests (78 + 7 CLI), clippy `-D warnings` and fmt pass. The Frame binary is already
built (`target/aarch64-unknown-linux-gnu/release/just-video`). None of this has run
on the headset yet.

## Install

```sh
FRAME_RESTART_STEAM=1 bash scripts/install-frame.sh   # restarts Steam once, to set the icon
```

The install now keeps the old binary as `~/Applications/JustVideo/just-video.prev`
(roll back: `mv just-video.prev just-video`). The app logs to
`~/Applications/JustVideo/just-video.log` (the previous run's log is `just-video.log.1`).

## What to test

1. **Steam art.** The library shows a poster (600×900), and the game page shows a banner and icon.
   If nothing shows, restart Steam. If it still doesn't show, the install prints the app id it used; compare it with the
   file name Steam writes into `userdata/*/config/grid/` when you set custom art by hand.
2. **Edit servers.** Every server row has **Edit** and **Remove** buttons (Remove asks first). The lock now
   only allows renaming or deleting files inside the server. An edit keeps the server's place in the list.
   Changing a server's address moves its saved resume points and per-file formats to the new address.
   An empty password keeps the saved one.
3. **VR/3D marks.** In a folder, VR180, VR360 and fisheye videos show a headset icon and flat 3D videos show glasses.
   Both keep the quality colour. The detail line starts with "VR180 3D ·" and similar.
4. **Faster start.** A bug meant resume never jumped: it decoded every frame from the start up to the resume point.
   It now jumps straight there (a 5-minute resume took 30 s on the PC; now 0.34 s). Opening reads less
   up front (≈ 85 ms instead of 270 ms on the PC). Subtitle files load alongside the video.
   Folder checks pause while a video plays.
5. **Faster jumps.** Long jumps (≥ 1 min, seek bar) land on the nearest keyframe. Short jumps stay
   exact, and skip frames nothing depends on while catching up. With software decoding (10-bit), a
   short jump may snap to a keyframe within half the jump (+10 s can land anywhere from +5 s to +15 s). The keyframe is shown
   at once while the decoder catches up.
6. **D-pad (left controller).** While playing:
   - Left/right jump 5 s, or 1 min with the grip held.
   - Up/down change the volume (default step 10 %; up to 150 % boosts quiet videos, with a limiter).
   - Holding a direction repeats it.
   - Every press shows a notice ("+5 s · 12:34", "Volume 60 %").
   - **If the D-pad does nothing:** run `grep Input: just-video.log`. If it says `index_controller`, SteamVR isn't
     using the Frame profile, and the log lists any rejected `dpad_*` paths.
7. **Settings.** A gear row under "Add server" opens Settings. Its rows are jump length, long jump length,
   volume step, volume, and continue where I left off. Click a row to step through its values; changes save
   at once.

## Timing data (please grab after testing)

```sh
FRAME_HOST=10.35.78.1 scripts/frame-ssh.sh 'grep -E "Timing:|Input:" ~/Applications/JustVideo/just-video.log'
# Benchmark without the headset UI (URL user/host must match `just-video servers`):
FRAME_HOST=10.35.78.1 scripts/frame-ssh.sh '~/Applications/JustVideo/just-video servers'
FRAME_HOST=10.35.78.1 scripts/frame-ssh.sh '~/Applications/JustVideo/just-video bench-seek "smb://USER@10.35.78.21/SHARE/path/video.mp4" 2>&1'
```

Still unknown until measured on the headset:
- How long the V4L2 decoder takes to flush and close.
- The real SMB connect time over Wi-Fi.
- Whether the hardware decoder honours frame skipping.

## Known, not fixed

- Rolling back to `just-video.prev` resets the new player settings (the old binary drops them on save).
- The playability marks in a folder stop updating while a video plays, and resume afterwards.
- Holding the D-pad while a video opens keeps repeating into the new video.
- A long jump to near the end finishes the video.
- Saving a server edit connects first, so you can't edit an offline server.
- Minor: a bare IPv6 address like `fe80::1` is parsed wrongly. In subtitles, a stray `{` drops the rest of the line.

## Headset benchmark (morning, 6K60 HEVC VR180 over home Wi-Fi)

Two 6K60 8-bit HEVC files from `smb://nick@n5.local/undupe/2026/09/` (hardware decoder),
measured with `bench-seek`:
- Opening: first frame in 0.2–0.6 s.
- Every jump (±10 s, ±10 min, random): 0.06–0.6 s, usually 0.2–0.4 s.

Three fixes found this way:
1. **FFmpeg patch `0002`.** FFmpeg's V4L2 decoder had no flush, so after each jump ~20 frames
   from the old place came out first. One of them was shown as the new picture, then playback
   fast-forwarded until the sound caught up. The decoder now drops queued input on a jump, and
   any old frames left are thrown away.
2. **Jumps land on a keyframe up to 1 s before the target** instead of decoding to the exact frame.
   These files have a keyframe every ~0.5 s.
3. **Read-ahead keeps at most 6 reads in flight (was 32).** Reads can't be cancelled, so a jump waited
   behind up to 32 MiB (≈1.5 s here). Throughput over this Wi-Fi is the same (~150 Mbit/s either way).

The SMB path over the home Wi-Fi gives ~150 Mbit/s, although the link runs at ~1 Gbit/s. That's
enough for these files (55 Mbit/s), but tight for 8K.

## Network (later the same morning)

- **The ~150–200 Mbit/s ceiling is the Wi-Fi, not SMB.** Plain TCP from the server to the Frame gets about the same
  (≈170–200 Mbit/s on one connection, 230–280 on two), although the radio link reports ~960 Mbit/s.
  - The Frame is on 5 GHz channel 40, 80 MHz, at -39 dBm, with power saving off.
  - Two NETGEAR mesh access points broadcast NickAir on the same channel. If the one near the Frame is a
    satellite with a wireless link back to the router, that link is the likely limit.
  - Neither the Frame's CPU (≈15 % of one core while reading) nor the server's disks (≈45 ms per cold
    1 MiB read) is the limit.
- **The playing video is now read over two SMB connections:** ~225 Mbit/s instead of ~175 (mean of 4
  alternating runs). A stall on one connection no longer stops the video, and a read after a jump goes
  to the less busy connection.
