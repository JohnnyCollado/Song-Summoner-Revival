# Song Summoner: The Unsung Heroes Encore — touchHLE port notes

A fork of [touchHLE](https://github.com/touchHLE/touchHLE) wired up to run
*Song Summoner: The Unsung Heroes Encore* on Windows against the user's
local music library. Everything below is about that game specifically —
the upstream emulator already runs other titles.

This document is the project-level overview. For a deeper RE-oriented
write-up of the picker work see
[`dev-docs/song-summoner-picker.md`](dev-docs/song-summoner-picker.md).

## What was tackled

The game's whole loop is *Pick a song → confirm → "Create Trooper" → play*.
On a real iPhone it goes through Apple's `MPMediaPickerController` against
the iPod library. Inside touchHLE there is no real iPod library, no real
`MPMediaPickerController`, and the game's 5-row picker is hard-wired to a
built-in 5-song fixture. The goal was to make the picker actually browse
the user's local music and have the rest of the iPod-flow downstream
of that (audio preview, confirmation panel, "Create Trooper") behave as
if the user had picked a song on an iPhone.

In scope:

- Wire a full 1317-row picker (sourced from the host music library)
  in place of the 5-row default, without breaking the game's `IPDSongsTab`
  delegate contract.
- Get the right `persistentID` to the game's pick handler on each tap.
- Play the actual user-selected MP3 as the preview when confirming.
- Make the **No** and **Back** and **Create Trooper** buttons behave.
- Get the panel to **re-build** when the user picks another song after
  tapping No, instead of being stuck on the first pick's state.

## Heads-up: antivirus false positives on Windows

`touchHLE.exe` is unsigned and ships a JIT (dynarmic) that allocates
executable memory at runtime to translate ARM code — a pattern that
trips heuristic scanners. **Avast, AVG, Bitdefender, occasionally
Windows Defender, and SmartScreen** may flag it or move it to
quarantine on first launch. It's a false positive.

Workarounds: restore the file from your AV's quarantine and add an
exception for the folder you unzipped into, or build touchHLE from
source on your own machine (the locally-built `.exe` generally
won't trip these heuristics). The end-user-facing details are in
[`dist_windows/README.txt`](dist_windows/README.txt).

The Android wrapper APK is signed (with the standard debug key)
and is not affected.

## What works

| Layer                                               | State |
| --------------------------------------------------- | ----- |
| App bundle / ARM slice loads                        | OK |
| GLES fallback rendering                             | OK |
| UI renders correctly                                | OK |
| Music library cache loads (1317 host songs scanned) | OK |
| `MPMediaQuery + songsQuery / albumsQuery / ...`     | OK |
| Promoted 1317-row picker visible in iPod app        | OK |
| Per-row dispatch — game receives the picked PID     | OK |
| Audio preview plays the actual host file            | OK |
| **No** button → cancel + remount picker             | OK |
| **Back** button → cancel + dismiss picker           | OK |
| **Create Trooper** → game advances with right PID   | OK |
| Game reaches Create Trooper / battle flow           | OK |

Verified end-to-end data contract on each pick:

1. UI row N (e.g. row 5) →
2. `persistentID` `AB7BF3A3...` (game logs `SONG : ... PID : ...`) →
3. touchHLE resolves to `03 HATSUKOI EVOLUTION.mp3` →
4. Audio path actually plays that file.

All three sides (UI, PID, file) match. The MediaPlayer hook layer is
sound — confirmed by tracing every `MPMediaItem valueForProperty:` call
through a session.

## What still does not fully work

### Cosmetic: confirmation-panel title stacking on Pick 2+

After picking → tapping **No** → picking again, the *previous* pick's
title text remains visible underneath the new pick's title text. Other
panel elements (artist name, artwork thumbnail, dialog text) render
correctly; only the **title sprite** stacks.

The functional flow is **not** affected. Even with the visible stack,
`Create Trooper` advances with the **correct** `iPodMusicID` (the
latest pick's PID).

#### Why this happens (current understanding)

- The confirmation panel is rendered through **custom OpenGL sprites**
  by the scene render function at `0x17800`, not through UIKit
  `UILabel`s. (Confirmed: no `UILabel` in `keyWindow`'s subtree has a
  panel-region frame; the game-rendered title is invisible to UIKit
  traversal.)
- On Pick 2+, the game does **not** re-query
  `MPMediaItem.valueForProperty:'title'` — title comes from a cache the
  game built at boot, keyed by PID. So we cannot fix the panel text by
  changing what we return from the media-player hooks.
- The OpenGL sprite that holds the old title sits in a sprite list whose
  address we have not located. It is **not** in the 88-byte scene struct
  at entry-5 (`+0x00..+0x57` all confirmed not to hold sprite refs after
  the panel is parked), and **not** in the global state struct at
  `0xc7890`.
- Each pick leaks ~2 GL textures (the title and artist text textures —
  confirmed via `glGenTextures`/`glDeleteTextures` tracking). Freeing
  those textures directly makes the still-referencing sprites draw
  **VRAM garbage**, which is worse.

#### Workaround

Tap **Back** to leave the iPod menu, then re-enter it. The game's
**natural** Back→re-enter transition runs the scene's destructor path
(burst of `glDeleteTextures`, scene table entry-5 reassigned to a fresh
struct, sprite list cleared as part of teardown). The next pick starts
clean.

## What we tried that did not stick

A non-exhaustive list of approaches investigated and reverted, with the
reason each was rejected. Helpful in case you have a new idea — checking
this list first will save you redoing experiments we already failed.

### Sprite-list / scene-struct manipulation

| Attempt | Result |
| --- | --- |
| Write `0xFFFFFFFF` to `scene+0x10..+0x3c` (matching in-render cleanup loop's value) | Render code still treated `-1` slots as referencing visible sprites |
| Write `0` to `scene+0x10..+0x3c` (matching natural teardown's resulting values) | Slots were **already** zero after panel parked — those aren't where sprite refs live |
| Dump full `scene+0x00..+0x57` (28 bytes past the usual window) during panel-build | No pointer-looking values appeared — sprite list lives elsewhere |
| Dump `[0xc7890]+0x00..+0x3c` (animation singleton) across picks | Unchanged after `0xa800(0)` reset; not where sprite refs live |

### Texture-pool / GL-handle manipulation

| Attempt | Result |
| --- | --- |
| Call `0xd2c8` global texture sweep on No-tap | Wiped picker-table textures → blank white rectangles |
| Call `0xc708` (per-index destroy) on the iPod scene's entry | Either no-op or destroyed wrong slot — sprites stayed |
| `glDeleteTextures` the leaked title/artist IDs directly from host | Sprites that still referenced them drew VRAM garbage |
| Track GL handles per "generation" and only delete the previous gen | Same VRAM-garbage outcome — sprite list still references the freed handle |

### Scene state-machine manipulation

| Attempt | Result |
| --- | --- |
| Reset only `scene+0x0c=0` (counter) on No-tap | Counter restarted but state stayed `0x11` (parked) → panel-build keyframe never fired → no `+songsQuery` for Pick 2 |
| `force_panel_rebuild_predicate`: write `scene+0x00=0xe`, `+0x08=3`, `+0x0c=0` | Pick-1-style behaviour, sometimes worked for one extra pick, didn't survive multiple cycles |
| `kick_scene_counter_for_rebuild`: write `scene+0x00=0x6` (loading), `+0x08+=2`, `+0x0c=0` — mimics the natural Back+re-enter delta | **Did** make `+songsQuery FIRED` fire reliably on Pick 2+ with the new PID. **But** state 0x6 made the game instantiate a *second* `iPodView2` on top of the existing one — two stacked iPodView2s in the window |
| Same kick + recursively remove all `iPodView2` instances from the window via UIKit traversal before the kick | Not enough — `iPodView2` total instance size is 21 bytes with 1 ivar (`isCanceled`); the title sprites are not its property, they're in C++ rendering state held elsewhere |

### Scene/iPodView destruction

| Attempt | Result |
| --- | --- |
| Allocate a fresh scene struct via the constructor at `0x16602`, write it into scene table entry 5 | Old scene leaked, double-render of all sprites, much worse than stacking |
| Find the scene destructor function | Function exists somewhere in the `0xc800..0xcb50` cluster (~13 small helpers around the scene-table) but is reached only via computed addressing (`base + idx*stride + offset`), not via direct `BL`. Static search has not surfaced it. |
| Look for an iPodView2 destructor / dealloc override | iPodView2 has only one ivar (`isCanceled`) and no custom dealloc surfaced — destruction happens at the C++ rendering layer, not at the Obj-C class level |

### Obj-C / view-hierarchy approaches

| Attempt | Result |
| --- | --- |
| `find_view_by_class("iPodView2")` + `removeFromSuperview` | Removes the UIView from the hierarchy but the C++ rendering layer keeps drawing because the sprite list is not owned by the UIView |
| Dump `ViewManager` / `MainView` ivars via Mach-O Obj-C metadata | Located `ViewManager` ivar layout (`+0x5c=ipodview`, `+0x60=loadView`, ...) but the ivars are *pointers to* the view objects; the sprite list itself is owned in C++ state below the Obj-C layer |
| Set up panel-views snapshot at `+songsQuery` time | Useful for diagnosis (showed labels with `parent=UITableViewCell` only — confirms the title is **not** a UIKit label) — did not lead to a fix |

### Why we stopped

The remaining fix path is: locate the C++ sprite list that the scene
render at `0x17800` iterates over each frame, then find the function
that mutates it on the natural Back transition, then call that function
on the No path. That's ~5–10 hours of careful RE inside the
`0xc800..0xcb50` cluster with no guarantee. The game is otherwise fully
playable — the visible stack is annoying but doesn't break gameplay or
mis-trooper anything — so this is documented as a known limitation
rather than a blocker.

## Files touched (relative to repo root)

- `src/frameworks/uikit/ui_view/ui_table_view.rs` — promoted picker,
  picker-swap dispatch, scene observer, `invoke_ipodview_reset`,
  `force_panel_rebuild_predicate`, GL texture tracker.
- `src/frameworks/uikit/ui_touch.rs` — button-area touch classifier;
  No / Back / Create Trooper routing.
- `src/frameworks/foundation/ns_run_loop.rs` — per-tick scene observer
  hook + deferred picker-swap drain.
- `src/frameworks/media_player/media_query.rs` — `+songsQuery`,
  `+albumsQuery`, etc.; collections-filtering for picker swap;
  state snapshots on query.
- `src/frameworks/media_player/media_item.rs` — `MPMediaItem` host
  object, `valueForProperty:` hook, cache-by-song-index identity.
- `src/frameworks/media_player/music_library.rs` — host song lookup,
  PID staging, audio preview path.
- `src/frameworks/media_player/media_picker_controller.rs` — delegate
  capture stub (unused since game uses its own
  `IPDMediaPickerController`).
- `src/frameworks/opengles/gles_guest.rs` — `glGenTextures` /
  `glDeleteTextures` tracking hooks.
- `src/frameworks/openal.rs` — `alcDestroyContext` LR-logging hook
  (used during scene-destructor RE).
- `inspect/` — RE scripts: see
  [`dev-docs/song-summoner-picker.md`](dev-docs/song-summoner-picker.md)
  for the full list.

## Key binary addresses

| Addr        | What |
| ----------- | ---- |
| `0x322c`    | `MainLoop_Set_iPodState(int)` |
| `0x3244`    | `MainLoop_Set_iPodCancel(int)` |
| `0x325c`    | `MainLoop_Set_iPodMusicID(uint32 lo, uint32 hi)` |
| `0xa800`    | iPodView2 animation reset (arg=0 = full reset) |
| `0xc48d0`   | MainLoop state struct base |
| `0xc7890`   | Animation / render-state singleton |
| `0xc9f4`    | `get_scene_obj()` |
| `0x16602`   | Scene constructor (mallocs 88 bytes, init state=9) |
| `0x17800`   | Scene render function (frame-counter keyframe dispatch) |
| `0x1134f0`  | Scene table base (16 entries × 0x1c bytes; scene_ptr at +0x18) |
| `0x1136b0`  | Current scene index (uint32) |
| `0x60150`   | `iPodView2.mediaPickerDidCancel:` |
| `0x60170`   | `iPodView2.mediaPicker:didPickMediaNumber:` |
