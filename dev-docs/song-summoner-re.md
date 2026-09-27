# Song Summoner: reverse-engineering notes (picker + input)

Static analysis of the thin armv6 `S.S.Encore` binary (the only slice in
the IPA). The binary keeps its **full C++ symbol table** and ObjC metadata,
so nearly every function has a real name. Tools: `inspect/mo.py`,
`inspect/ssdis.py`, `inspect/xref.py` (see "Tools" at the end).

This doc replaces the "scene frame counter" model in
`song-summoner-picker.md` and `inspect/RE_FINDINGS.md`. Those docs got
the addresses right but misread what the fields mean.

## 1. The game never uses `MPMediaPickerController`

The picker is the game's own code, compiled from
`Classes/IPD/*.mm` (paths are in NSAssert strings). It is built from
stock UIKit parts:

| Class | Superclass | Role |
|---|---|---|
| `IPDMediaPickerController` | NSObject | Builds a `UITabBarController` with 4 tabs, each in a `UINavigationController`. Its view gets tag `0x123`. |
| `iPodView2` | IPDMediaPickerController | The instance the game creates. It is **its own delegate**. |
| `IPDSongsTab` / `ArtistsTab` / `AlbumsTab` / `PlaylistsTab` | `IPD*TableViewController` → `IPDTableViewController` → `UITableViewController` | Tab contents |
| `IPDSongsListTableViewController` | IPDTableViewController | Pushed when a row is a multi-song collection |
| `IPDMediaPickerLoadingView` | UIView | "Loading" splash. It runs the library queries on a thread. |
| `IPDMediaItemCollection` | singleton | Caches `songs/albums/artists/playlists` collections |
| `NSNCThread` | NSThread | Background row loader used by every table |
| `Music`, `MusicLibraryCell` | | Row model + custom-drawn cell |

`MPMediaPickerController` does not appear anywhere in the binary. The
existing `PICKER_DELEGATE`/`PICKER_CONTROLLER` capture in
`media_picker_controller.rs` never fires for this game.

## 2. The picker contract (this is all a replacement needs)

Everything is driven by one global, `iPodState`, in the `MainLoop`
struct at `0xc48d0`:

| Field | Accessors | Meaning |
|---|---|---|
| `+0x0c` iPodState | `Set 0x322c` / `Get 0x3238` | 0 idle, 1 open, 2 re-show, 3 close, 4 result ready |
| `+0x10` iPodCancel | `Set 0x3244` / `Get 0x3250` | 1 = user cancelled |
| `+0x14/+0x18` iPodMusicID | `Set 0x325c` / `Get 0x3268` | u64 persistent ID |
| `_savedat+0x20` | | Last selected tab (0–3), passed back to `initWithSelectionIndex:` |

(All accessors are Thumb, even though the symbol table lacks the Thumb
flag for them.)

### Who writes what (every call site in the binary)

```
Palace_Main  step 10  -> iPodState=1, cancel=0     (open picker)
Palace_Main  step 13  -> iPodState=2, cancel=0     ("No" on confirm: re-show)
Palace_Main  step 15  -> iPodState=3               (cancel path: close)
Palace_Main  0x18962, 0x189e4 -> iPodState=3       (later steps; probably
                                                    Create Trooper / Back,
                                                    not yet traced)
MainView.mainLoop     -> iPodState=2 right after open_iPod, 0 after close
iPodView2 mediaPicker:didPickMediaNumber: -> MusicID=pid, cancel=0, state=4
iPodView2 mediaPickerDidCancel:           -> cancel=1, state=4
```

### `-[MainView mainLoop]` (0x96a0) is the whole state machine

`m_mode` (ivar `+0x24`) is 0 while the game runs and 1 while the picker
is up. **While `m_mode == 1`, `MainLoop_Main()` is not called at all.**
The C++ game is frozen until the picker reports a result.

```
m_mode == 0:
    MainLoop_Main()                          // game ticks
    state 1 -> [self open_iPod]; m_mode = 1; state = 2
    state 3 -> [self close_iPod]; state = 0
    state 2 -> [vm enable_iPodView]; [vm rotate_iPodView:angle]; m_mode = 1
m_mode == 1:                                 // game frozen
    state 3 -> [self close_iPod]; m_mode = 0
    state 4 -> [vm disable_iPodView]; m_mode = 0   // hide, resume game
```

`ViewManager` (the root VC) does the UIKit work:

| Method | Does |
|---|---|
| `start_iPodView` (0x31b98) | Adds `IPDMediaPickerLoadingView` (delegate = ViewManager) to the window. Clamps `_savedat+0x20` to 0..3. |
| `animationDidStop` (0x31b00) | Called by the loading view when done: `ipodview = [[iPodView2 alloc] initWithSelectionIndex:tab]`, `[window addSubview:[ipodview getView]]`, then rotate |
| `enable_iPodView` / `disable_iPodView` | `[ipodview setHidden:NO/YES]` |
| `rotate_iPodView:` | `[ipodview rotate:angle]` |
| `end_iPodView` (0x31a00) | For each window subview with `viewWithTag:0x123`, `removeFromSuperview`. Then `[ipodview release]` |

### What the game does with the result (`Palace_Main`, 0x17800)

The Palace scene work struct comes from `SysTask_Get_WorkAdr()` (0xc9f4,
called `get_scene_obj` in older notes). **`+0x0` is the step number**
(a 41-way `___switch16`), `+0x8` is a sub-step, and `+0xc` is a
per-sub-step frame timer. It is not a frame counter with keyframes.

- **Step 14** runs on the first tick after the game unfreezes. If
  `iPodCancel == 1` it goes to step 15 (fade out, `iPodState=3`,
  back to step 5, the Palace menu). Otherwise it reads `iPodMusicID` and
  runs `[MPMediaQuery songsQuery]` + `MPMediaPropertyPredicate
  (PersistentID == pid)` → `collections` → `items` to build the
  confirmation panel (title/artist/artwork).
- Steps 11–13 ("No"): replay the anim, `Palace_Disp(0,0)`, fade, then
  `iPodState=2` so mainLoop un-hides the **same** picker.

This explains the old bugs. The previous hack wrote `scene+0x0 = 0x0e`
to force a rebuild, which re-entered step 14 without going through the
teardown in steps 11–13. So the panel sprites were built again on top
of the old ones ("text stacking"). The touch-coordinate No/Back
classifier was only needed because the promoted table covered the GL
view and swallowed its touches. **If the picker just calls the delegate
and leaves hide/show to the game, both problems go away.**

### The minimal contract, restated

1. Present something when `-[IPDMediaPickerController getView]` is asked
   for a view. It must have tag `0x123` so `end_iPodView` removes it.
2. On pick: `[ipodview mediaPicker:ipodview didPickMediaNumber:@(pid)]`
   (an `NSNumber`; the game calls `longLongValue` on it).
   On cancel: `[ipodview mediaPickerDidCancel:ipodview]`.
3. `setHidden:` / `hidden` / `rotate:` must work on that view. The game
   calls them; it never needs anything else from the picker.
4. `selectedTabIndex` must return 0..3 (it's stored and asserted ≤ 3).
5. The PID must round-trip through `MPMediaQuery songsQuery` +
   PersistentID predicate. `media_query.rs` already supports this.

## 3. Proposed fresh implementation

### 3a. Generic hook: host overrides for guest methods

touchHLE has no way to replace a method defined in the app binary.
The cleanest fix is a small, generic one:

- After `ObjC::register_bin_classes` (`src/objc/classes.rs:617`), apply
  a table of `(class_name, selector, &'static dyn HostIMP)` overrides by
  doing `ClassHostObject.methods.insert(sel, IMP::Host(imp))`.
- Skip any entry whose class the app doesn't define, so this is inert
  for every other app.
- `substitute_classes()` in the same file is the existing precedent for
  per-app class fixes.

### 3b. Override `IPDMediaPickerController` (6 methods)

Implement these on the base class, so `iPodView2` inherits them:

| Selector | Host implementation |
|---|---|
| `initWithSelectionIndex:` | Build a host `SSMusicPickerView` (480×320, tag `0x123`) and remember it keyed by `this` in host state. Return `this`. |
| `getView` | Return that view |
| `setHidden:` / `hidden` | Forward to the view. On un-hide, reset the scroll or highlight if wanted. |
| `rotate:` | No-op or a transform. The game is landscape-only. |
| `selectedTabIndex` | 0 (or the remembered tab if tabs are added) |
| `dealloc` | Release the view, drop the host state, then call super |

`SSMusicPickerView` is a host UIView containing a UITableView over
`music_library`'s host songs plus a Cancel button. Its only outputs are
the two delegate calls in §2. It needs no statics, no `ns_run_loop`
drain, no touch classification, and no memory writes.

Optional: also override `-[ViewManager start_iPodView]` to skip the
threaded loading splash and call `[self animationDidStop]` directly.
That removes the `NSNCThread`/`performSelectorOnMainThread` dependency.
It also skips `IPDMediaItemCollection` loading, which only the IPD
tables use.

### 3c. What can then be deleted

- `ui_table_view.rs`: promoted table, `PROMOTED_TABLE`,
  `IPD_SONGS_TABLE`, `PENDING_PICKER_SWAP`, remount/dismiss/drain,
  `invoke_ipodview_reset`, the scene observer, and the HUD re-skin of
  foreign tables.
- `ui_touch.rs`: the confirmation-panel button classifier (~150 lines).
- `ns_run_loop.rs`: the per-tick picker drain and scene observer.
- `media_query.rs`: the `+songsQuery` instrumentation and the
  picker-swap collection filtering.
- `media_picker_controller.rs`: `PICKER_DELEGATE`/`PICKER_CONTROLLER`.
- `gles_guest.rs`: the texture tracker, which was only for panel
  debugging.

### 3d. MediaPlayer surface the game still needs

- Query: `songsQuery`, `albumsQuery`, `artistsQuery`, `playlistsQuery`,
  `addFilterPredicate:`, `collections`, `items`, `representativeItem`,
  `predicateWithValue:forProperty:comparisonType:`.
- Properties: Title, Artist, AlbumTitle, AlbumArtist, Artwork
  (`imageWithSize:`), PersistentID, Genre, PlayCount, PlaybackDuration.
  `MainLoop_Check_id2fighter(u64)` and `getGraphicNo(u64)` derive the
  trooper from the PID.
- Playback: `+[MPMusicPlayerController applicationMusicPlayer]`,
  `setQueueWithItemCollection:`, `play`, `stop`, `setVolume:`/`volume`,
  `setCurrentPlaybackTime:`, `setRepeatMode:`, `setShuffleMode:`.
  Callers: `Palace_iPodPlayer_Play` (0x16a7c), `Result_iPodPlayer_Play`,
  `Ending_iPodPlayer_Play`.
- `SysiPodList_Get_SongCount` (0x42d20) runs before step 10. It decides
  whether the iPod option is offered at all.

### 3e. Fighter portraits

The game's own list fills `-[Music setFid:]` from
`IPD_ID2Fighter(pid)` (0x650a8, called from
`-[IPDSongsTableViewController nowLoading]` and the
`IPDSongsListTableViewController` one), and
`-[MusicLibraryCell fighterImageDraw]` (0x5dc84) draws
`list_fighter%03d.png` at (260, 0, 200×55):

| `fid` | Image | Meaning |
|---|---|---|
| -2 | none | |
| -1 | 089, teal music note | never summoned (the default) |
| 999 | 087, "MUSIC FIGHTER" | summoned, but its look changed since |
| 998 | 088, "?" | |
| other | that number | the summoned trooper |

- `MainLoop_Check_id2fighter(pid)` (0x3352): -1 for pid 0. Otherwise
  `g = getGraphicNo(pid)`, then it scans 80 roster slots of 0xf8 bytes
  at `savedat + 0x31b8` (u64 pid at +0, in-use flag at +8, graphic at
  +0x14). Pid match and `g == 998` → 998; pid match and stored graphic
  `== g` → g; pid match with another graphic → 999 once the scan ends;
  no match → -1.
- `IPD_ID2Fighter` passes -1/999/998 through and subtracts 10 from
  anything else: `getGraphicNo` numbers start at 10, portraits at 0.
- `getGraphicNo` (0x33340) sums the pid's 8 bytes mod 73 and maps
  that through `unitgrapchangeptb` for the current palace level
  (`ScriptFlag_Get_PalaceValue`). That's why a trooper's look can
  change later.
- The picker calls `IPD_ID2Fighter` directly (`picker_view.rs`) and
  clears its cache each time it opens, so a new summon shows up the
  next time the picker opens.

## 4. Controller injection: useful hooks  🎮

### The single input funnel: `_touch_work` @ `0x1226b4`

`-[MainView touchesBegan/Moved/Ended/Cancelled:]` only forward to
`SysTouch_*` C++ functions, which fill one struct. **All gameplay reads
touch through getters on this struct**:

| Getter | Addr | Call sites |
|---|---|---|
| `SysTouch_Get_EndedState` (reads `+0x58`) | 0xd6f8 | 153 in 84 functions |
| `SysTouch_Get_PosData(CGPoint*,CGPoint*,int*)` | 0xd71c | 93 in 55 |
| `SysTouch_Get_FlickState` | 0xd6c8 | 52 in 29 |
| `SysTouch_Get_BeganState` | 0xd6ec | 18 in 17 |
| `SysTouch_Get_TouchState`, `_CountState`, `_DoubleClick`, `_FingerData`, `_PrevData` | 0xd6bc… | |

Known layout (from `SysTouch_Began_F1` 0xd99c / `SysTouch_Clear` 0xd69c):
`+0x0c` finger count (≤2), `+0x10` began, `+0x18/+0x1c` tap count,
`+0x20` extra arg, `+0x24/+0x28` current x/y (float), `+0x2c/+0x30`
start x/y, `+0x54` touching, `+0x58` ended. The writers are
`SysTouch_Began_F1/F2` (0xd99c/0xd94c), `Moved_F1/F2` (0xd5e0/0xd608),
`Ended_F1/F2` (0xd640/0xd668), and `SysTouch_Main` (0xd9d8), which
derives flick/double-click per frame.

**Injection options, best first:**
1. **Call the `SysTouch_Began/Moved/Ended_F1` functions from the host**
   (`GuestFunction::from_addr_and_thumb_flag(addr, true)`), just before
   `MainLoop_Main`. This is deterministic, bypasses UIKit, and makes
   flick/double-click logic run normally.
2. Keep synthesizing UIKit touches (today's `button_to_touch`). This is
   simpler but depends on the window/orientation mapping.
3. Patch function entries to jump to host code. touchHLE already routes
   imported functions through SVC trampolines (`src/dyld.rs:199-246`),
   so writing an SVC stub over a guest function's first instructions is
   feasible.

### Semantic hooks (for "real" controls, not fake taps)

| Symbol | Addr | Why it's useful |
|---|---|---|
| `SysMessage_Next(int)` / `SysMessage_Get_ComState` | 0xacb8 / 0xb598 | Advance dialog = A button |
| `SysMessageKeyMark_*` | 0xad9a… | The "tap to continue" marker. It tells you a dialog is waiting. |
| `SysButtonMenu_Open/Position/Size/Check/Disp/Close` | 0x546d4… | The generic button menu. Position/Size give every button rect, so D-pad focus can move between them without hard-coded coordinates. |
| `SysTouch_Check_InRect(CGPoint*, …)` (3 overloads) | 0xd7d0/0xd83a/0xd8ba | Hit tests. Few callers (Option, TacticsUnitSelect), so mostly bespoke. |
| `TownMenu_Ctrl`, `WorldmapMenu_Ctrl`, `WorldmapHensei_Ctrl`, `TownHensei_Ctrl` | | Per-screen menu controllers |
| `TacticsUnitMenu_CtrlTop/Item/Skill/Status`, `TacticsMoveSelect_CtrlTouch/Flick/Release/Cancel`, `TacticsUnitSelect_CtrlBegan/Release`, `Tactics_CtrlTest` | | Battle input handlers. `TacticsMapCursor_Check` shows a map cursor already exists. |
| `TacticsMap_Touch_Screen2Map` / `TouchFast_Screen2Map/Unit` | | Screen ↔ tile conversion, for placing a virtual cursor on tiles |
| `Keyboard_Main` / `getkeybord(CGPoint*)` | 0x4d1d0 / 0x4d0e8 | On-screen password keyboard |
| `SysTask_Get_WorkAdr()` | 0xc9f4 | Current scene's work struct (`+0x0` = step) |
| `MainLoop_Set_NextPhase(int)` | 0x31f0 | Scene switch. Hook it to learn which screen is active. |
| `MainLoop_getgravitycontrol` / `set/getcurrentangle` | 0x3344 / 0x33d4 / 0x3338 | Tilt control + screen angle (from `accelerometer:didAccelerate:`) |
| `MainLoop_Get_Interruption` | 0x3208 | Pause flag checked by scenes |
| `MainView.mainLoop` `m_mode` | ivar `+0x24` | 1 = UIKit picker is up. Route controller input to the picker, not the game. |

Suggested approach: hook `MainLoop_Set_NextPhase` plus
`SysTask_Get_WorkAdr()+0` to know the current screen and step. Harvest
button rects from `SysButtonMenu_Position/Size` for D-pad focus. Map A
to `SysMessage_Next` when a KeyMark is shown, otherwise to a tap at the
focused rect.

### Frame loop and injection (confirmed 2026-09-27)

- `-[MainView mainLoop]` (0x96a0) is the frame callback. With
  `m_mode == 1` (picker up) it skips the scene; otherwise it calls
  `MainLoop_Main` (0x3418).
- `MainLoop_Main`: if `_mainloop_work+0` (the next phase, set by
  `MainLoop_Set_NextPhase`) is non-zero it opens that scene
  (`SysTask_Open(Init, Main, Exit)`) and stores the scene number in
  `_savedat+0`. Then, every frame: `SysTask_Main` (the scene, which reads
  touch), `SysFile_Main`, **`SysTouch_Main`**, textures, anims, prims,
  `SysMessage_Main`.
- `SysTouch_Main` clears `+0x54` (began) each frame, and after `+0x58`
  (ended) calls `SysTouch_Clear`. **So a synthetic tap must put Began
  and Ended before different frames**, or the scene never sees the Began.
- Signatures (from the `touches*` callers): `Began_F1(CGPoint *pos,
  CGPoint *prev, int fingers, int tapCount)`, `Moved_F1(pos, prev,
  fingers)`, `Ended_F1(pos, fingers)`. Points are `locationInView:` of the
  MainView, whose bounds are set to **480×320 landscape**, the same space
  as the picker.
- Scene numbers `MainLoop_Main` stores in `_savedat+0`: 1 Bumper, 2 Title,
  3 ListeningPoint, 5 Tactics, 6 Result, 7 Worldmap, 8 Catalog2, 9 Town,
  10 Palace, 11 Shop, 12 Colosseum, 13 Ending, 14 StageSelect (15+ are
  test scenes). But `_savedat` is the **save data**, so after a load `+0`
  is the saved location, not the current scene. The scene task's main
  function (task table) is the reliable way to tell the current scene.
- `_savedat`, `_touch_work`, `_mainloop_work` and the `SysTouch_*`
  functions are all in the symbol table, so touchHLE looks them up by name
  (`game_input.rs`), not by address.

### Task table and `SysButtonMenu` (confirmed 2026-09-27)

- `_task_manage`: 16 slots of 0x1c bytes. `+0x0` state (2 = running),
  `+0xc` init, `+0x10` main, `+0x14` exit (function pointers with the
  Thumb bit), `+0x18` work struct. `SysTask_Open` takes the **first free**
  slot, so table order isn't opening order. `SysTask_Main` runs slots 0–15
  each frame with `_task_play_num` set to the slot.
- A `SysButtonMenu` is a task whose main is `SysButtonMenu_Main`. Its work
  struct: `+0x0` state (1 loading, 2 accepting input, 3 closing), `+0x4`
  Check's sub-state, `+0xc` button count, `+0x14` pressed button, `+0x18`
  / `+0x1a` position x/y and `+0x1c` / `+0x1e` button w/h (`short`s),
  `+0x20` "tap outside cancels", `+0x28` cancel-button prim (-1 if none),
  `+0x2c` → `int enabled[count]`, `+0x30` → prim ids, `+0x34` → label
  message ids.
- Layout: a column centred on (x, y). Button i is centred at (x,
  y − (h·(count−1))/2 + i·(h+2)) (C integer division), size w×h.
- `SysButtonMenu_Check(menu)` on a touch **ended**: a hit on an enabled
  button plays SE 9, animates it for 5 frames, then returns its index. A
  disabled one plays SE 1. A miss with "tap outside cancels" on (and no
  flick) plays SE 3 and returns −2 (cancel). Otherwise it returns −1.
- `SysButtonMenu_Enable_CancelButton` adds a cancel icon (`tc_icon.png`,
  48×48, centred at (456, 296)) as the prim at `+0x28`. Tapping it plays
  SE 9, animates it for 5 frames, and Check returns −3. Note that
  `SysPrim_Touch_DrawRect` returns **0 on a hit**. The Hip-O-Drome menu
  (`PalaceFlow_TutorialMenu`) is a `SysButtonMenu` with this icon.
- `Title_Open_StartMenu` opens a `SysButtonMenu` at (240, 212), buttons
  160×54, with `Enable_OutrangeCancel`. That's not the menu after "press
  start", though (see `SysDrum2`).

### `SysDialog`, message boxes with buttons (confirmed 2026-09-27)

- `SysDialog_Open` is `SysButtonMenu_Open(…, eMENU_DIRECTION)` with
  direction 2. It makes a task whose main is `dialogmain`
  (`__ZL10dialogmainv`). Example: "Start from last Auto save?" No / Yes.
- Work struct: `+0x0` state (takes touches above 2; 0x63 = a button was
  pressed, 0x65 = cancelled), `+0xc` message id, `+0x1c` button count,
  button i at `+0x28 + i·0x14`: `short` x, y, w, h (top-left rect), `int`
  enabled at `+8`. `+0xc8` the result, `+0xcc` the button the finger went
  down on, `+0xf0` "a tap off the buttons cancels" (`Enable_Cancel`).
- `SysDialog_Check`: a press needs Began and Ended on the same button.
  While the message is still printing (`SysMessage_Get_ComState != 5`)
  taps are ignored. With no buttons, a tap advances the message
  (`SysMessage_Next`). A tap off the buttons with `+0xf0 == 1` plays SE 3
  and cancels.

### Sprites, and screens with their own buttons (confirmed 2026-09-27)

- `_prim_work`: sprites ("prims"), 0x54 bytes each, indexed by prim id.
  `+0x0` in use, `+0x8` shown, float `+0xc` x, `+0x10` y, `+0x14` w,
  `+0x18` h; with `+0x38` set, x/y are the centre.
- `SysPrim_Touch_DrawRect(id, point)` returns **0** when the point is in
  that rectangle (edges included), −1 otherwise. Screens that draw their
  own buttons hit-test them with it, so a sprite's rectangle is exactly
  the tappable area.
- Hip-O-Drome (`Palace_Main`): five button sprites, ids at `work +
  0x454 + i·4`, placed at x = `+0x418` (slide offset) + 350, y stepping
  44. On finger-up it stores the hit index at `+0x414`. The same code runs
  the tutorial's first pull (other buttons greyed) and the full menu.
  `PalaceFlow_TutorialMenu` separately opens a `SysButtonMenu` for a
  tutorial sub-menu.
- The Hip-O-Drome's panel after picking a song (0x187e8): on finger-up
  it tests the sprites at `+0x47c` (Create Trooper → index 0), `+0x480`
  (No → 1) and `+0x48c` (the back icon → 2), stores the index at `+0x414`,
  then animates the pressed sprite for 5 frames. Closed sprite ids are set
  to −1.
- Towns (`Town_Main`; Soul Master's Place is one): `TownMenu_Ctrl` tests
  the icon sprites at `work + 0x60 … 0x78` (seven), skipping any whose
  `SysPrim_Get_Disp` isn't 1. `TownMenu_Disp` shows the subset the town's
  row of `_town_icontable` enables (7 ints per town, indexed by
  `_savedat + 0x8b78`). `TownMenu_Update_PosSize` puts them at y = 160.

### The card list (`_cardlist_work`) (confirmed in code 2026-09-27)

Used to pick a trooper (delete, and elsewhere). One global work struct:
`+0x0` state, `+0xc` mode, `+0x14` region the finger went down on,
`+0x18` the command region on a press, `+0x1c` card count, `+0x20` scroll
position (12 per card), `+0x34` x centre of the card strip, `+0x38`
selected card, `+0x40` target scroll, `+0x4c` key lock (1 = ignore
touch), `+0x58` status-panel mode, bottom icon sprite ids at `+0x3c8`,
`+0x3c4`, `+0x3cc`, `+0x3d0` (regions 5–8), scrubber knob sprite at
`+0x3b8`.

`CardList_Flow_Select` on finger-down picks a region: 4 = status panel
(x ≤ 160, only in status-panel mode), 3 = the scrubber (x within c ± 136,
y 216–272; jumps to the card under the finger), 1 = the card band
(40 < y < 224), 5–8 = the icons (`SysPrim_Touch_DrawRect`). On a non-flick
finger-up: 5/6 play SE 9 and set state 2 with `+0x18` = the region; 7/8
do the same only in mode 2 with their flags (`+0x53`/`+0x54`), else SE 1;
4 is `Status_Change`; 1 with x within c ± 55 returns 1 (the middle card
picked), else x < c − 55 turns the list back one card (two if x ≤ 60) and
x > c + 55 forward one (two if x ≥ 420). `CardList_AutoScroll(i)` and
`CardList_Set_Select(i)` exist too; touchHLE uses taps instead.

What the icon regions do (`CardList_Main`, after state 2): region 5
(`+0x3c8`, the status icon) toggles status mode `+0x58` (opening or
closing the stats panel); region 6 (`+0x3c4`) closes the panel and
returns 2, leaving the list. touchHLE's north button taps region 5 (or the
open panel, to flip it); back taps region 5 while the panel is open, else
region 6.

### `SysMenu`, scrolling lists (item list) (confirmed in code 2026-09-27)

- A task whose main is `Menumenumain` (`__ZL12Menumenumainv`), which runs
  `menumain(work)`. `SysMenu_Open(shown, z, …)`, `AddSelect` adds items
  (up to 64), `Set_Position` (`+0xd08/+0xd0a`), `Set_Size` (`+0xd0c`
  width, `+0xd0e`), `Get_Cursor` reads byte `+2`.
- Work struct: byte `+2` cursor, `+4` item count, `+5` rows shown. Item i
  is 0x34 bytes at `+0xc + i·0x34`: byte `+0x14` enabled, `short`
  `+0x30/+0x32` its row's current x, y (moved as it scrolls). `+0xd18`
  scroll position in rows (float), `+0xd1c` scroll speed.
- `SysMenu_Check2` (the item list's): finger down stops the scroll; a held
  finger on an enabled row makes it the cursor (highlight); a moving finger
  (flick) sets the speed to −Δy / 24 per frame. On a non-flick finger-up: a
  row (x ≤ px < x + width, y ≤ py < y + 40) that's enabled returns its
  index, a disabled one −3, no row −2 (cancel). It tests every row, **even
  ones scrolled out of the window**, so only tap rows on screen.
- `menumain`: position += speed, speed × 0.9 each frame, clamped to the
  list; below 0.08 the speed stops and the position rounds to a whole row.
  So one move of 4.8 points scrolls about one row.

### Edit Troopers (`Teammake`) sort panel and the status panel

- `Teammake_Open` makes a task (`Teammake_Main`). Its sort panel
  (`Teammake_Check`, 0x558ee) has six rows at x 80–400, y 37 + 46·i to
  83 + 46·i, row sprites at `work + 0x4c … 0x60`. A tap on a row changes
  that row's setting (state 12), the back icon sprite `+0x3c` closes it
  (state 13), and a tap well outside (x < 60, x > 420, y < 20, y > 310)
  cancels (SE 3). The panel shows over the card list.
- The status panel is part of the card list: in status mode (`+0x58` = 1)
  a tap at x ≤ 160 is region 4, `Status_Change`, which flips between the
  stats and skill pages.

### Options (pause menu > Options) (read from code 2026-09-27)

- `Option_Open` makes a task (`Option_Main`, 0xfba0; init `Option_Init`,
  exit `Option_Exit`); `Option_Check` (0x10198) does the touch handling.
  The pause menu's `SysButtonMenu` stays open under it.
- Work struct (0x70 bytes): `+0` state (2 = shown), `+4` touch sub-state
  (0/1 waiting for a touch, 2 a button's press animation, 3 a switch's),
  `+8` what the current touch grabbed, `+0x1c` shows an extra button
  (`+0x64`, `button_001.png` at (114, 292), 208×44 centred). Sprite ids:
  `+0x44` volume bar, `+0x48` volume knob (32×32 centred at
  (254 + 200·volume, 64)), `+0x4c/0x50/0x54` switch art, `+0x58/0x5c/0x60`
  the BGM/SE/lock switches' knobs (45×42, top-left at x = 368 + 44·on,
  y 88/168/220), `+0x68` back icon (`tc_icon.png`, 48×48 at (456, 296)).
  Settings are `_configdat`: float volume at `+0`, bytes BGM `+4`, SE `+5`,
  lock orientation `+6`.
- Finger-down on the volume knob grabs it; while held, the volume is
  clamp((x − 254) / 2, 0, 100) / 100 of the finger's x, every frame.
- Finger-down on a switch's knob grabs that switch; its finger-up sets it
  by the last move's direction (so a still tap on the knob turns it off).
- Otherwise a non-flick finger-up in (368, 84, 88, 40), (368, 164, 88, 40)
  or (368, 216, 88, 40) flips BGM, SE or lock orientation, and one on the
  back icon (or the extra button) plays SE 9 and closes.
- So the controller drags the knob for volume, and taps a switch's box on
  the half away from its knob.

### Help (pause menu > Help) (read from code 2026-09-27)

- A task, `Help2_Main` (0x60b2c); the pause menu stays open under it.
  Work struct (`stHELP2_WORK`): `+0` state (3 the list, 4 opening an item,
  5 an item's page, 6 closing it, 7/8 next/previous item, 10 a sideways
  swipe, 0x63 exit), `+0xc` page height, `+0x10` tab, `+0x14` item count,
  `+0x18` item touched/opened (-1 none), `+0x1c` list height, `+0x20/+0x24`
  list scroll position/speed, `+0x28/+0x2c` page scroll position/speed,
  `+0x54` row highlight sprite, row sprites at `+0x80 + 4i` (y = 32 + 56i
  − scroll).
- List (state 3): finger-down on a row sprite sets `+0x18`; a non-flick
  finger-up at y < 256 opens it (`SysHelp2_Open_MenuItem`, SE 9), at
  y ≥ 256 picks tab x / 80 (tab 5 is Exit: state 0x63). A move sets the
  speed to the move (previous y − y); each frame the position gains the
  speed and the speed drops 10%, so a drag glides 10× its last move,
  clamped to 0..`+0x1c` − 224. Rows show from y 32 to 256.
- Page (state 5): a non-flick finger-up at x > 400, y < 72 closes it;
  x < 48, 36 ≤ y ≤ 320 opens the previous item and x > 432, 76 ≤ y ≤ 320
  the next (if any). A vertical move scrolls the text the same way as the
  list (clamped to `+0xc` − 272); a sideways one over 24 points swipes.

### `SysDrum2`, the rotating drum (title menu) (confirmed 2026-09-27)

- `SysDrum2_Open` makes a task whose main is `drummenumain2`
  (`__ZL13drummenumain2v`). The title's drum: `AddSelect` × items,
  `Set_Position(110, 152)`, `Set_Size(260, 128)`, `Set_Cursor`,
  `Enable_OutrangeTouch`.
- Work struct: byte `+0` state (2/3 = handling touches), `+2/+3` cursor
  item (bytes), `+8` counter (`Get_CursorCounter`), `+0xc` outrange
  touch, `+0x290/+0x292` position, `+0x294/+0x296` size (`short`s),
  `+0x298` from Open's first argument, `+0x29c` angle (float), `+0x2a0`
  spin velocity, `+0x2a8` offset (settled when both are 0:
  `SysDrum2_CheckCursordisp`), `+0x2b4` "armed" (set while touching, by
  `drummenumain2`), item i's value at `+0x34 + i·40`.
- `SysDrum2_Check` on an armed, non-flick finger-up: with c = y − 8 +
  h/2, a tap with y < c sets the velocity to +0.5 (turn), y > c + h/3
  sets it to −0.5 (turn the other way), and in between returns the
  cursor item's value. A turn only starts if the velocity was 0.
- In testing, a tap above the band brings in the item below it, and one
  under the band the item above. So the controller's up taps under the
  band and down above it.
- So on the title drum the select band is y 208–250. The old centre tap at
  y = 160 was above it, which turned the drum: the "scrolling" seen in
  testing.

### Milestones

1. **Picker** (done 2026-09-27, tested on a real pad): D-pad focus,
   section jumps, tabs on the shoulders, confirm/back, button icons.
   `picker_view.rs`, `pad.rs`.
2. **Plumbing** (done 2026-09-27: confirm starts the game from the title
   screen): `game_input.rs` wraps `-[MainView mainLoop]`, logs scene
   changes, and injects taps through `SysTouch_Began_F1`/`Ended_F1`.
3. **Dialogs and generic menus** (in progress: `SysDrum2`, `SysDialog`
   and the title's `SysButtonMenu` confirmed 2026-09-27, with a
   touchHLE-drawn focus outline; the Hip-O-Drome menu, its song panel and
   town icons, the card list (delete screen), Edit Troopers' item list,
   sort panel and status panel (north button) and Options
   confirmed; Help written, not yet tested): A advances text (`SysMessage_Next`),
   the D-pad moves between `SysButtonMenu` rects, B backs out. This
   includes the game's Yes/No panel after picking a song.
4. **Battle**: unit/command menus, D-pad on the map cursor.
5. **Town, world map, formation, password keyboard**, one at a time.

## Tools (`inspect/`)

Extract `Payload/S.S.Encore.app/S.S.Encore` from your own IPA into the
folder you run these from. Never commit it.

- `mo.py`: Mach-O/ObjC2 parser (classes, ivars, methods, selrefs,
  classrefs, stubs, symbols).
- `ssdis.py <hexaddr | "-[Class sel]">`: annotated Thumb disassembly.
  Resolves literals, ivars, cfstrings, and `objc_msgSend`
  receiver/selector.
- `xref.py <hexaddr>...`: every BL/BLX caller, with the owning function
  and the `r0` immediate (handy for setter calls).
