# Help: iOS Game Confirmation Panel Doesn't Refresh on 2nd Pick

## Project Context

I'm running an iOS game (**Song Summoner: The Unsung Heroes Encore**, 2009, armv6 iPhone) inside **touchHLE** — a Rust-based high-level iOS emulator I've forked. The game has a music picker UI (pick a song from your iPod library to generate a "Trooper" character). I've added a "picker-swap" layer that exposes the user's local music library to the game and dispatches `tableView:didSelectRowAtIndexPath:` into the game's `IPDSongsTab` delegate when the user taps in my promoted picker.

## The Problem

**Pick 1 works correctly:**
1. User taps a row in the promoted picker
2. Game's `IPDSongsTab.didSelectRow` runs path B: logs `select!!!! / SONG : <artist> , PID : <PID>`
3. Game calls `[iPodView2 mediaPicker:_ didPickMediaNumber:NSNumber(PID)]`
4. That writes `MainLoop.iPodMusicID = newPID`, `iPodCancel = 0`, `iPodState = 4`
5. **Asynchronously**, on the next render tick, the game performs `MPMediaQuery + filter by PID + valueForProperty('title'/'artist'/'artwork')` and renders the confirmation panel with the picked song info

**Pick 2 fails to refresh the panel:**
1. User taps No → game's cancel handler runs → my promoted picker returns
2. User taps a different row
3. Game's `IPDSongsTab.didSelectRow` runs path B identically: logs `select!!!! / SONG : <NEW_artist> , PID : <NEW_PID>`
4. `didPickMediaNumber:` fires identically: writes new MusicID + iPodState=4
5. **Audio preview switches correctly**, **PID is the new song**, **Create Trooper proceeds with the new song**
6. **But: no MPMediaQuery, no valueForProperty reads** — the confirmation panel labels (title/artist/artwork) stay frozen on pick 1's content

The pick is functionally correct — the only failure is the **visible panel labels never refresh**.

## Architecture (from binary reverse engineering)

### Two state machines

**Machine 1 — Global `MainLoop` state struct** (accessed via getter/setter functions)

| Offset | Field | Setter @ | Getter @ |
|--------|-------|----------|----------|
| +0xc | `iPodState` (int) | 0x322c | 0x3238 |
| +0x10 | `iPodCancel` (int) | 0x3244 | 0x3250 |
| +0x14/0x18 | `iPodMusicID` (uint64) | 0x325c | 0x3268 |
| +0x20 | `selectedTabIndex` | (written inline) | — |

Observed `iPodState` values: 0, 1, 2, 3, 4. Both `didPickMediaNumber:` (pick) and `mediaPickerDidCancel:` (cancel) write `iPodState=4` — they're differentiated by the `iPodCancel` flag (+0x10).

**Machine 2 — Per-scene state struct**

The game has a **scene table** at fixed address **`0x1134f0`** — array of 16 entries, each 0x1c (28) bytes. The current scene index is stored at **`0x1136b0`** (uint32). Each entry has a scene struct pointer at `+0x18`.

`get_scene_obj()` at 0xc9f4 is a C function:
```c
void* get_scene_obj() {
    uint32_t idx = *(uint32_t*)0x1136b0;
    return *(void**)(0x1134f0 + idx * 0x1c + 0x18);
}
```

Inside each scene struct, **offset `+0xc` is the scene's local state**. The iPod-related scene's state can be 2, 5, 9, 15, or 30:
- **9 = confirmation panel render** — function at 0x17800 reads `scene+0xc`, branches on `== 9`, and only then calls `MPMediaQuery + songsQuery + filter PID + valueForProperty(title/artist/artwork)`. **This is the panel build that fails on pick 2+.**

The scene constructor at **0x16602** mallocs 88 bytes and initializes `new_scene+0xc = 9`. Direct callers of 0x16602 not found via BL search (probably called via C++ vtable or function-pointer table).

### Pseudocode of the critical functions

```c
// iPodView2.mediaPicker:didPickMediaNumber: at 0x60170
- (void)mediaPicker:(id)picker didPickMediaNumber:(NSNumber*)mediaNumber {
    uint64_t pid = [mediaNumber longLongValue];
    uint32_t tabIdx = [self selectedTabIndex];
    MainLoop_state->tabIdx = tabIdx;          // +0x20
    MainLoop_Set_iPodMusicID(pid);             // +0x14/+0x18
    MainLoop_Set_iPodCancel(0);                // +0x10
    MainLoop_Set_iPodState(4);                 // +0xc
}

// Scene render at 0x17800 (called from main loop, fires per frame)
void render_ipod_scene() {
    void* scene = get_scene_obj();             // 0xc9f4
    int scene_state = *(int*)((char*)scene + 0xc);
    if (scene_state == 2)  { /* picker init */ }
    if (scene_state == 5)  { /* picker showing */ }
    if (scene_state == 9)  {
        // *** PANEL BUILD — MPMediaQuery + valueForProperty reads ***
    }
    if (scene_state == 15) { /* animation */ }
    if (scene_state == 30) { /* closing */ }
}

// IPDSongsTableVC.tableView:didSelectRowAtIndexPath: at 0x5c761
- (void)tableView:tv didSelectRowAtIndexPath:ip {
    int flat = [ip row] * 5 + [ip section];
    if (![self isLoadedCollections:flat]) return;
    [tv deselectRowAtIndexPath:ip animated:YES];
    [tv setAllowsSelection:NO];
    [self threadStop];
    id song = [self getSong:flat];

    if ([song isKindOfClass:CollectionCls] && [song count] > 1) {
        // PATH A — drill into sub-list (not taken when user picks a single song)
        UIViewController* vc = [[IPDSongsListTableViewController alloc] initWithCollection:song naviTitleName:title];
        [[self navigationController] pushViewController:vc animated:YES];  // nav is NIL in touchHLE!
        [vc release];
    } else {
        // PATH B — single song pick (FIRES on every pick)
        NSLog(@"select!!!!");
        id item = [[self items] objectForKey:[NSNumber numberWithInt:flat]];
        NSLog(@"SONG : %@ , PID : %llX", [item artist], [[item pid] longLongValue]);
        [self.delegate mediaPicker:self.delegate didPickMediaNumber:[item pid]];
    }
}
```

## Smoking-gun logs

**Pick 1 (works):**
```
Picker-swap: dispatching row 4 (PID 7D40...) → IPDSongsTab
select!!!!
SONG : WolfStriker , PID : 7D403EE95470141D
[touchHLE reloadData]
[audio preview starts for song #4]
MPMediaQuery +songsQuery               ← PANEL BUILD STARTS
MPMediaQuery alloc/init/addFilterPredicate
collections filter PID=7D40... → indices=[4]
valueForProperty:'title' on song_index=4
valueForProperty:'persistentID'
valueForProperty:'playbackDuration'
valueForProperty:'genre'
(another MPMediaQuery for additional fields)
valueForProperty:'playCount'
valueForProperty:'title'
valueForProperty:'artwork'
valueForProperty:'artist'               ← PANEL BUILT
```

**Pick 2 (broken — after No tap):**
```
Picker-swap: dispatching row 6 (PID 8B7A...) → IPDSongsTab
select!!!!
SONG : Logic;Alessia Cara;Khalid , PID : 8B7A8A3C9E81CF81
[touchHLE reloadData]
[audio preview starts for song #6]
[NOTHING — no MPMediaQuery, no valueForProperty reads]
```

The MPMediaQuery + property reads happen ASYNCHRONOUSLY after the touch handler returns. They fire from the scene render function (0x17800) when it observes `scene_obj+0xc == 9`. On pick 2 the scene state isn't 9, so the render skips that branch.

## Hypothesis we've validated

The render is **edge-triggered**, not continuously observed. The render function only does work when state CHANGES between frames. On pick 1, scene state happened to be ≠9 prior (fresh scene init), so when it became 9, an edge fired. On pick 2 the scene state had advanced past 9 (to 5/15/30), and after the dispatch it's still in that advanced state — `didPickMediaNumber:` only writes the global `iPodState`, not the scene-local state at `scene+0xc`.

## Failed approaches (and WHY each failed)

1. **`reloadData` + cell-cache purge on IPDSongsTab** — cells re-fire `cellForRow` but that doesn't trigger the MPMediaQuery (which lives in scene render, not cell builder)

2. **Pre-dispatch `Set_iPodState(1)` to force a 1→4 edge on the global state** — the render gate is on `scene+0xc`, not `iPodState`; setting global state didn't help

3. **Post-dispatch explicit `Set_iPodMusicID + Set_iPodCancel(0) + Set_iPodState(4)`** (mimicking didPickMediaNumber) — same reason

4. **Direct call to `[iPodView2 mediaPicker:nil didPickMediaNumber:newItem]`** — couldn't find `iPodView2` instance. Game uses its own `IPDMediaPickerController` (not Apple's), so my `MPMediaPickerController.setDelegate:` hook never fires. Walking keyWindow's view tree didn't find an `iPodView2` view either

5. **Pop the topmost VC off the nav controller** so the next pick pushes fresh — touchHLE's `UIViewController.navigationController` is a stub returning nil. Path A's `pushViewController:animated:` is effectively a no-op in this emulator

6. **Write `items_` ivar via `[songs_delegate setItems:array]`** — **CRASHED**: `items_` is an `NSDictionary` keyed by PID NSNumber (game does `[items_ objectForKey:pid]`), not an `NSArray`. Passing an array made `objectForKey:` panic

7. **`Set_iPodState(1)` on the No tap** to force a state transition before the next pick — broke the dismissal flow; the game's main loop saw lingering state=1 as "show picker" and the confirmation page never appeared again

8. **Full teardown on No** (`dismiss_promoted_picker` instead of un-hide) — game re-presented its small 5-row `IPDSongsTab` (not our 1317-row promoted view), user got stuck on the small picker

9. **Write `9` into `get_scene_obj() + 0xc` before each dispatch** — log confirmed the write happened, but panel still didn't refresh AND game got stuck rendering the confirmation panel (couldn't tap past Create Trooper). The scene returned by `get_scene_obj()` at touch handler time may not be the iPod confirmation scene — and locking it at 9 prevents natural progression

10. **Deferred 2-frame edge architecture (Option C)**: `Set_iPodState(1)` immediately in the touch handler + stash all dispatch params in a `Mutex<Option<PendingPickerSwap>>` global + drain from `NSRunLoop`'s main-loop tick (the same hook that fires `media_player::handle_players`). Drainer (one tick later) ran reloadData + `didSelectRow` dispatch + `play_song` + setHidden. **Result: drain fired, didSelectRow ran, game logged new PID, audio switched — but the picker stayed visible (didn't dismiss to confirmation) and no panel refresh occurred.** Setting iPodState=1 as the intermediate value confused the game's main loop ("show picker" interpretation conflicted with the dismissal that should follow the dispatch)

## Current Working Theory

**The visible panel is controlled by the scene-local state at `scene_obj + 0xc`, NOT by `iPodState`.** The global iPodState=4 edge is irrelevant; what matters is whether the iPod confirmation scene's local state transitions to 9.

We can't just write 9 at `get_scene_obj() + 0xc` — that returns the *currently-active* scene, which during a touch handler might be the picker scene or another scene, not the iPod confirmation scene. Writing 9 to the wrong scene locks it stuck rendering as if it were the confirmation panel.

## Refined hypothesis: missing teardown/rebuild lifecycle (not edge)

The first attempt at a per-tick observer crashed at boot with `idx=16` (scene-table "no scene" sentinel) and `iPodState=0xFFFFFFFF` (MainLoop singleton ptr=0 — uninitialized). The crash itself was `null-page access at 0xe` in mem.rs — likely the game's own code touching an uninitialized MainLoop field, not my observer code.

The takeaway: **the scene system has a strict init order**. Each scene gets a fresh constructor on entry, state=9 only on creation, and the scene must be **torn down** for state=9 to be reachable again. My promoted picker preserves the same `IPDSongsTab` across the natural teardown the game wants to do — so the scene never reconstructs and the panel-build code path never re-enters.

Evidence supporting this:
- Pick 1 always works (scene freshly constructed → state 9 → panel build)
- Pick 2 audio/PID updates correctly (didPickMediaNumber:'s side effects still fire)
- Pick 2 panel labels never refresh (scene was never reconstructed → state 9 never re-entered)
- Forcing scene+0xc=9 locks the WRONG scene (wrong target — `get_scene_obj()` returns current scene, not iPod scene)
- Full teardown on No restores natural behavior but loses the promoted 1317-row picker
- The boot crash proves the game expects strict init ordering

The "edge-triggered" hypothesis explained the surface symptom; the root cause is the lifecycle bypass.

## Current instrumentation (just deployed — paste log below)

**1. `+songsQuery` hook (MPMediaQuery class method)** — fires at the EXACT moment scene+0xc==9 is observed, because 0x17800 calls `+songsQuery` only in its state-9 branch. Snapshots scene & MainLoop state plus a 16-word window of the scene struct:
```
+songsQuery FIRED: scene_idx=K entry5.scene=0xADDR scene+0xc=STATE | iPodState=S iPodCancel=C iPodMusicID=HHHHLLLL
+songsQuery scene+0x00..+0x3c = [ ... 16 hex words ... ]
```

**2. Boot-safe per-tick observer** — gated on MainLoop pointer being non-null AND scene_index < 16. Logs scene-table entry 5 + MainLoop state on every change.

Expected result from pick 1 → No → pick 2:
- Pick 1: `+songsQuery FIRED` fires (multiple times) with `scene+0xc=9` and a full scene struct dump
- Pick 2: `+songsQuery` does NOT fire — confirming the state-9 branch was never re-entered. Observer shows scene state stays in some non-9 value.
- The state-9 scene struct dump from pick 1 vs the scene snapshot at pick-2 dispatch identifies WHICH field changes between successful (state 9) and stuck (non-9).

## What I Need from You

Given the above, please propose:

1. **How should I identify the iPod confirmation scene unambiguously** from a scene-table dump? Other than `scene+0xc ∈ {2,5,9,15,30}`, what fingerprints would the iPod scene have? (e.g. specific scene-table entry index, distinctive function-pointer values in entry+0x00..+0x18, distinctive scene-struct fields beyond +0xc)

2. **When should I write `scene+0xc = 9`** — before or after the dispatch? Should I do it synchronously in the touch handler, or one tick later via the run-loop drainer?

3. **What adjacent fields likely need clearing** alongside `scene+0xc = 9`? Common patterns in 2009-era mobile engines for "dirty flag", "needs refresh", "frame counter", "transition cookie", "last-rendered state cache" — what offsets should I probe?

4. **Should I look for the scene constructor's caller** (0x16602)? If I can find that function-pointer table or vtable, I could trigger a fresh scene construction instead of patching an existing one. How would I find such a vtable in an armv6 Mach-O — search `__data` / `__const` for word patterns containing 0x16603 (Thumb-bit set)?

5. **Is there a cleaner architectural fix** I'm missing? Real iOS would have:
   - Touch event → run loop → game tick → render
   And maybe in the real OS, the run loop fires multiple ticks between user actions, so the scene state naturally re-initializes. In touchHLE we have a single-threaded run loop that compresses these. Should I be implementing a different abstraction to give the game multiple ticks between picks?

## Capabilities Available in My Emulator

- Call any guest function: `GuestFunction::from_addr_and_thumb_flag(addr, thumb).call_from_host(env, args)` (works for plain C, Thumb-2)
- Read/write guest memory: `env.mem.read(Ptr)` / `env.mem.write(Ptr, value)`
- Send Obj-C messages: `msg![env; receiver selector:arg]`
- Hook any selector / message dispatch
- Add per-tick callbacks to the main run loop

## Key Binary Addresses

| Addr | What |
|------|------|
| `0x322c` | `MainLoop_Set_iPodState(int)` |
| `0x3238` | `MainLoop_Get_iPodState()` |
| `0x3244` | `MainLoop_Set_iPodCancel(int)` |
| `0x325c` | `MainLoop_Set_iPodMusicID(uint32 lo, uint32 hi)` |
| `0xc48d0` | Address of pointer to MainLoop state struct |
| **`0xc9f4`** | **`get_scene_obj()` — returns current scene struct ptr** |
| **`0x1134f0`** | **Scene table base (16 entries × 0x1c bytes; scene ptr @ entry+0x18)** |
| **`0x1136b0`** | **Current scene index (uint32)** |
| `0x16602` | Scene constructor — mallocs 88 bytes, writes 9 into new scene+0xc |
| `0x17800` | Scene render function — reads scene+0xc, branches; calls MPMediaQuery + valueForProperty when state==9 |
| `0x5c761` | `IPDSongsTableVC.tableView:didSelectRowAtIndexPath:` |
| `0x60150` | `iPodView2.mediaPickerDidCancel:` (sets state=4, cancel=1) |
| `0x60170` | `iPodView2.mediaPicker:didPickMediaNumber:` (sets state=4, cancel=0, new MusicID) |
| `0xca54` | Likely malloc (called from scene ctor) |

## Selectors / Classes

- Classes (game): `iPodView2`, `IPDMediaPickerController`, `IPDSongsTab`, `IPDSongsTableViewController`, `IPDSongsListTableViewController`, `IPDMediaQueryUtils`, `IPDMediaPickerLoadingUtil`
- Game custom selectors: `mediaPicker:didPickMediaNumber:`, `mediaPickerDidCancel:`, `getSong:`, `isLoadedCollections:`, `playlistSongs:`, `albumSongs:`
- ivar: `items_` is an `NSDictionary` keyed by PID NSNumber, **not** an array

## Updated confidence ranking

- **Missing teardown/rebuild lifecycle**: **75%** (strongest after boot-crash analysis)
- Wrong scene pointer being returned by `get_scene_obj()` at touch handler time: **60%**
- Adjacent dirty-flag fields near `scene+0xc` need clearing: **40%**
- Edge-triggered observer model (symptom, not cause): **30%**
- Hidden refresh method we haven't found: **20%**
- Implementing `navigationController` would fix it: **5%** (path B fires on every pick regardless)

## What I'd like ChatGPT to do

Given the +songsQuery hook log (which will appear below), please:

1. **Identify which scene-struct fields differ between successful state-9 entry (pick 1) and the stuck state at pick-2 dispatch.** These are the dirty/cache flags I need to clear.
2. **Propose the safest write strategy** (which fields, in what order, before or after my dispatch, synchronous or deferred).
3. **Suggest signatures to find the iPod scene's destructor/teardown function** in the armv6 Mach-O binary (so I can find what's NOT being called between picks).
4. **Suggest an architectural alternative**: if I should let the game's natural lifecycle run (let scene tear down on No, re-promote my 1317-row table when the game re-presents the picker), what signals identify "picker is fully reconstructed and ready for me to inject again"?

## Observer + +songsQuery log

[PASTE LOG BELOW — grep for `ipod_observer` and `+songsQuery FIRED` lines from run.log after pick 1 → No → pick 2]
