# Feature ideas

Already in the app (so not ideas): resume position per file, per-file layout and
picture overrides, subtitle tracks and sidecar files with size/position, audio
track choice, previous/next video, thumbnails, rename/delete behind a server lock.

Effort: S under half a day, M about a day, L more. Risk is about breaking
playback or needing the headset to judge it.

| # | Idea | Value | Effort | Risk | Plan |
|---|------|-------|--------|------|------|
| 1 | Watched and resume markers in the list | See at a glance what is finished and where to continue, without opening files | S | Low (local config read once per folder) | Build |
| 2 | Sort modes (A-Z, largest, unwatched first) | Find the next thing to watch in a big folder; sort persists | S | Low | Build |
| 3 | A-B loop | Repeat a section (practice, favourite scene) | M | Medium: seek timing, a new button on the bar | Build (logic and a bar toggle) |
| 4 | Playback speed | Skim lectures or slow a section down | M | High: audio resampling and clock master | Defer |
| 5 | Skip back/forward by a fixed step | Quick 10 s hops without aiming at the seek bar | S | Low | Build if time allows |
| 6 | Filter by name (on-screen keyboard) | Jump to a title in a long folder | M | Medium: keyboard in the browser panel | Defer |
| 7 | Hide watched videos toggle | Declutter once something is finished | S | Low, but shifts row indexes | Defer (sort covers most of it) |
| 8 | Continue watching list on the home screen | One click back to the last few videos | M | Medium: needs stored paths and a place in the server list | Defer |
| 9 | Auto-play next in the folder | Binge without taking the headset off | S | Low, but needs an on/off setting and an end-of-video hook | Defer |
| 10 | Mark watched / unwatched by hand | Fix wrong markers, mark a series done | S | Low | Defer |
| 11 | Controller help overlay | Learn the buttons without a manual | M | Medium: needs on-device layout check | Defer |
| 12 | Plain-language playability hints on rows | Explain why a file stutters or is refused | S | Low, mostly exists as the row title | Defer |
| 13 | Recenter and view nudges (yaw/pitch/IPD-like offsets) | Comfort for different heads and seating | M | High: needs headset to judge | Defer |
| 14 | Remember sort and filter per folder | Different folders, different habits | S | Low | Defer |
| 15 | Per-folder default layout | Whole VR folder opens in the right projection | M | Medium: precedence against file overrides | Defer |
