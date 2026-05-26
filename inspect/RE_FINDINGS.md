# Song Summoner RE — Confirmation Panel Refresh Investigation

## TL;DR

The confirmation panel render is gated by a **scene-specific** state field, NOT by `iPodState`. On pick 1 the scene starts at state=9 (the "show confirmation" branch in the render function). After pick 1's confirmation is dismissed, the scene state transitions to other values (5, 15, 30, etc.) — but we never push it back to 9, so pick 2's `didPickMediaNumber:` triggers no rebuild even though it correctly sets `iPodState=4` and `iPodMusicID=newPID`.

## Two state machines

### Global state at `MainLoop` struct
Located via the global pointer at `0xc48d0` (loaded by all `MainLoop_*` accessors).
- **+0xc** — `iPodState` (Set at 0x322c, Get at 0x3238). Values observed: **0, 1, 2, 3, 4**.
- **+0x10** — `iPodCancel` (Set at 0x3244, Get at 0x3250). Boolean.
- **+0x14, +0x18** — `iPodMusicID` (Set at 0x325c, Get at 0x3268). 64-bit PID.
- **+0x20** — `selectedTabIndex` (written by `didPickMediaNumber:` via `[self selectedTabIndex]`).

Functions that write `iPodState`:
| Address | Function | Sets state to |
|---|---|---|
| `0x60150` | `iPodView2.mediaPickerDidCancel:` | **4** (also sets cancel=1) |
| `0x60170` | `iPodView2.mediaPicker:didPickMediaNumber:` | **4** (also sets cancel=0) |
| `0x96a0` | (some fn) | 2 |
| `0x17800` | (scene renderer) | 1, 2, 3 |

**iPodState=4 alone does NOT trigger panel render.** Both pick and cancel set state=4 — they're differentiated by `iPodCancel` (+0x10).

### Scene-local state
Function `0x17800` (the scene render function) loads a scene object via `0xc9f4 → r5`, then reads `[r5, +0xc]` and compares against:
- **2** — picker init/show?
- **5** — picker showing
- **9** — **confirmation panel** ← MPMediaQuery + `valueForProperty:title/artist/artwork` reads happen ONLY in this branch
- **15** — animation / transition
- **30** — closing?

Scene constructor at `0x16602` mallocs 88 bytes and writes **state=9** as the initial value (+0xc on the new struct). On pick 1 this scene is fresh → state=9 → render path runs.

## Why pick 2+ doesn't refresh

After pick 1's panel renders, the scene state transitions away from 9 (likely to 5 or 30 as the panel is dismissed). On pick 2:

1. User taps row → our dispatch fires
2. Game's `didSelectRow` → `didPickMediaNumber:` runs path B (NSLog "select!!!! / SONG : ...")
3. `iPodState=4`, `iPodMusicID=newPID`, `iPodCancel=0` set correctly
4. **But scene_obj+0xc is NOT reset to 9** — it stays at whatever pick 1 ended at
5. Next render tick: `0x17800` reads scene state, doesn't match 9, skips the MPMediaQuery branch
6. Panel cells stay frozen on pick 1's content

## What we tried and why each failed

| Approach | Result |
|---|---|
| Cell purge + `reloadData` on IPDSongsTab | Fired, but cellForRow doesn't read song properties — those happen inside scene render, not cell builder |
| Direct `[iPodView2 mediaPicker:nil didPickMediaNumber:item]` via objc_msgSend | Couldn't find iPodView2 (not in view tree; not Apple's MPMediaPickerController delegate) |
| `Set_iPodState(1)` pre-dispatch | iPodState isn't the gate; scene_obj+0xc is |
| `Set_iPodState(1)` on No | Broke cancel flow — game saw lingering state=1 as "user back at picker" and conflicted with hide logic |
| Direct write to `items_` ivar via `setItems:` | Crashed — `items_` is an `NSDictionary` keyed by PID (game does `objectForKey:`), not array |
| Full teardown on No (`dismiss_promoted_picker`) | Game presented its 5-row IPDSongsTab fresh, which is small and doesn't cover the screen — user got stuck on 5-row picker |
| Pop `navigationController` topmost VC on No | `navigationController` returns nil — touchHLE's UIViewController stubs this property |

## What functions to investigate next

### 1. Find caller of scene constructor (0x16602)
We didn't find any direct BL caller of 0x16602. It might be invoked via:
- C++ vtable (function pointer table)
- Scene-stack registration function

Look at function pointer references — search for word `0x16603` (Thumb bit set) in `__data` or `__const` segments.

### 2. Find where scene state transitions OUT of 9
Function 0x17800 (the renderer) writes `iPodState` (the global) to 1/2/3, but probably also writes the scene state at scene_obj+0xc to advance through 9→other. Trace its STR instructions to [r5, #0xc] (where r5 is the scene obj from `0xc9f4` return).

```
At 0x17800: r5 = get_scene_obj()
Then: ldr/str r?, [r5, #0xc] manages the scene state
```

If we can identify the transition `state==9 → state==X`, we could force it back to 9 between picks.

### 3. Directly memory-write scene_obj+0xc = 9 before our dispatch

If we can:
- Call `get_scene_obj()` from our Rust code (function at 0xc9f4) — this is a plain C function, easy to invoke via `GuestFunction::from_addr_and_thumb_flag(0xc9f4, true)`
- Get the returned pointer  
- Memory-write 4-byte value `9` at `[scene_obj + 0xc]`

That would set the scene back to "confirmation" state. Next render tick would fire the MPMediaQuery branch.

**This is the most promising next step.** Pseudo-code in `ui_table_view.rs` picker-swap dispatch:

```rust
use crate::abi::{CallFromHost, GuestFunction};
let get_scene = GuestFunction::from_addr_and_thumb_flag(0xc9f4, true);
let scene_ptr: u32 = get_scene.call_from_host(env, ());
if scene_ptr != 0 {
    let scene_state_addr = MutPtr::<u32>::from_bits(scene_ptr + 0xc);
    env.mem.write(scene_state_addr, 9);
}
```

⚠️ Risks:
- `get_scene_obj` at 0xc9f4 uses a global index — might return a different scene object than the iPod-related one if we call it at the wrong time
- Setting state=9 might trigger render but for the WRONG scene (e.g., main menu)
- Need to confirm the scene returned is actually the iPod confirmation scene
- May need to query MainLoop+0x4 or +0x8 to identify scene type before writing

### 4. Memory-tap approach for runtime confirmation

Add temporary logging to `Set_iPodState` in our code — log the call stack each time iPodState changes. Run the game through pick 1 → No → pick 2 and watch the state transitions. That'll reveal what the scene state machine looks like in practice.

## Key addresses reference

| Address | What |
|---|---|
| `0x322c` | `MainLoop_Set_iPodState(int)` |
| `0x3238` | `MainLoop_Get_iPodState()` |
| `0x3244` | `MainLoop_Set_iPodCancel(int)` |
| `0x3250` | `MainLoop_Get_iPodCancel()` |
| `0x325c` | `MainLoop_Set_iPodMusicID(low, high)` |
| `0x3268` | `MainLoop_Get_iPodMusicID()` |
| `0xc48d0` | Global pointer to MainLoop state struct |
| `0xc9f4` | `get_scene_obj()` — C function returning current scene pointer |
| `0xca54` | Probable malloc (called from scene constructor) |
| `0x16602` | Scene constructor — mallocs 88 bytes, sets new_scene[+0xc]=9 |
| `0x17800` | Scene render function — reads scene_obj[+0xc], renders panel when ==9 |
| `0x5bc78` | `IPDSongsTableVC.getSong:` — reads collections, NSAssert |
| `0x5c761` | `IPDSongsTableVC.tableView:didSelectRowAtIndexPath:` — path A pushes, path B calls didPickMediaNumber |
| `0x5d35d` | `setCollections:` (objc_setProperty wrapper) |
| `0x5fd50` | `IPDMediaQueryUtils.playlistSongs:` (unused — no selrefs) |
| `0x5fef8` | `IPDMediaQueryUtils.albumSongs:` (unused) |
| `0x60150` | `iPodView2.mediaPickerDidCancel:` |
| `0x60170` | `iPodView2.mediaPicker:didPickMediaNumber:` |
| `0x62a85` | `IPDMediaPickerLoadingUtil.loading` (initial library load) |

## Useful selrefs

| C-string addr | Selector |
|---|---|
| `0x680e0` | `items` |
| `0x68134` | `songsQuery` |
| `0x6c164` | `getSong:` |
| `0x6c328` | `items_` |
| `0x6c41c` | `setItems:` |
| `0x6c198` | `mediaPicker:didPickMediaNumber:` |
| `0x6c7c0` | `mediaPickerDidCancel:` |
| `0x6cf60` | `IPDMediaPickerController` (class) |
| `0x6d504` | `iPodView2` (class) |
| `0x6d4a4` | `playlistSongs:` |
| `0x6d4b4` | `albumSongs:` |
| `0x6bb84` | `isLoadedCollections:` |

## Inspect scripts

All in `F:\ios_emu\inspect\`:

- `disasm_getsong.py` — original disasm of pick-related IMPs (existing)
- `disasm_specific.py` — disasm specific addresses (existing)
- `find_imp.py` — find IMP for selector (existing)
- **`re_state_observers.py`** — find readers/writers of MainLoop state struct fields
- **`find_panel_builder.py`** — find functions that call `+songsQuery` (panel renders)
- **`disasm_panel_candidate.py`** — disasm panel candidate functions
- **`find_method_selectors.py`** — find selector names for IMP addresses
- **`find_query_util_callers.py`** — find callers of util selectors (with selref lookup)
- **`dump_didselect2.py`** — decode selectors from a function's literal pool (path A vs B)
- **`dump_dpmn_selectors.py`** — selector dump for didPickMediaNumber
- **`disasm_state_render.py`** — disasm state-readers for render gate detection
- **`find_state_cmp_4.py`** — find `ldr iPodState; cmp #N` patterns
- **`find_state_writes.py`** — find all `str iPodState; const=N` writes
- **`dump_main_loop_setters.py`** — disasm MainLoop_Set_*/Get_* functions in 0x3200-0x3400
- **`find_set_state_calls.py`** — find all BL callers of state setter functions

## Recommendation

Implement option 3 from "what to investigate next": call `get_scene_obj()` at 0xc9f4, then memory-write `[scene_ptr + 0xc] = 9` right before dispatching `didSelectRow` on pick 2+. This should make the scene re-enter the confirmation render branch.

But test carefully — if `get_scene_obj()` returns a non-iPod scene when called outside the right context, writing 9 to it could corrupt unrelated game state.
