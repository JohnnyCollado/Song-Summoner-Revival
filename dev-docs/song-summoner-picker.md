# Song Summoner: The Unsung Heroes Encore — iPod Music Picker

Notes on the hooks and patches that make the game's iPod music picker
work against a user's local music library inside touchHLE on Windows.

## What works

- **1317-row promoted picker.** The game's default 5-row `IPDSongsTab`
  is replaced at runtime with a full-library view (via the reloadData
  hook in `ui_view/ui_table_view.rs`).
- **Per-row dispatch.** A tap on the promoted picker forwards to the
  game's `IPDSongsTab.tableView:didSelectRowAtIndexPath:`, so the game
  receives the correct `persistentID` for the picked song.
- **Audio preview.** `play_song(row)` plays the user's actual file via
  the host audio path when the user picks.
- **No / Back / Create Trooper buttons.** Touch-coord classification in
  `ui_touch.rs` identifies which game-rendered (OpenGL) button the user
  tapped on the confirmation panel and routes:
  - **No** → `MainLoop_Set_iPodCancel(1)` + un-hide promoted picker.
  - **Back** → `Set_iPodCancel(1)` + `dismiss_promoted_picker`.
  - **Create Trooper** → `dismiss_promoted_picker`; game advances scene
    using the picked PID.

## Panel-rebuild on pick 2+

The game's confirmation panel is rendered by a scene render function at
`0x17800` reading `scene_obj+0xc` as a per-frame counter. Specific
counter values (frames 3, 6, 9, 15, 30, 31) trigger keyframe actions —
notably the `MPMediaQuery + valueForProperty` calls that populate the
panel.

When the user picks once → taps No → picks again, the scene is not
naturally torn down (touchHLE's `UIViewController.navigationController`
is a stub that returns nil, so the game's natural pop-and-rebuild path
is bypassed). The scene stays alive with the counter parked at frame 31
(idle).

To trigger a fresh panel build on each pick, the picker-swap dispatch
in `ui_view/ui_table_view.rs::force_panel_rebuild_predicate` writes:

```
scene+0x00 = 0x0e   (scene type id)
scene+0x08 = 3      (prev_state cookie)
scene+0x0c = 0      (counter reset → naturally advances to keyframe 3)
```

This re-arms the scene for a fresh keyframe walk. `+songsQuery` fires
with the new PID and the panel text/artwork query happens correctly.

## Known cosmetic limitation: panel text stacking

After picking → tapping **No** → picking again, the new song's
title/artist text renders **on top of** the previous pick's text. The
two appear stacked / overlapping in the confirmation panel area.

### Root cause

The panel content is rendered as custom OpenGL sprites, not UIKit
subviews (confirmed: no `UILabel`/`UIImageView` in keyWindow's view
tree has a panel-region frame). The sprite list lives in a location we
haven't located:
- Not in `scene+0x10..+0x3c` (always zero).
- Not in `[0xc7890]+0x00..+0x3c` (the iPodView2 animation singleton —
  contents unchanged across picks after `0xa800(0)` reset).
- Likely in `iPodView2`'s instance ivars or a separate global sprite
  manager, accessed via indirection that hasn't surfaced via static
  searches.

GL texture tracker confirmed each pick leaks ~2 textures (the
title/artist text textures). Freeing the textures directly causes the
old sprites to render garbage VRAM (the sprites that reference them
remain in the render list and draw with broken texture handles).

### Workaround

To clear the stack, return to **Soul Master via Back**, then re-enter
the iPod music menu. The game performs a natural scene reconstruction
during this transition (new scene struct allocated, sprite list reset
as part of teardown — observed in logs as a burst of
`glDeleteTextures` calls and a new `entry5.scene_ptr`). The next pick
starts cleanly.

The functional flow is not affected: even with the visible stack,
`Create Trooper` uses the **correct** `iPodMusicID` (verified in logs).
Only the visible labels are stale.

### Why we stopped here

The natural cleanup path involves:
1. Calling the iPod scene's destructor (function unknown — not reached
   via direct BL search, accessed indirectly).
2. Calling the scene constructor at `0x16602` to allocate a fresh
   scene struct (also not reached via direct BL).
3. Writing the new pointer into scene table entry 5
   (`0x1134f0 + 5*0x1c + 0x18`) — accessed via computed addressing
   (`base + idx * stride + offset`), not by literal.

Finding the natural swap function requires disassembling and tracing
the ~13 small functions in the scene-management cluster at
`0xc800..0xcb50`. Estimated 5-10 hours of careful RE with no
guarantee of a clean fix.

## Files touched

- `src/frameworks/uikit/ui_view/ui_table_view.rs` — promoted picker,
  picker-swap dispatch, `force_panel_rebuild_predicate`, GL texture
  tracker, scene observer.
- `src/frameworks/uikit/ui_touch.rs` — button-area touch classifier,
  No/Back/Create Trooper routing.
- `src/frameworks/foundation/ns_run_loop.rs` — per-tick scene observer
  hook + deferred picker-swap drain.
- `src/frameworks/media_player/media_query.rs` — `+songsQuery` hook
  for state snapshots; collections-filtering for picker-swap.
- `src/frameworks/media_player/music_library.rs` — host song lookup,
  PID staging, audio preview.
- `src/frameworks/media_player/media_picker_controller.rs` — delegate
  capture (unused since game uses its own IPDMediaPickerController).
- `src/frameworks/opengles/gles_guest.rs` — `glGenTextures` /
  `glDeleteTextures` tracking hooks.

## Key binary addresses for future work

| Addr        | What |
|-------------|------|
| `0x322c`    | `MainLoop_Set_iPodState(int)` |
| `0x3244`    | `MainLoop_Set_iPodCancel(int)` |
| `0x325c`    | `MainLoop_Set_iPodMusicID(uint32 lo, uint32 hi)` |
| `0xc48d0`   | MainLoop state struct base |
| `0xc7890`   | iPodView2 animation/render-state singleton |
| `0xc9f4`    | `get_scene_obj()` |
| `0xa800`    | iPodView2 animation reset (arg=0 = full reset) |
| `0x1134f0`  | Scene table base (16 entries × 0x1c bytes) |
| `0x1136b0`  | Current scene index (uint32) |
| `0x16602`   | Scene constructor (mallocs 88 bytes, init state=9) |
| `0x17800`   | Scene render function (frame-counter keyframe dispatch) |
| `0x60150`   | `iPodView2.mediaPickerDidCancel:` |
| `0x60170`   | `iPodView2.mediaPicker:didPickMediaNumber:` |

The scene-management cluster `0xc800..0xcb50` contains ~13 small
functions that mediate scene-table reads/writes. The scene swap /
destructor lives there.

## Reverse-engineering scripts

In `inspect/`:

- `disasm_getsong.py` — disassemble specific IMPs by selector
- `dump_main_loop_setters.py` — disasm `MainLoop_*` accessor functions
- `find_state_writes.py` — find all writes to `iPodState` with constants
- `find_set_state_calls.py` — find BL callers of state setters
- `find_a800_callers.py` — find callers of `0xa800` / `0xa788` /
  `0xc57c` (animation/state reset routines)
- `find_panel_builder.py` — find functions calling `MPMediaQuery
  +songsQuery`
- `find_query_util_callers.py` — find callers of util selectors via
  selref lookup
- `re_state_observers.py` — find readers/writers of MainLoop state
  struct fields
- `find_scene_swap.py` — find scene-constructor callers and scene-table
  write sites (the cluster at 0xc800..0xcb50 is where to look next).

## See also

- `inspect/RE_FINDINGS.md` — earlier RE writeup (the model evolved
  significantly; the "frame counter" insight in this doc supersedes
  the "state machine" model in RE_FINDINGS.md).
