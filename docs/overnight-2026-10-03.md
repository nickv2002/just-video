# Overnight changes, 2026-10-03: what to test

All work is on branch `overnight/integration` (checked out). `main` is unchanged.
Feature branches, each merged into the integration branch: `feature/steam-art`, `feature/browser-ux`,
`feature/controls-settings`, `perf/start-and-seek`, `fix/bug-bash`.

Tests (82 + 7 CLI), clippy `-D warnings` and fmt pass. The Frame binary is already
built (`target/aarch64-unknown-linux-gnu/release/just-video`). Benchmarks below ran on
the headset; the D-pad and the UI changes still need someone wearing it.

## Install

```sh
FRAME_RESTART_STEAM=1 bash scripts/install-frame.sh   # restarts Steam once, to set the icon
```

The install now keeps the old binary as `~/Applications/JustVideo/just-video.prev`
(roll back: `mv just-video.prev just-video`). The app logs to
`~/Applications/JustVideo/just-video.log` (the previous run's log is `just-video.log.1`).

## What to test

**First, since the afternoon install:** the app now uses the Frame controllers' own input profile,
not SteamVR's Index mapping. Every binding changed, not only the D-pad. Check pointing, trigger
(select), A and B (back) before the D-pad. If the basics misbehave, roll back over SSH with
`scripts/frame-ssh.sh 'cd ~/Applications/JustVideo && mv just-video.prev just-video'`,
and send `grep -E "OpenXR|Input" ~/.local/state/just-video/log.txt`.

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
   - Up/down change the headset's own volume (default step 10 %), as its volume buttons do (since the evening change below).
   - Holding a direction repeats it.
   - Every press shows a notice ("+5 s · 12:34", "Volume 60 %").
   - **If the D-pad does nothing:** run `grep -E "OpenXR|Input" ~/.local/state/just-video/log.txt`. It should say
     `enabled XR_VALVE_frame_controller_interaction` and `frame_controller: 20 bindings`. Until the afternoon
     fix below, the extension was never enabled, so the D-pad could not work.
7. **Settings.** A gear row under "Add server" opens Settings. Its rows are jump length, long jump length,
   volume step, and continue where I left off (the volume row is gone since the evening change). Click a row to step through its values; changes save
   at once.

## Timing data (please grab after testing)

```sh
# Started from Steam, the app logs to ~/.local/state/just-video/log.txt (previous run: log.previous.txt).
scripts/frame-ssh.sh 'grep -E "Timing|Input|OpenXR" ~/.local/state/just-video/log.txt'
# Benchmark without the headset UI (URL user/host must match `just-video servers`):
scripts/frame-ssh.sh '~/Applications/JustVideo/just-video servers'
scripts/frame-ssh.sh '~/Applications/JustVideo/just-video bench-seek "smb://nick@n5.local/undupe/path/video.mp4" 2>&1'
```

Still unknown until measured on the headset:
- How long the V4L2 decoder takes to flush and close.
- The real SMB connect time over Wi-Fi.
- Whether the hardware decoder honours frame skipping.

## Known, not fixed

- Rolling back to `just-video.prev` resets the new player settings (the old binary drops them on save).
- The playability marks in a folder stop updating while a video plays, and resume afterwards.
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

## Playback performance (afternoon, measured in the real frame loop)

**How it was measured.** SteamVR doesn't ask an app for frames while nobody wears the headset.
`JUST_VIDEO_RENDER_UNSEEN=1` makes the frame loop draw anyway. The run then goes through the
real swapchains, upload, shader and `xrEndFrame`. Launching through Steam applies the app's
SteamVR settings. The app now logs `Timing: frames …` every 5 s while a video plays: time in each part of the
frame loop, GPU time (timestamps), missed display periods, and skipped pictures. It also logs
the frames that made the runtime skip a period.

**Your SteamVR settings for Just Video** (`steamvr.vrsettings`, left unchanged): 120 Hz,
`framesToThrottle: 1` (the app runs at 60 Hz), resolution override 3408 (eyes 3408×3408).

Fixes, 6K60 VR180 HEVC (6144×3072):

1. **D-pad: the Frame controller extension was never enabled.** The openxr crate keeps extension
   names NUL-terminated, so the check missed it, and SteamVR mapped the Frame controllers
   onto Index bindings (no D-pad). Your morning log shows this. The native profile is now
   accepted with all 20 bindings, D-pad included (bindings only; nobody has pressed it yet).
2. **The frame loop no longer waits for the GPU.** Our GPU work is ~5 ms per frame at 3408²
   (upload 1.5, both eyes 3.5). The GPU runs it only after the compositor's work, and waiting for
   that took ~11 ms of each frame. Per frame, the loop's own work went from **14 ms to 4 ms**. Before,
   it was over budget at 90 and 120 Hz; that may be why SteamVR throttles the app.
   Two frames are now in flight (their own command buffers, fences, staging buffers and colour
   uniforms). Screenshots of two 6K VR videos rendered with the new code are correct: no tearing
   or stale picture. (None was taken with the old code to compare against.)
3. **Smoother pacing.** The audio position runs in a ~7 ms sawtooth every 1.5 s. The video clock
   followed it, so a frame was shown twice and the next skipped every second or two
   (0–3 per 5 s). Now the clock follows the audio closely for 1.5 s after a start, jump or pause.
   After that, it only corrects drift beyond 12 ms (real drift is ~0.2 ms/s). Result: **0 skipped frames**
   per 5 s, on 6K60 and on 1080p30.

Steady state now, through a Steam launch:

| Video | Display | Missed periods | Pictures / 5 s | Loop work max | GPU |
|---|---|---|---|---|---|
| 6K60 HEVC VR180 (hardware) | 60 Hz, eyes 3408² | 0 | 300–302, 0 skipped | ~4 ms | 5 ms |
| 1440p60 AV1 10-bit (CPU decode) | 60 Hz | 0 | 301–302 | ~1.8 ms | 2 ms |
| 1080p30 H.264 | 60 Hz | 0 | 151, 0 skipped | ~1.4 ms | — |

**9-minute soak** (6K60 VR180, Steam launch, 60 Hz, 3408² eyes): after the first 5 s, every
5-s window had 0 missed periods and 0 skipped frames. GPU time (eyes 3.5 ms, worst 5.1) and loop
work (~4 ms) stayed flat. The hottest sensor went from 54 °C to 59 °C. Panel updates (notices,
captions) cost 2–4 ms of the loop, text rendering included, and missed no periods.

**Installed on the headset:** see the end of these notes (the evening build replaced the 12:01 one).

Still to look at:
- **The first second of a video misses ~20 display periods.** During those frames our loop's
  work is small, and `xrEndFrame` blocks for up to 13 ms. This is probably SteamVR's launch
  transition; the test launched straight into a video. Starting a video from the browser may not
  show it. Check `Timing:` in the log after a real session.
- Copying each picture costs 2 ms of CPU and 1.5 ms of GPU at 6K. Importing the decoder's buffers
  straight into Vulkan would remove both, but needs larger changes, and the frame budget doesn't need it now.
- **Not measured at 120 Hz.** Every Steam-launched run went at 60 Hz (throttled). With ~4 ms of
  loop work and 5 ms of GPU per frame, 120 Hz (8.3 ms) should now fit: expected, not shown.
  Resetting Just Video's per-app "throttling" setting in SteamVR would show it.
- My browser-mode test (run without a terminal) rotated the app log, so the log from before
  00:40 is gone. `log.previous.txt` is now the 00:40 session.

## Evening: jumps that stopped playback, headset volume, D-pad menus

**Jumping forward quickly sometimes dropped back to the browser.** It wasn't the buffer. The
headset's video driver (iris) sometimes refused to restart decoding at a jump. The kernel log
says `current session not supported(-12)` on `STREAMON`, and the app logged `Decoding failed
(FFmpeg error -12)`. That restart comes from our FFmpeg patch `0002` (the fast-seek flush).
After one refusal, that decoder session stays broken: retrying gets "busy" (-16) every time.
The player now replaces the video decoder, keeping the file, audio and subtitles, and jumps
again. That takes 7–43 ms and stays on the hardware decoder. The log line is `Decoder failed (…);
replaced in N ms (hardware), continuing at …`.

- It shows on fast-decoding video (your 1080p H.264 episode) right after an exact jump, which
  decodes ~30 frames as fast as it can. In 6K, short jumps land on keyframes, and 39 jumps
  never triggered it.
- To reproduce: `bench-seek <url> --from 5 --play 60 --hz 60 --jump-every 0.3`. On that
  episode, before the fix, playback stopped within 3 jumps. After it: 119 jumps at one per
  0.5 s, 40 refusals, all recovered, and playback never stopped.
- 6K with forced failures: 19 replacements of 31–43 ms, all on hardware.

**D-pad up/down set the headset's volume.** That's the SteamOS level its buttons set
(`wpctl` on the default output, where the app's sound ends up), at most 100 %. Turning it up
also unmutes. The app's own gain is always 100 % now; the old Settings "Volume" row and the
saved value are gone. Untested with someone listening: check that the notice's percentage
matches Steam's volume slider.

**D-pad in menus.**
- In the browser:
  - Any D-pad press shows a focus (the same highlight as pointing).
  - Up/down move row by row and scroll the list. Right on a folder or share opens it (let go
    before it opens the next one); on rows with buttons (servers, edit mode) left/right reach
    the buttons (Rename, Delete, Edit, lock…).
  - Up from the first row reaches the breadcrumbs and header buttons.
  - A (or the trigger) clicks; B goes back one level (it already did).
- In dialogs, focus starts on the safe button (Cancel, never Delete).
- On the keyboard, the D-pad moves between keys, and Shift keeps the focus in place.
- Moving the pointer more than a little hands control back to it.
- While playing, an open dialog (tracks, screen, image) takes the D-pad the same way, and B closes the
  dialog instead of leaving the video. With no dialog open, the D-pad still jumps and sets the volume.
- A sideways stick flick counts as left/right.
- A direction held while a video opens or closes no longer repeats into the next screen.

**To test:**
1. In a folder, press down a few times, then right on a row with buttons, then A.
2. Press B to go up, check the focus is on the folder you left, then A to go back in.
3. Open "Add server": move around the keyboard and type with A.
4. While playing, open the track dialog, pick a track with the D-pad and A, then B (the video should keep playing).
5. Press D-pad up/down while playing and compare with Steam's volume.
6. Jump forward quickly on the episode that used to stop.

**Installed on the headset (evening):** the latest commit on this branch. To roll back, in
`~/Applications/JustVideo`: `just-video.prev` has the jump fix only (old app volume, no D-pad menus),
and `just-video.pre-overnight` is the original from before last night. This morning's build is gone.

## Afternoon: faster folders and opening (SMB)

**Status:** committed on `overnight/integration` and installed on the headset (14:30). To roll back,
in `~/Applications/JustVideo`: `mv just-video.prev just-video` (the evening build). Tests: 94 + 7;
clippy and fmt pass.

Measured on the headset over the home Wi-Fi, with a test binary (`just-video.test`) run next to the
installed one. `library-stress` runs the app's own library workers without the headset UI.

**Where the time went before.** A 13-video folder took ~3.5 s to mark, one video at a time, and
again on every visit. Opening a video spent ~30 ms connecting and ~50 ms opening the file before
reading anything.

1. **Folder marks are remembered** (`~/.config/just-video/probe-cache.json`). A folder seen
   before is marked in **5 ms instead of ~3.5 s**, also after a restart. The cache keeps what the
   header says (codec, size, bit depth…), not the verdict, so playability rule changes apply at
   once. A file whose size or modification time changed is read again. Errors are never kept.
   Rename and delete keep it up to date. The old binary ignores the file.
2. **New folders: 3 videos read at once.** This helps less than hoped: 3.7 s → 3.35 s (median of 4
   alternating pairs). Probing is mostly bandwidth: FFmpeg reads each MP4's index, 1–11 MiB per file,
   45 MiB for this folder. Limiting FFmpeg's frame-rate probing (`fpsprobesize`) changed neither
   the bytes nor the results. Probes now pause mid-file when a video opens, and run again after it
   closes: clicking right after entering a new folder costs nothing extra (the open's probe waited
   104 ms with 3 workers, 126 ms with 1). After that video closed, the paused probes finished: all
   13 videos were marked 3.2 s later.
3. **Opening connects to the share directly, both connections at once.** "open file": **48 → 5 ms**.
4. **Standby connections.** When a folder with videos shows (and when a video ends), two
   connections sign in to its share in the background. Opening uses them: "connect": **~30 → 0 ms**.
   They're used for up to 10 minutes. If they fail to open the video within 2 s
   (server restarted, headset slept), it connects again as before. The log line says `warm connections`.
   Together with 3., opening spends **~7 ms instead of ~77 ms** before reading the file (medians of 12 opens).
5. **A cached folder doesn't stall a frame.** All its marks arrive at once. The rows are now rebuilt
   once, not once per video (2000 videos: 10 ms instead of 1.6 s in a debug test).
6. **The log now times folders:** `Timing: folder NAME: listed in N ms, M videos marked in N ms (K from cache)`.
   `just-video info` also prints what probing read.

**What's left when opening** (6K VR180 and 1080p files): the header probe, ~110–150 ms waiting for
the network. The folder probe read the same bytes earlier, but the cache keeps facts, not bytes.
Next candidate: prepare the video the pointer rests on (open it and read its header before the
click). Not built: it's speculative network work, and it needs care so it doesn't block renaming the
pointed-at file.

**Tried and shelved: smaller first reads after a jump** (the jumped-to 1 MiB block as 4 × 256 KiB parts).
Over 6 alternating runs the jump times varied more between runs of the same binary (166–332 ms)
than between binaries. No measurable gain, so not kept.

**Not measured:**
- Whether 3 probe threads cost the browser display periods (`Timing: frames` only runs while playing).
- Unexplained, twice, on both the installed and the test binary: the first `bench-seek` right after
  another one spent ~4.5 s in "open decoder". It wasn't seen inside the app.

`library-stress` gained `--cold` (forget the probe cache), `--warm` and `--click-at-once`, and
`JUST_VIDEO_PROBE_WORKERS=N` sets how many probes run at once, for comparing.

**To test (in the headset; `library-stress` doesn't run the browser screens):**
1. Enter a folder, go up, enter it again: the marks appear at once.
2. Open a video from it: it starts as before (the log says `warm connections`).
3. Rename a video (server unlocked): its mark stays.
4. Enter a folder never seen before and pick a video at once: it plays normally. After you back out,
   the rest of the marks fill in.

```sh
scripts/frame-ssh.sh 'grep -E "Timing: (folder|open)|warm|Probe cache|Library" ~/.local/state/just-video/log.txt'
```

## Evening: sound that stuttered, and 8K that stuttered

**Cause.** In the session where you heard it, every hardware decoder open failed
(`hevc_v4l2m2m … VIDIOC_REQBUFS failed: Cannot allocate memory`), so 6K and 8K were decoded on the
CPU. The CPU can't decode 8K in real time, and the sound is decoded by the same thread, along with
the picture. So the sound ran out too (about 15 % short), and the picture drifted 100–360 ms ahead
of it. After a reboot the hardware decoder opened every time (also 5 × 8K back to back). With it,
6K, 8K and 1080p all play with 0 skipped pictures and the sound never short. 8K on hardware doesn't
stutter, but has less headroom: up to 9–14 ms of the 13.9 ms frame at 72 Hz, mostly copying the picture.
Probably today's ~100 benchmark decoder sessions left the driver unable to allocate; not proven.

**Changes:**
1. **The sound is read up to 1 s ahead of the picture.** The decoder keeps a queue of video packets
   and decodes sound as soon as it is read. Right after a jump, the queue waits until the first picture
   is out, so jumps and opening stay as fast. The queue then grows by up to 3 packets per picture.
2. **CPU decoding that falls behind decodes less.** It skips frames nothing refers to. More than 1 s
   behind, it decodes keyframes only until it catches up; with keyframes seconds apart, the picture
   then freezes for a moment. Hardware decoding is unaffected.
3. **A failed hardware decoder open is retried twice** (after 0.25 s and 0.5 s) before using the CPU.
   If it still fails, the video shows "Hardware video decoder unavailable: playing on the CPU. If this
   keeps happening, restart the headset." The retry is untested: the failure didn't come back.
4. **New log lines every 5 s while playing:** `Timing: audio …` (how full the sound buffer was, and
   time spent waiting for sound), `Timing: sync picture ahead of sound by …`, and `Playback: decoding fell behind …`.

**8K on the CPU** (the failure case, measured headless with `--hw none`), per 5 s:

| | Sound arrived | Waited for sound | Picture vs sound |
|---|---|---|---|
| Before | ~85 % | ~400 ms | 100–140 ms ahead (up to 360) |
| After | 100 % (never below 59 ms queued) | 0 ms | within ±13 ms |

The picture itself still lags up to ~0.8 s on the CPU and skips about half its frames; it can't keep up.

**Jumps (6K, `bench-seek`, 5 alternating pairs):** median ready 140 ms vs 122 ms installed. That's within
the Wi-Fi spread (122–313 vs 112–152), but may be a small real cost.

**To test:** play an episode and a VR video for a few minutes and listen. Then:
`scripts/frame-ssh.sh 'grep -E "Timing: (audio|sync)|Playback:|CPU|REQBUFS" ~/.local/state/just-video/log.txt'`

## Late evening: jumps that never showed, and resuming too early

Three problems found above, all also in the installed build:

1. **H.264 on the hardware decoder never showed a picture after a jump.** After a jump flushes it,
   the iris H.264 decoder returns one empty picture and then never takes another packet, so the
   decoder thread waits forever (HEVC flushes fine). Playing straight through, or opening at a
   time, works. Now a jump in H.264 on the hardware opens a new decoder (14–26 ms at 1080p),
   like opening the video at that time. 1080p `bench-seek`: every jump shown in 46–294 ms
   (before: none).
2. **"Almost nothing shown at a jump every 0.3 s"** was problem 1 on the 1080p file, and on 6K the
   driver failing at jumps (below). 1080p, a jump every 0.3 s for 20 s: 484 pictures shown over
   66 jumps (before: about 10 in 60 s).
3. **Resuming 8K started up to 10 s early** (keyframes 10 s apart; it always took the keyframe
   before). Now resuming takes a keyframe up to 2 s before the target, else one up to 3 s after
   it (the target is already 3 s before where you stopped, so never past that). Else, on the
   hardware and with the keyframe before at most 4 s back, it decodes on to the target,
   showing the keyframe meanwhile. Otherwise it still starts at the keyframe before: 8K
   decodes at only ~1.8× real time, so a 5 s gap is a ~3 s still picture. 8K resumed at
   839.4 s: starts at 840.8 s in 0.38 s (before: 830.8 s in 0.5 s).

**The hardware decoder failing at jumps (6K and 8K).** Every jump in 6K or 8K fails on the
hardware ("busy", then "out of memory"), in every build, including the one from before this
work. Opening a video at any time still works. It is not worn-out driver state, as first
guessed: it fails right after a restart too. Same kernel, SteamOS and Steam builds on every
boot. 6K jumps worked at 16:33 on Oct 3 and failed from 17:10, right after a reboot, on every
boot since. Likely cause, not proven: Steam's web helper (its browser zygote) holds a hardware
decoder session with 8 buffers from boot. A jump restarts our decoder, which needs more
than the driver has left. Opening a new decoder at each jump fails the same way: the old
one stays alive while its pictures are still on screen or queued. Now the last of the three
restarts at a jump goes to the CPU instead of stopping, with the "playing on the CPU" notice.
6K in that state: every jump shown in 0.3–1.1 s on the CPU, but 8K then plays on the CPU and stutters.

**`bench-seek` now fails** (exit code 3) when a jump never shows a picture, playback stops, or
the hardware decoder fails at a jump and the CPU takes over, so it works as a check on the headset.

**Cause confirmed (Oct 4):** with Steam's web helper process holding the decoder stopped (Steam
restarted it without opening the decoder again), 8K jumps had no failures and stayed on the
hardware. 6K had no failures in one run, and one failure that recovered on the hardware in
another. Steam may open its session again (at the latest on the next boot).

**Fixed: the old decoder closes before a new one opens.** The renderer copies each picture, so
the player now lets go of its frames at a jump (the picture stays on screen). The decode thread
waits until no frame of the old decoder is left (at most 0.3 s), frees it, then opens a new one.
HEVC still flushes at jumps until a flush fails once; from then on, and always for H.264, every
jump gets a new decoder this way (70–90 ms). Measured with another hardware decoder session
running (a 1080p decode in the background, standing in for Steam's):

| 6K jumps, with a second session | Before | After |
|---|---|---|
| Hardware failures | 3, then the CPU for the rest | 1 (the first flush), then hardware |
| Jumps shown | 0.4–0.9 s (CPU) | 0.15–0.7 s (once 1.4 s) |

8K: the same, all jumps on the hardware. Not fixed: short 8K jumps that decode on to the target
take 1.6–4.3 s (it assumes the hardware decodes much faster than real time; at 8K it manages ~1.8×).
