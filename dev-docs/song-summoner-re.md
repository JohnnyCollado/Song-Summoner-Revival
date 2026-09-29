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

Layout (the getters at 0xd6bc…0xd7c4, checked 2026-09-27): `+0x08` frames
the finger has been down (`Get_Frame`), `+0x0c` finger count (≤2,
`Get_FingerData`), `+0x10` a finger is down (`Get_TouchState`), `+0x14`
flicking (`Get_FlickState`), `+0x18` tap count (`Get_CountState`), `+0x20`
double tap, `+0x24/+0x28` current x/y (float; with two fingers `+0x34 …
+0x40`), `+0x2c/+0x30` start x/y, `+0x54` began this frame
(`Get_BeganState`), `+0x58` ended (`Get_EndedState`), `+0x5c` cancelled
(`Get_CanceledState`; `SysTouch_Set_CanceledState` 0xd704 sets it, so the
host could cancel a held finger without it counting as a lift). The writers are
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
  Sub-state `+4` (1 takes touches). Like the world map's location menu,
  a finger-up on icon slot i selects it if `+0x14` (−1 at first) isn't
  i (SE 2, description, highlight anim `+0x7c`), and presses it if it is
  (SE 9, `+4 = 2`). Slots 4 and 6 also need `GetMasterEventFlag`, else
  SE 1 and the selection clears.

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
- The row highlight (read 2026-09-28): `SysMenu_Open` opens three
  untextured sprites, `+0xd24` (0x80202020), **`+0xd28` the highlight
  (ARGB 0x40f0f0f0, near-white at 25%)** and `+0xd2c` (0x80f0f0f0, a line
  as wide as the menu); each row also gets a 1-point 0x40ffffff line.
  `SysMenu_Disp_Cursor(id, shown)` puts the highlight at the menu's x
  (`+0xd08`), one point above the cursor row (`+2`), and shows or hides
  it; there's no `Set_Cursor`. `Check2` calls it for a still finger on an
  enabled row (the row becomes `+2`) and hides it on a flick.
  `SysTouch_Moved_F1` sets the flick flag on any move. The controller
  draws a copy of the highlight (`FocusShape::Highlight`) on its focused
  row, and hides it while the game's own shows.
- `menumain`: position += speed, speed × 0.9 each frame, clamped to the
  list. Below 0.08 (worked through against a log, 2026-09-28) the
  snap *adds* min(0.005 / d, d) toward the nearest row (d its distance)
  to the speed each frame, and only stops when the position crosses a
  whole row while under 0.08. A glide that ends far from a row speeds
  back up past 0.08 and overshoots: 4.8 points ends a row the **wrong**
  way after 47 frames. Drags of 2.7-3.8 points land one row on; the
  controller uses 3.5 (10 frames). Touch hits the same crawl when a flick
  stops near half a row.
- **The snap only runs with the cursor (`+2`) at -1.** Edit Troopers'
  item list has a cursor (0), and there `menumain` just glides at x 0.9
  forever (log 2026-09-29: a 3.5 drag from row 0 came to rest at
  1.4583, half between rows, its speed 1e-10 and never 0). A glide
  covers 10x its first frame's dy / 24 rows, so the controller drags
  2.4 points a row, aimed at a whole row from wherever the list is, and
  counts a list as still below 0.012 rows a frame (`list_still`; about
  5 points left to coast, ~20 frames a row). Snapping lists still wait
  for exactly 0.
- The controller only scrolls a list toward its focus after a command
  moved it (`list_keep_focus`). A list scrolled by touch takes the focus
  along onto its rows instead; dragging it back to the focus fought the
  finger (2026-09-29 log).
- `SysMenu_Check` (the shop's older list) computes the same speed,
  (prev.y - y) / 24, but a finger down on an enabled row makes it the
  cursor and calls `SysTouch_Clear`. Its caller runs `menumain` itself
  through `SysMenu_Update`; the `Menumenumain` task only closes the work.

### The shop's quantity dial (older `SysDrum`) (decoded 2026-09-28)

- `ShopFlow_BuyMenu` opens two `SysDrum`s (task main `drummenumain`),
  ids at shop work `+0x10c` (tens, at (306, 40)) and `+0x110` (ones, at
  (375, 40)), each 80x170, ten items each whose value (`+0x38`) is the
  digit. The amount is tens x 10 + ones (0x13ac4); a `SysDialog` below
  has Forget it (left) and Confirm (right), and Confirm is made
  unselectable past 99 held or what the Luna covers.
- Drum work: byte `+2` cursor, `+4` count, items 0x2c bytes (`+0x18`
  enabled, `+0x38` value), `+0x1610/+0x1612` position, `+0x1614/+0x1616`
  size, `+0x161c` position (float, in items, wrapping), `+0x1620` speed,
  `+0x1628` the frame's nudge. `SysDrum_CheckCursordisp`: speed 0 and
  nudge under 1e-5.
- `SysDrum_Check`: a finger down in the rect zeroes the speed; a move in
  it sets the speed to (prev.y - y) / 24. A finger-up in the rect with no
  move clears the touch and returns the digit, which the Buy flow takes
  as "pick": it clamps the amount and `SysDrum_Set_Cursor`s both drums
  (which zeroes their speed). After a move the flick branch runs
  instead, so a flick never picks.
- `drummenumain`: position += speed, speed x 0.8. Below 0.1 a nudge
  toward the nearest digit (2a(a + 0.5) / 3, a the signed distance past
  the half) joins the speed; when they pull opposite ways the speed is
  quartered, and under 0.02 within 0.05 of a digit it stops there.
  The controller flicks 6 points per digit (8 frames).

### The shop's password spot and keyboard (decoded 2026-09-28)

- `Shop_Main`'s flow (`+0`, `__switchu8` at 0x14274): 3 the top menu
  (`ShopFlow_MenuSelect`), 4 password input, 5 its tutorial (the first
  time), 6 buy, 7 sell, 8/9 leaving.
- The top menu (flow 3, sub-state `+4` = 1) is a `SysButtonMenu` at
  (320, 200), three buttons. Before `SysButtonMenu_Check`, a non-flick
  finger-up at x 40-192, y ≥ 108 (over the shopkeeper,
  `ui_bust_shop.png` at (-48, 64), 256×256) closes the menu and opens
  the password entry. No button marks it. The controller's Info button
  (north) taps it, and while the pad or keyboard is in use touchHLE draws
  a "Password" pill with that button's icon in the bottom-left corner
  (`setup_view::render_hud`).
- Password input (flow 4) shows messages (tap anywhere), then opens the
  `Keyboard_Main` task and waits for it to end.
- `Keyboard_Main` (0x4d1d0): state `+0` (switch16 at 0x4d1e8), 1 takes
  touches. `getkeybord(point)` (0x4d0e8) finds the key in `keyrect`
  (0x6ea48): 10-byte entries, `short` x, y, w, h and the key byte,
  ending at key 0, hit where x ≤ px < x + w and y ≤ py − 160 < y + h. Rows
  at screen y 164, 204, 244 (39×32 keys, 47 apart from x 9) and 284 (Z to
  M from x 80), plus `r` (9, 285, 51×30), `b` (432, 244) and `e`
  (419, 285, 51×30). I and O are stored as `1` and `0`.
- While a finger is down the key under it is kept at `+0x14`; on the
  finger-up that key acts: `e` checks the password (state 2), `b`
  deletes the last character (count byte `+0x30c`, text from `+0x18`),
  `r` opens a `SysDialog` (state 9, to stop), anything else is typed
  (at most 35). Results and errors are `SysDialog` messages.
- Controller (`keyboard_command`): the D-pad moves over the keys by
  position, confirm types the focused key, back is Backspace (or, with
  nothing typed, the `r` key). With Setup > Game > Shop password set to
  the device keyboard (the default on desktop), a real keyboard types too
  (`typed_key`, before the key mapping): letters, digits, Backspace, and
  Return for Enter; I and O type 1 and 0 as the game's keys do. On
  Android that setting shows the on-screen keyboard (SDL text input)
  while the game's keyboard is up, and its text is typed the same way.

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

### Cutscene SKIP (`Script_Main`) (read from code 2026-09-27)

- `ScriptSkip_Init` loads `ui_skip.yas` at (440, 12). `ScriptSkip_Enable`
  / `_Disable` show or hide it and set the byte `_skip_able` (0xa718d,
  an odd address; `Script_SkipMode` writes the unnamed byte before it).
- `Script_Main` (run by `Town_Main` and by Tactics' cutscenes): a
  finger-up without a flick at x >= 440, y <= 40, with `_skip_able` set,
  opens a `SysDialog` "Skip?" (`_dino_skipselect`), which it then polls:
  Yes (button 0) fades out and skips (`_skip_fade`), No closes it.
- Controller: Start taps (460, 20) while `_skip_able` is set; the dialog
  is then driven like any other `SysDialog`.

### World map (`Worldmap_Main`) (read from code 2026-09-27)

- Task main `Worldmap_Main` (0x7cf8). Work (`SysTask_Get_WorkAdr`):
  `+0x0` phase (`Worldmap_Set_Phase`, which also zeroes `+4`, `+0xc`):
  0 load, 1 fade in, 2 open symbols, **3 map takes touches**, 4 player
  walking (`WorldmapPlayer_Move`), 5 location menu (`WorldmapMenu_Ctrl`).
  `short +0x28/+0x2a` camera centre (map coords), `+0x2c` camera
  animating (while set, `WorldmapCamera_Moving` returns −1 and the phase
  is skipped), `+0x38` the highlighted symbol, `+0x3c` the symbol the
  finger went down on (−1 = none).
- `_worldmap_symbol`: 32 entries of 0x1c: `int` id, `int` type (0 =
  unused, 1 and 2 drawn; 1 looks like towns), `short` x, y (map coords)
  at `+8`/`+0xa`, then up to four (`short` neighbour, `short` 0) pairs,
  −1 = none: the road graph. 25 real entries.
- `_savedat + 0x8a78 + i·4`: symbol i's state; **2 = open (selectable)**.
  `_savedat + 0x87f0`: the current location. `_savedat + 0x87e8`: zoom
  (float, 0.5–1; pinch changes it).
- `Worldmap_Check_SymbolTouch(CGPoint)` (0x5e80), read-only: with
  `s = 3 − 2·zoom`, map = camera + s·(point − (240, 160)); returns the
  first open symbol whose (x − 36, y − 24, 72, 48) holds it, else −1.
- Phase 3 input: a one-finger touch without a flick sets `+0x3c` from
  `Check_SymbolTouch`; a flick pans the camera and resets `+0x38` to the
  current location; two fingers pinch-zoom. On finger-up with `+0x3c`
  set: a different symbol than `+0x38` → SE 2, `+0x38 = +0x3c`, name
  banner (`WorldmapName_Set`), highlight anims moved to it. The same
  symbol → SE 9, then if it's the current location
  `WorldmapMenu_Disp(1, 1)` and phase 5 (the menu), else phase 4 (walk
  there, camera following; the menu opens on arrival).
- Selecting is inline in `Worldmap_Main`: there's no function that
  highlights a symbol, so without writing `+0x38` it takes a tap.
- `WorldmapCamera_Set(x, y, frames)` (0x6ad8): frames ≤ 1 moves the
  camera at once, clamped to the map's edges for the zoom; more animates
  it (`+0x2c`). Only touches the camera.
- **Neither it nor `Check_SymbolTouch` can be called from the host.**
  They find the work through `SysTask_Get_WorkAdr()`, which reads slot
  `_task_play_num`; `SysTask_Main` runs every slot and leaves that at 16,
  one past the table, so between frames it returns garbage. So the host
  reads the table, save data and camera itself, and pans with a drag: a
  one-finger flick moves the camera by twice the finger's movement per
  frame (map units, clamped), then resets `+0x38` to the current location.
- Controller (`game_input.rs`, `WorldMap`): D-pad goes to the next open
  location along the roads in that direction (past closed ones), else the
  nearest open one within 60°; shoulders cycle open locations; confirm
  taps the highlighted one again. An off-screen target is panned to (a
  one-move flick from the centre, repeated until it's on screen or the
  camera stops), then tapped at the point `Check_SymbolTouch` maps onto
  it.
- The location menu (phase 5, `WorldmapMenu_Ctrl` 0x6da8, same work
  struct): sub-state `+4` (0 sets up, **1 takes touches**, 2+ act on the
  choice). Sprites at `+0x2f0 … +0x2fc` (`SysPrim_Touch_DrawRect`): three
  icons, then the back icon; `WorldmapMenu_Disp` shows or hides them all.
  `+0x1c`: a press began this touch. On finger-up over icon i: if `+0x14`
  (the selection, −1 at first) isn't i, SE 2, `+0x14 = i`, its
  description and highlight (anim at x = 160 + 80·i, y = 160) show;
  if it is, SE 9 and `+4 = 2` (pressed). The back icon presses on one tap.
  A tap outside (x < 80 or > 400, y < 120 or > 240) cancels: SE 3, menu
  hidden, phase 3. Driven as a `SCENE_BUTTONS` group with `selected`.

### Battle (`Tactics_Main`) (read from code 2026-09-27)

Not tested yet: everything here is from the disassembly.

**Phases.** `Tactics_Main` (0x3f54) is the scene task's main. Its work
struct (`stTACTICS_WORK`, found through the task table like the world map's)
has `+0x0` phase, `+0x4` sub-phase (`Tactics_Set_Phase` /
`Tactics_Set_PhaseSub`), `short +0x14/+0x16` the camera origin, `+0x20..0x28`
`Tactics_CtrlTest`'s gesture state. The phase switch (`___switch16`) runs one
module's Start/Main/End per phase. The ones the player drives:

| Phase | Module | What it is |
|---|---|---|
| 8 | `TacticsSortieSelect` | deploy units before the fight |
| 19 | `TacticsUnitSelect` | the map with the status panel (the hub) |
| 20 | `TacticsUnitMenu` | the command ring around a unit |
| 21 | `TacticsMapMenu` | MENU (retreat, terms, end phase, options) |
| 22 | `TacticsMoveSelect` | pick a tile to move to |
| 23 | `TacticsMoveWalking` | the unit walks; no input (see below) |
| 24 | `TacticsAttackSelect` | pick a target |
| 25 | `TacticsAttackInfo` | the attack preview / confirm |

Others: 12 battle start, 13 Listening Point, 14 Colosseum target, 16 phase
start, 17 unit check, 18 turn script, 26 attack animation, 27 silent box, 28
unit end, 29–35 contest win/lose, 36–38 result / game over. Before the
switch, `Tactics_Main` returns early while `_tactics_script_flag` is set
(script loop), while `TacticsTutorial_Main` returns non-zero, and on the
frames `TacticsMapCursor_Moving` or `_Zooming` animate.

**`Tactics_CtrlTest(began, hold, flick, release, cancel, CGRect)`**
(0x3a28; the 5-argument overload at 0x3b88 passes (0, 0, 480, 320)). The
shared gesture classifier for the map phases:
- one finger down: `began` on the Began frame. Not flicking, inside the rect
  (`SysTouch_Check_InRect` returns **0 on a hit**; rects are x, y, w, h) and
  held more than 6 frames: "hold", which puts the game's map cursor on the
  tile under the finger (`TacticsMap_Touch_Screen2Map`) and calls `hold`
  every frame. Flicking: calls `flick` (map scroll).
- two fingers: pinch (`TacticsMap_Pinch`).
- Confirmed in play 2026-09-27: holding then dragging moves the game's
  cursor with the finger, and letting go acts on that tile, like a tap there
  (select a unit, walk to a reachable tile, pick a target). Letting go
  anywhere else does nothing; the cursor goes back.
- So the controller could drive the game's own cursor with a held virtual
  finger (its previews show: the status panel follows the unit under it, and
  the attacker turns to face the tile) instead of drawing its own. Confirm =
  lift; back = slide off to a tile with no effect, then lift (or a touch
  cancel, `SysTouch_Get_CanceledState` → the phase's cancel handler, if
  the host can raise it). To try in step 2 against the drawn cursor.
- Confirmed in play 2026-09-27: the camera does **not** follow the held
  cursor to the screen edge (no auto-scroll), so a held finger can't reach
  off-screen tiles without lifting, flicking and pressing again. In move
  select, letting go on the unit's own tile **cancels the move and returns
  to the command ring**; on an unreachable tile it does nothing.
- finger up: `release`, then `SysTouch_Clear`; returns 1 after a hold, 2
  after a flick, 3 for a plain tap, 0 after a pinch.

**The grid map: `_tacticsmap_work` (0x149504), a global.** The host reads
it directly (no task lookup):

| Offset | What |
|---|---|
| `short +0x8` / `+0xa` | map width / height in tiles |
| `+0x28`, `+0x2c` | background sprite ids (the pre-rendered map art) |
| `+0x30` → `int[w·h]` | unit number on each tile, −1 = none (`TacticsMap_Get_UnitMapping`) |
| `+0x34` → `int[w·h]` | highlight layer: move / attack ranges (`Set_WorkData`) |
| `+0x38` → `int[w·h]` | its temp copy (`Set_WorkDataTemp` keeps the max) |
| `+0x3c` → `int[w·h]` | terrain chip index per tile (`Get_ChipIndex`) |
| `+0x40` → `int[w·h]` | each tile's highlight sprite id |
| `+0x48 + chip·0x16` | terrain records: defence rates at `+0` (`[2][…]` bytes), move costs at `+8`, cover line at `+0x11` |
| `+0x5c4` → `0x1c`-byte records per tile | map objects |
| `+0x5c8` / `+0x5d0` | map cursor state / moving (`TacticsMapCursor_Get`, `_Get_Move`) |
| `short +0x5d8/+0x5da` | the game's map cursor tile (`TacticsMapCursor_Get_Position`) |
| `+0x5e8 … +0x5fe` | zoom: animating flag, mode, current %, start %, target %, frame / frames |

Tile (x, y) is at index `y·w + x`. Units: `_savedat + 0x570 + n·0x190` (24
slots): byte `+0x0` in play (2 = on the map), byte `+0x2` team (the team
whose turn it is: `_savedat + 0x7f60`), `short +0x4` state (3 = out),
position `xy` at `+0x100` of the unit data. `_g_select` points at the
selected unit (`+0` number, `+4` → its data).

**The grid is flat.** Tiles are 48×24 diamonds; cliffs and height are only
in the background art. `TacticsMap_Get_Map2DrawPosition(x, y)` (0x1ae7c):
map point = origin + (24·(x − y), 12·(x + y)), origin = the Tactics work's
`+0x14/+0x16`; the tile's sprite is placed 24 left and 12 up of that
(`TacticsMap_Update_ChipPosition`), so that point is the diamond's centre.
The whole map is drawn scaled by `_prim_scale` (`SysRender_Get_Scale`,
zoom % / 100) about (240, 160), so screen = (240, 160) + scale · (map point
− (240, 160)). `TacticsMap_Touch_Screen2Map` (0x1b0a8), used by the held-finger
cursor, is the inverse but also adds −36 / scale to y in map points: **36
screen points** at any zoom, so the hold cursor sits 36 points above the
finger. `TacticsMap_TouchFast_Screen2Map` (0x1aed0), used for taps, has no
offset. So a tap goes on the tile's centre, and a held finger 36 below it
(`battle.rs`, `Camera::hold_point`). Map x runs down-right on screen, map y
down-left.

**The host can't call the map API.** Like the world map, nearly all of it
finds the work through `SysTask_Get_WorkAdr()`: `TacticsMapCursor_Set_Position`,
`_Set_Map2Focus`, `_Get_Map2Focus`, `_Add/_Offset_Focus`, `_Moving`, `_Zooming`,
`TacticsMap_Get_Map2DrawPosition`, `_Update_ChipPosition`, `_Scroll`,
`_Pinch`, `TacticsUnitAnim_Get_ScreenPosition`, and every phase's
Start/Main/Ctrl. So the host projects tiles itself and acts with taps and
drags. Readable without it: the getters on `_tacticsmap_work`
(`TacticsMap_Get_Size`, `_Get_UnitMapping`, `_Get_ChipIndex`,
`TacticsMapCursor_Get`, `_Get_Position`, `_Get_Zoom`).

**Zoom: `TacticsMapCursor_Set_Zoom(percent, frames, mode)`** (0x1b294)
doesn't use the task work: it writes `_tacticsmap_work` +0x5e8…,
`SysRender_Set_Scale` and the zoom number (`TacticsTurnInfo_Change_ZoomInfo`,
global `_tacticsturninfo_work`). With frames ≤ 1 it applies at once;
otherwise `Tactics_Main` animates it (`TacticsMapCursor_Zooming`, which also
moves tiles, units and cursor, and skips the phase logic that frame). The
scale is about the screen centre, so the camera doesn't jump. Pinch keeps
the scale in 1.0–2.0 (100–200%) and also saves it to `_savedat + 0x7f58`
(restored on resume); `Set_Zoom` doesn't.

**Unit select (phase 19).** Global `_tac_unit_select` (0x14d860): `+0x0`
state (`TacticsUnitSelect_MainCtrl`'s switch, 0x3958e: 0 one-off setup, **1
taking touches** (`Tactics_CtrlTest`), 2 MENU's press animation, 3 another
team's unit's status, 4–6 status panel rolls, 7 a unit tapped, 8 the panel
tapped), `+0x4` frame counter, `+0xc` the tapped unit, `+0x10`, `+0x18` "the
finger went down on the status panel", `+0x1c/+0x20` where, `+0x24` the MENU
button sprite. `_tac_unit_list` (0x14d888): the player's units, `+0x60`
the current index. `TacticsUnitSelect_MainCtrl` passes the rect (0, 0,
480, 256) (the map above the panel) to `Tactics_CtrlTest`.
- Began inside (104, 256, 272, 64), the status panel, sets `+0x18`; a flick
  from there rolls the panel (`TacticsStatusPanel_Pos2Angle`) and, on
  release, angle < −2 / > 2 goes to the previous / next unit (`_ListNum2`).
- Release, not a flick: on the MENU sprite (`SysPrim_Touch_DrawRect`) SE 9,
  state 2, which animates it 5 frames and opens phase 21. On the panel (not
  flicked): SE 9, the cursor goes to the selected unit, state 8, then after
  5 frames: an own unit whose team has the turn opens the command ring
  (phase 20), otherwise its status shows (state 3). In (56, 272, 44, 48) (the
  left curved arrow) `TacticsUnitSelect_Prev_ListNum`; in (379, 256, 44, 64)
  (the right one) `_Next_ListNum`.
- A tap on the map (`TacticsMap_TouchFast_Screen2Unit`), or a release after a
  hold (the unit on the cursor tile): a unit that's in play is selected
  (state 7; the cursor and camera go to it). Tapping the selected unit again
  does what a panel tap does. No unit: the cursor goes back to the selected
  unit.
- **Tap vs hold pick differently** (`CtrlRelease` 0x399c4, decoded
  2026-09-27). After a hold (Tactics work `+0x20` == 2, at 0x39c8a) it
  takes `TacticsMapCursor_Get_Position` → `TacticsUnit_Get_Pos2Number`:
  **by tile**. Out (state 3) or none: the cursor goes back to `_g_select`.
  Otherwise `Set_GlobalSelect` at once and `Set_Map2Focus` (camera, 5
  frames); if it's `_tac_unit_list[+0x60]` (the current one): its team has
  the turn → phase 20 sub 0x66 (the ring), else status (state 3);
  if not, the cursor goes to it and `Update_ListNum` makes it current (a
  second release acts). But a held finger never gets there: every frame of
  the hold, `CtrlTouch` (0x39510) calls `Update_StatusPanelList` for the
  unit under the cursor, making it current, so **one hold acts** (seen in
  play: one hold on an unselected own unit opened the ring). A plain tap (0x39bd8) uses
  `TacticsMap_TouchFast_Screen2Unit`, which goes by the units' sprites:
  seen in play, a unit drawn over the tile behind it takes a tap meant for
  that tile. So the controller's confirm is a still held finger (10
  frames, 36 points below the tile), which needs the finger inside
  `CtrlTest`'s rect (0, 0, 480, 256): the tile's centre at y ≤ 212
  (`battle::holdable`).

**Command ring (phase 20).** `_tacticsunitmenu_work` (0x149b80): 7 command
sprites at `+0x54 + 4i`, placed on a ring by
`TacticsUnitMenu_Update_PosSize` (item i at angle `+0x14` + i·360/7). In
`TacticsUnitMenu_CtrlTop` a drag turns the ring (atan2 of the finger around
the centre); a finger-up outside (100–380, 40–280) cancels back to unit
select (SE 3); otherwise it hit-tests the item sprites. Sub-controllers
`_CtrlSkill` (the skill panel, below), `_CtrlItem`
(`TacticsItemWindow`), `_CtrlStatus` (tap flips the status page).
Confirmed in play 2026-09-27: tapping an item that isn't at the front turns
the ring to bring it to the front; tapping the front item picks it. The
front item is the one whose angle is nearest 270°, which `Update_PosSize`
stores in `_tacticsunitmenu_work + 0x18`. Controller: D-pad left / right
tap the neighbouring item (the ring turns one step, with the game's own
animation and sound), confirm taps the front item, back taps outside the
ring (x < 100 or > 380, y < 40 or > 280). The item sprites' rects come
from `+0x54 + 4i` (centred prims); wait for the turn to finish (`+0x14`
stops changing) before the next tap.

**Item window** (the ring's Item; `TacticsUnitMenu_CtrlItem` 0x1d674 →
`TacticsItemWindow_Main` 0x44a44; read from code 2026-09-27). A **`SysMenu`**
(the same scrolling list as Edit Troopers' item list, `SysMenu_Check2`), so
the existing `List` widget drives it. Global `_itemwindow_work` (0x14e198):
`+0x4 + 4i` the items, `+0x34` the picked item, `+0x38` the `SysMenu`
(opened at (120, 60), 240×200), `+0x3c` the description window, `+0x40 …
+0x48` its messages. `Check2` ≥ 0: SE 9, the item is picked,
`_g_select + 0x198` = 2 (item) and `+0x19c` = the item, then attack select
(phase 24) for its target. −2 (a tap off the rows): SE 3, back to the ring.
−3 (a disabled row): SE 1. Controller: the list as elsewhere; back taps off
the rows.

**Status** (the ring's Status; `TacticsUnitMenu_CtrlStatus` 0x1d5c8). A
finger-down records its side in `_tacticsunitmenu_work + 0x8` (0: x < 160,
1: x ≥ 160); a finger-up counts only on the side it started: x < 160 (the
stats panel) SE 9 and `Status_Change` (flip the page), x ≥ 160 SE 3 and back
to the ring. The same pattern as the deploy map's status and the card list's
status panel. Controller: north (or confirm) taps (80, 160) to flip, back
taps (320, 160).

**Skill panel** (the ring's Skill; `TacticsUnitMenu_CtrlSkill` 0x1d6ec,
sub-state `_tacticsunitmenu_work+0x4`; read from code, the flow confirmed in
play 2026-09-27). The skills list on the right (x ≥ 320), with the map still
on the left. Rows are 60 high from y 8: row i = (y − 8) / 60. Data, all in
the global `_status_work` (0x14d4ec): `+0x24` skill count, `+0xc + 4i` the
skill in row i (`Status_Get_SkillNumber`, > 0 = a skill), `+0x70` the
game's skill cursor sprite (at (304, 20 + 60i)). Input:
- a finger held on a row: the cursor goes to it and, if it's a skill, its
  area flashes on the map (`TacticsAttackSelect_Get_AbleTargetNum_Skill`,
  `_Flash_AreaWork`), a preview (confirmed in play 2026-09-27: holding a
  skill shows its valid targets without selecting it; the user wants this
  kept, it's the main reason for the held finger here). Held on the map
  side: the cursor hides; if
  the touch began there the map scrolls.
- finger-up on a row: a skill with targets and enough SP (unit data `+0x122`
  ≥ the skill's cost, skill table entry 0x48 bytes, cost at `+0x18`) plays SE
  9, sets `_g_select + 0x198` = 1 and `+0x19c` = the skill, and goes straight
  to attack select (phase 24), which shows the skill's valid tiles. Otherwise
  SE 1 and nothing.
- a non-flick finger-up at x < 320 closes the panel, back to the ring.
  A finger that moved there (a drag off the list) is a flick: the cursor
  and preview just clear. So a still tap on the map side closes it, and
  letting go of a held finger anywhere but a usable skill does nothing
  (confirmed in play 2026-09-27).
Controller: the finger-follow pattern: a held finger on the focused row
(the game's cursor and the area preview show), the D-pad moves it between
rows 0 … count − 1, confirm = lift, back = slide off the list and lift
(harmless), then a still tap on the map side, e.g. (160, 160), which closes
the panel. `StatusSkillCursor_Set` and
the area functions don't use the task work, so calling them for the
preview instead is possible, but taps keep the game's own flow.

**MENU (phase 21).** `TacticsMapMenu_Start` opens a `SysButtonMenu`
(retreat disabled unless `TacticsMap_Get_RetreatOK`) with outside-cancel and
the cancel icon; its items open `SysDialog`s, `TacticsMapTerms`, or
`Option_Open`. All of these already work with the controller.

**Move select (phase 22).** Uses the whole screen (the 5-argument
`Tactics_CtrlTest`). Reachable tiles: `*_tactics_move_select_target_list`
(0xa59ec, a pointer), `(short x, short y)` × `_tactics_move_select_target_list_cnt`
(0x14a930), and a per-tile byte map `*_tactics_move_select_check_map`
(0xa59e8), all built by `TacticsMoveSelect_Create_CheckMap` and freed on exit.
`TacticsMoveSelect_Search_CheckMap(xy)` returns a tile's index in the list
(−1 = not reachable). Confirmed in play 2026-09-27 (step 1's log): the list
is exactly the lit tiles (highlight layer value **4**), ready one frame
before the tiles light; it **includes the unit's own tile** (letting go
there cancels the move), leaves out tiles other units stand on, and can
reach tiles behind allies. While a finger is held the game's cursor goes
anywhere, occupied and unreachable tiles too, so the controller's lock must
come from this list. A tap (`TacticsMap_TouchFast_Screen2Map`), or a
release after a hold (the cursor's tile), on a reachable tile **moves there
at once**: no confirmation (route `TacticsMoveWalking_Create_RouteData`,
phase 23; confirmed in play 2026-09-27).
Work: `_tacticsmoveselect_work` (0x14a924).

**Attack select (phase 24).** `_tac_attack_select` (0x14b70c); targets in
`_tactics_attack_info_target_list` (0x14b604) × `_cnt` (0x14b700). A tap on
a target (`TacticsAttackSelect_TouchFast_Screen2Unit`, size-aware for big
units) picks it at once: SE 9, `_tac_attack_select` state 1, and 5 frames
later `TacticsAttackSelect_Main` faces the attacker to it and opens attack
info (phase 25; 27 in one case). `CtrlTouch` (hold) turns the attacker to
face the tile (`Check_PosAngle`) and shows the spread area.
`TacticsAttackSelect_CtrlRelease` (0x2f7b4) treats the two finger-ups
differently. `+0x72c` picks the mode: a unit is looked up in the target list
(`Search_TargetList`), or, for area skills, the tile in a position list
(`Search_PosList`), so the controller's lock must follow whichever list is
active. Both are in `_tac_attack_select`: target tiles `(short x, short
y)` at `+0x8`, count `+0x38c` (`Search_TargetList` 0x2e5d4); area tiles at
`+0x394`, count `+0x718` (`Search_PosList` 0x2e614); `+0x390` the picked
entry.
- after a hold (gesture 2): the cursor's tile, if it's a target, picks it;
  anything else does nothing, the cursor just hides.
- a quick tap: a target picks it; **anything else (outside the available
  tiles) plays SE 3 and goes back to the command ring**
  (`Tactics_Set_Phase(20)`). This is the touch "back". Confirmed in play
  for skills too (2026-09-27): a tap off the skill's valid tiles returns to
  the command ring.
Both confirmed in play 2026-09-27.

**Attack info (phase 25)** (read from code, behaviour confirmed in play
2026-09-27). After a target is picked the camera zooms onto it (a spotlight
circle) and the attacker / target panels and the damage and hit % show.
`TacticsAttackInfo_Main` (0x2e248) acts on any non-flick finger-up (no
hold) by fixed rectangles, not the circle:
- more than one target (`_tactics_attack_info_target_list_cnt` > 1) and x
  ≥ 360, 180 ≤ y ≤ 228: the next target (SE 2; `_…_target_list_pos` + 1,
  wrapping). The list is 8-byte entries, the unit number at `+4`. No
  "previous".
- y < 40, or y < 216 with x < 140 or x > 340: **cancel**, SE 3, back to
  attack select (phase 24).
- anything else: **confirm**, SE 9, the attack (phase 26). That's the band x
  140–340 above the panels (wider than the circle), and the whole panel strip
  y ≥ 216.
Controller: confirm taps (240, 160), back taps (240, 20), and with several
targets L/R tap (420, 204) (previous = count − 1 taps).

**Walking (phase 23)** takes no input. `TacticsMoveWalking_Main` steps the
route (`_tactics_move_walking_flag` 0 start, 1 walking, 2 arrived); on
arrival it updates the unit map, runs any map script on the tile
(`TacticsMapScript_Check_PosScript`), and sends a player unit back to the
command ring (phase 20); an AI unit ends its turn or goes on to attack.
`_tactics_move_walking_yesno` is **dead**: `TacticsMoveWalking_Init` zeroes
it and nothing reads it (its only literal-pool reference; `Main`'s `[r4,
#4]` loads are off `_g_select`, not the flag). Probably left over from a
removed "move here?" prompt; the game moves at once.

**Sortie / deploy (phase 8)** (read from code 2026-09-27, not tested).
`TacticsSortieSelect_Main` (0x37120), global `_sortieselect_work`
(0x14d6e4): `+0x0` state, `+0x4` sub-state, `+0xc` frame counter, `+0x10`
panel row, `+0x14/+0x18` the picked card (`CardList_Get_LastSelectBench`),
`+0x20` units deployed, `+0x24` the most allowed, `+0x28` deploy tile count,
`+0x2c` → deploy tiles (0x14 bytes each: `short` x, y; `+0x8`/`+0xc` the
card on it, `+0xc` = −1 when free), `+0x30` → a unit sprite per tile,
`+0x34` the picked unit's sprite, `+0x38` the common menu, `+0x40` the open
dialog, `+0x68` the sort panel's back icon, `+0x90 + 4i` its row sprites,
`+0xd8` the map view's back icon. The tiles come from the per-map tables
`_sortie_pos_MAP_00…49` (`_sortie_pos_list`). The flow:
- **Card list (state 3):** `CardList_Main`. Picking a card that isn't
  deployed: SE 9, the list hides, the deploy tiles light up
  (`TacticsMap_Disp_WorkData`), the camera goes to the first tile and the
  unit's sprite appears, then state 4. With `+0x20 ≥ +0x24` (full), or a
  unit that can't go, a `SysDialogMessage` instead (SE 1). Picking a deployed
  card takes it back off its tile. The list's exit (region 6) opens the
  common menu (state 6).
- **Placing (state 4):** `Tactics_CtrlTest` (whole screen) with
  `TacticsSortieSelect_Ctrl_*`. While held, the unit's sprite stands on the
  tile under the finger, tinted when that isn't a free deploy tile
  (`Ctrl_Touch`); a flick scrolls. `Ctrl_Release` (0x36eb4): after a hold
  or a tap, on a free deploy tile it places the unit (SE 0x3e, `+0x20` + 1);
  on an occupied one it **replaces** that unit (whose card is unmarked). A
  lift after a hold anywhere else does nothing (still placing); **a tap
  anywhere else cancels** (SE 1, back to the card list). After placing, state
  5 waits 5 frames and marks the card; if that filled the party, a
  `SysDialog` (with cancel) asks, else back to the card list.
- **Common menu (state 6):** `TacticsSortieSelect_Open_CommonMenu`: a
  `SysButtonMenu`, 5 buttons of 320×54, outside-cancel and the cancel icon
  (both go back to the card list). Button 0 (greyed out in one mode until a
  unit is deployed): a `SysDialog` → start the battle (fade, state 0x13).
  Button 1: the **sort panel** (state 7). Button 2: the **map view** (state
  8). Button 3: the victory terms (`TacticsMapTerms`, state 10; see below). Button 4: a
  2-button `SysButtonMenu`: Options (`Option_Open`), and a `SysDialog` that
  fades out and leaves (retreat / quit).
- **Sort panel (state 7):** the same layout as Edit Troopers' (six rows, x
  80–400, y 37 + 46i to 83 + 46i; a tap on row i: SE 9, then
  `CardList_Sort`), back icon `+0x68` closes it, a tap well outside (x < 60,
  > 420, y < 20, > 310) cancels (SE 3); both return to the card list.
- **Map view (state 8):** `Tactics_CtrlTest` with `TacticsSortieMap_Ctrl_*`:
  flick scrolls, a hold shows the status panel for the unit under the finger.
  On release: the back icon (x ≥ 432, y ≥ 272; `tc_icon` at (456, 296))
  animates `+0xd8` and returns to the common menu; a unit opens its full
  status (sub-state 3: a tap at x < 160 flips the page, `Status_Change`,
  x ≥ 160 closes it, SE 3).

Confirmed in play 2026-09-27 (controller build as of then): the card list
(trooper screen) is already fully navigable here. Confirm on a unit
highlights its deploy tiles; a tap on a valid tile places it and returns to
the card list; a tap anywhere else returns to the card list. Placing the
last unit the party allows brings up the confirmation `SysDialog` (state 5
→ `+0x40`, cancel enabled), which the controller already drives.

Controller plan for the sortie:
- **Set the focus when the party-full dialog opens.** Today a dialog gets
  button 0 only when its work address differs from the last focused widget's;
  a new dialog allocated at the same address would keep a stale index. So
  when `_sortieselect_work` state becomes the party-full dialog's (the
  sortie's own dialog at `+0x40` appears), reset the focus to **button 1,
  "No"** (decided 2026-09-27: someone pressing fast who wants a change
  mustn't be locked into the battle). The dialog is `{0x17, 4, 5}` (message
  0x17, labels 4 and 5 from the IPA's message file); its handler (state 13,
  0x38234): result 0 starts the battle (state 11), result 1 **or cancel**
  (−2, a tap off the buttons) closes it and returns to the card list. So
  back is No as well. More generally, reset the focus whenever a dialog
  appears where none was open, not only when the address changes.
- Card list, the common menu and its sub-menu, dialogs and Options: the
  existing widgets (check that `Cards` is found here, over the map).
- Placing: the cursor is **locked to the deploy tiles**, with a held
  virtual finger so the unit's sprite shows where it will stand (and the
  tint shows an occupied tile). Confirm = lift (place or replace); back =
  lift off the tiles (harmless), then tap off them (cancel to the card list).
- Sort panel: a `SCENE_BUTTONS`-style group with Edit Troopers' geometry.
- Map view: the drawn free cursor from unit select; confirm taps a unit
  (status), north flips the status page (x < 160), back closes it or taps
  the back icon.

**Victory terms (`TacticsMapTerms`)** (read from code 2026-09-27). Shown
at a battle's start (phases 5–6, with a fade), from the battle MENU and from
the deploy menu (button 3). `TacticsMapTerms_Main` (0x1a194), global
`_tacticsmapterms_work` (0x1494d8): `+0x0` state, `+0x4` frame counter,
`+0x8` "fade in / out" (the battle-start version), `+0x10` the title
animation, `+0x14/+0x20` windows, `+0x18/+0x1c/+0x24/+0x28` its messages.
States: 0 wait for the art and messages to load, then show them (1: fade in
when `+0x8`); 2 the title animation; 3 slide in for 8 frames, then
`SysTouch_Clear` (taps before now are dropped); **4 any finger-up anywhere
closes it** (no area, no flick check); 5 slide out for 8 frames, then end
(fading when `+0x8`). Controller: confirm or back taps the centre while `+0x0
== 4`.

**Listening Point** (read from code 2026-09-27, not tested). Two parts:
- **The scene** (scene 3, task main `ListeningPoint_Main` 0x53230, work
  `lpwork` through the task table). `calclisteningpoint` (0x530c4) adds up
  points from each trooper's song's play count (`SysiPodList_Get_PlayCount`);
  this is where the play counts from the music library matter. The scene
  scrolls the card list by itself (`CardList_AutoScroll`) and reveals points
  and levels. Input:
  - three "tap to continue" steps (0x53474, 0x5350e, 0x5356a): any finger-up
    anywhere, SE 9, next step;
  - **SKIP**: the icon `lpwork+0x8c` (a `SysAnim`) at the top right. While it
    shows, a non-flick finger-up at x ≥ 440, y ≤ 40 either opens the "Skip?"
    `SysDialog` (`ListeningPoint_SkipCheck`, dialog at `+0x90`, cancel
    enabled; result 0 skips to phase 0x18, 1 or cancel resumes) or, during
    the auto-scroll, sets `+0x80` = 1 so the dialog opens next
    (`ListeningPoint_SkipCheck2`). `+0x80`: 0 idle, 1 asked, 2 dialog open.
  - Controller: confirm taps the centre (the no-menu fallback already does
    this); Start taps (460, 20) only while the icon shows and `+0x80` is 0
    (a tap there while it's hidden would count as "tap to continue"); the
    dialog is an ordinary `SysDialog`. The icon is shown when its
    `SysAnim` slot (below) is in use and its shown flag is 1.

**`SysAnim` slots** (read from code 2026-09-27): `_anim_work` (0x13dcd8),
512 slots of 0x5c bytes (exactly up to `_tacticsmapterms_work`), indexed by
the animation id `SysAnim_Open` returns. `+0x0` state (0 = free; > 3 =
loaded, only then are its sprites touched), `+0x8` set to 1 on open, `+0xc`
**shown** (`SysAnim_Set_Disp` writes it; `SysAnim_Get_Disp` returns it, or
−1 for a free slot), float `+0x10/+0x14` position and `+0x18` z
(`Set_Position`), `+0x2c` flags, `+0x3c` sprite count, `+0x40/+0x44`
animation / frame, `+0x50` → its sprite ids, `+0x58` → frame table. So an
animation is visible when 0 ≤ id < 512, `+0x0` ≠ 0 and `+0xc` == 1.
- **In battle (phase 13)**, `TacticsListeningPoint_Main` (0x52520), global
  `_tac_listening_point` (0x14e2b0): `+0x0` state, `+0x8` the bonus
  (`getlpbonus`; the phase only runs when there is one), `+0x10` window,
  `+0x14/+0x18` messages, `+0x1c` a `SysDialog` (`{0x14, 0x1e, 0x1f}`, no
  cancel). Result 0 applies the bonus (`_savedat + 0x7f64/68/6c`, the groove
  gauge, `clearlpbonus`), waits 10 frames, SE 8, ends; result 1 declines.
  Only 0 and 1 are handled, so a tap off the buttons does nothing. Nothing new
  for the controller: it's a plain `SysDialog`.

**Colosseum target (phase 14)** (read from code 2026-09-27): **no
input.** `TacticsColosseumTarget_Main` (0x521c0) never reads touch. It's a
roulette over the candidate enemies, global `_ui_battling_target`
(0x14e228): `+0x0` state, `+0x4/+0xc` counters, `+0x10` frames per step
(grows, so it slows), `+0x14` the highlighted candidate, `+0x18` how many
(1 skips the spin), `+0x1c + 4i` the candidates, `+0x7c/+0x80/+0x84`
animations. It stops on the unit chosen beforehand
(`TacticsUnit_Search_SortieUnitColosseumTarget`), plays the target
animation, moves the camera to it and restores the saved zoom
(`_savedat + 0x7f58`), then ends after 30 frames. Nothing for the
controller; a confirm tap then is ignored. (The Colosseum scene itself,
`Colosseum_Main` with its mode/menu/explain selects, is a separate scene,
not read.)

**Tap-anywhere screens** (read from code 2026-09-27):
- **Tutorials** (`TacticsTutorial_Main` 0x4baac, global
  `_tacticstutorial_work` 0x14e1e4; text from `_tacticstutorial_messagetable`,
  art from `_tacticstutorial_graptable`). While one runs, `Tactics_Main`
  skips the battle phase. Each step shows the "tap to continue" key mark
  (`SysMessageKeyMark`), clears touch, and waits for a finger-up anywhere
  (the position is never read), then SE and the next step. The phase modules
  start them with `TacticsTutorial_Check_SceneStart(n)`.
- **Battle intro** (`TacticsBattleStart_Main`), **between fights**
  (`TacticsBattlingNext_Main`) and **game over** (`TacticsGameOver_Main`):
  only `SysTouch_Get_EndedState`, no position: tap anywhere.
- **The attack animation** (`TacticsAttack2`, damage, effects), unit end,
  phase start and results read no touch at all.
  But a script can run in any of them (`_tactics_script_flag`, e.g. a map
  script's "You found a buried treasure chest!" in unit end, seen
  2026-09-28), and its text takes a tap; so can any message showing the
  "tap to continue" mark (`_mesmanage`: `+0x0` its `SysAnim`, `+0x4`
  enabled).
Controller: confirm taps the centre. **While a tutorial is active the battle
input must stand down**: no held virtual finger (its lift would be what
advances the tutorial, and the phase underneath isn't running), and no
cursor; confirm is a plain tap. Tell by `_tacticstutorial_work`'s state (or
the key mark being shown); read its layout in step 1.

**Decoded 2026-09-27 for the controller** (`inspect/ssdis.py`; each is a
named `const` in `game_input.rs` with its switch address):
- Command ring: `_tacticsunitmenu_work + 0x0` is the screen
  (`TacticsUnitMenu_Main`'s `___switch8`, 0x1de6c): 0 the ring
  (`CtrlTop`), 1 the skill panel, 2 the item window, 3 the status. The ring
  takes touches in 0 with no state of its own; a turn runs while `+0x24`
  (a flick's spin) or the gap to `+0x1c` (the item being brought round) is
  non-zero, so "still" is `+0x14` unchanged. Items can be greyed out:
  `CtrlTop`'s front-item switch (0x1dada) checks an enabled flag per
  command (`+0x30`, `+0x34`, `+0x38`, `+0x3c`); a tap on a greyed one
  plays SE 1 and does nothing. Cancel (a finger-up outside 100–380 ×
  40–280) goes back to unit select, or, if the unit has moved and not
  acted, undoes the move (`TacticsMoveWalking_BackUp_Pos`) and stays on the
  ring.
- Skill panel: sub-state `+0x4` (`CtrlSkill`'s switch, 0x1d6f8): 0 sets up
  and clears the touch, **1 takes touches**, 2 a skill was picked (phase
  24), 3 closes it. It never scrolls: the row is `(y − 8) / 60`, so only
  rows 0–4 are on screen. A finger on the list side shows the cursor and
  area at once (no hold needed), and its finger-up there doesn't check
  for a flick.
- Move select: `_tacticsmoveselect_work + 0x0` (tested at 0x21af8): **0
  takes touches** (the 5-argument `Tactics_CtrlTest`, whole screen), 1 a
  tapped tile's 5 frames before the walk. A **quick tap** on a tile that
  isn't reachable goes back to the command ring too (0x21cd4), like a lift
  on the unit's own tile.
- Attack select: `_tac_attack_select + 0x0` (tested at 0x2fcee): **0 takes
  touches** (whole screen), 1 a picked target's 5 frames. `+0x72c`
  (`CtrlRelease`'s switches at 0x2f7f8 and 0x2f91a): 0 and 1 look up the
  target list, 2 and 3 the area list (2 also turns the attacker to the
  tapped tile and rebuilds the list first). Attack info has no state of its
  own: only the sub-phase and the zoom.
- Zoom: `TacticsMap_Pinch` calls `TacticsMapCursor_Set_Zoom(pct, -1)` (the
  2-argument form, mode 0: at once). Mode 0 animates in even steps, 1 halves
  the gap each frame. The controller calls `__Z25TacticsMapCursor_Set_Zoomiii`
  with (pct, 6, 0).
- Deploy: `_sortieselect_work + 0x0` (`TacticsSortieSelect_Main`'s
  `___switch16`, 0x37158) as listed above, 13 being the party-full
  dialog's handler; placing (4) and the map view (8, sub-state 1) use the
  whole-screen `Tactics_CtrlTest`. The sort panel takes touches in
  sub-state 0 (0x37c94). The map view's sub-states (switch at 0x37fca): 0
  set up, 1 `CtrlTest`, 2 the back icon's animation, 3 a unit's status;
  its back icon only takes a tap, not a hold's release.
- Victory terms: the switch at 0x1a1a2 confirms states 0–7 as above; 7 is
  "done" and stays until the next `Start`. `TacticsMapTerms_End` (and
  `_Init`) set the animation and window ids at `+0x10` / `+0x14` to −1,
  which is how the controller tells they're up.

### Battle controller plan (decided 2026-09-27)

The remaining steps (3–7 below, plus zoom, sortie, Listening Point and
terms) are itemised for implementation in
[`battle-controller-plan.md`](battle-controller-plan.md).

- **Tile cursor on grid axes** (FFT / Tactics Ogre style): the D-pad steps
  map x ± 1 or y ± 1, which is diagonal on screen. Which D-pad direction is
  which axis is picked on screen in step 1. touchHLE draws the cursor (a
  diamond version of the focus outline); the game's own cursor only follows a
  held finger.
- **Unit select:** free cursor; L/R tap the curved arrows (previous / next
  unit); confirm taps the cursor's tile; Start taps MENU.
- **Command ring:** D-pad left / right tap the neighbouring item (the game
  turns the ring to it), confirm taps the front item (`+0x18`), back taps
  outside the ring (cancel).
- **Cursor style (proposed):** unit select uses the drawn cursor (it roams
  the whole map and there's no auto-scroll). Move and attack select use a
  **held virtual finger** so the game's own cursor and previews show (facing,
  spread area, target info): pan so the range is on screen, press on the
  start tile, slide the finger tile to tile, confirm = lift. If the next tile
  is off-screen: slide to a tile that does nothing, lift, pan, press again.
  To be proven in step 4 before it's final; the drawn cursor is the fallback.
- **Move select:** the cursor is **locked to reachable tiles**: a D-pad step
  goes to the next listed tile in that grid direction, else the nearest one
  roughly that way, else stays. It skips the unit's own tile, because
  letting go there cancels the move; that's what **back** does (slide home,
  lift: the command ring comes back). Confirm acts at once; the game has no
  confirmation step.
- **Attack select:** the cursor is locked to the target list; shoulders
  cycle targets. Letting go of a held finger off a target does nothing, so
  **back** = slide off the targets, lift, then quick-tap a non-target tile
  (SE 3, back to the command ring).
- **Zoom:** triggers step `TacticsMapCursor_Set_Zoom` within 100–200%, a few
  frames animated, only in phases 19, 22 and 24. A viewing aid only: it
  isn't saved, so a resumed battle opens at the last pinch zoom (accepted).
- **Off-screen tiles** are panned to with a one-finger drag from the centre
  (the map scrolls on a flick), repeated until the tile is on screen, as on
  the world map.
- Pure logic (grid, projection, cursor steps, snapping, pan steps) goes in a
  new host-only `song_summoner/battle.rs` with unit tests written first from
  these notes; `game_input.rs` only reads guest memory and injects.

Steps:
1. **Done 2026-09-27** (`battle.rs`, `game_input.rs` `battle_debug`, debug
   builds only). Confirmed in play at 130% zoom: the tile outlines sit on
   the game's tiles and follow the camera; a tap's tile is the unit the
   game selects; the held-finger tile matched the game's cursor on the next
   frame 167 times out of 167 over a drag (so the 36-point hold offset is
   right); move select lit 16 tiles in the highlight layer, the same 16 as
   the game's reachable-tile list. Attack select's lists (unit targets,
   area tiles, mode `+0x72c`) are logged too but not yet seen. Planned as:
   read-only: read the phase, grid, units, camera and zoom; log them; draw an
   outline on every tile as a debug overlay to prove the projection
   (including the −36 / scale question) against the real screen. Log on
   change: phase / sub-phase, touch position, the `Tactics_CtrlTest` gesture
   (work `+0x20`: 1 flick, 2 hold, 3 pinch), the game's cursor tile
   (`_tacticsmap_work + 0x5d8`), the zoom, and the tile our projection puts
   under the touch. One hold-and-drag then shows whether the two agree. (The
   log as of 2026-09-27 only has task lists and menus; it did confirm the
   MENU screen is found as a `SysButtonMenu` at (240, 160) with 4 buttons.)
2. Unit select: cursor, confirm, L/R, Start, panning. **Written and
   verified in play 2026-09-27** (log: outline moves on the grid axes,
   confirm opens the ring / enemy status, L/R and MENU work). The game's
   first tap on a unit that isn't the selected one only selects it (state
   7, the cursor and camera go to it). Confirm is a held finger instead
   (see "Tap vs hold" above): it picks by tile and acts in one, verified
   in play 2026-09-27 (gesture 2 every time, the game's cursor on our
   tile). A tap only at the map's bottom edge, where a finger 36 below
   can't be held (there an unselected unit takes two presses). The finger
   stays down while confirm is held (at least 10 frames), so the game's
   cursor and status panel preview the unit; letting go acts. Verified in
   play 2026-09-27 (finger down / up one frame after the button). It goes
   on any tile, as a finger would (user's choice): on empty ground the
   game's cursor goes there while held, and returns to the selected unit
   on release (the game's rule). Only the bottom-edge tap skips empty
   tiles (`confirm_picks`).
   The diamond cursor: `FocusShape::Diamond` in `gles/present.rs`.
   Code: (`battle.rs` `grid_step`, `pan_toward`,
   `unit_select_intent`; `game_input.rs` `unit_select`). Option A: up = y −
   1 (up-right on screen), right = x + 1, down = y + 1, left = x − 1. Acts
   only with sub-phase 0x65, `_tac_unit_select` state **1** (first written
   as 0, which is only its one-off setup: nothing worked; see below), no
   menu, no
   tutorial (`_tacticstutorial_work + 0x8`), and the camera still; commands
   wait otherwise. A pan is a one-move flick from (240, 128), half the
   distance to the target (`TacticsMap_Scroll` moves the camera twice the
   finger's movement; `SysTouch_Moved_F1` sets the flick flag at once), and
   its release puts the game's cursor back on the selected unit, which the
   controller's cursor ignores. State 3 (another team's unit's status,
   read from 0x39600, to confirm): confirm / back tap (320, 160) to close,
   north (80, 160) to flip.
   **Changed 2026-09-28 (not yet tested):** a virtual finger now rests on
   the controller's tile the whole time, so the game's own cursor (under the
   units) and status panel follow it; confirm is the release. See
   `battle-controller-plan.md` §7.
3. Command ring. **Written 2026-09-27, not yet tested in play**
   (`ring_top`, `battle::ring_command`), with its status and skill panel
   (`skill_panel`: a held finger on the row, confirm lets go, back slides
   off, lets go and taps the map side). The item window is the existing
   `List` widget.
4. Move select. **Written 2026-09-27, not yet tested**: `locked_select`
   with `battle::follow`, a held finger (`Finger`: down, then still for 8
   frames before it may move or lift) on a cursor that starts on a
   reachable tile and goes anywhere on the map (unlocked 2026-09-28 at the
   user's request). Back slides to the unit's own tile and lets go.
5. Attack select, then attack info. **Written 2026-09-27, not yet
   tested**: the same finger, starting on the list `+0x72c` picks, free on
   the map (2026-09-28); L/R cycle the list; back lets go where nothing happens, then taps nothing
   (`battle::harmless_tap`). Attack info taps its fixed rects.
6. Zoom on the triggers. **Written 2026-09-27, not yet tested**: L2/R2 are
   turned into buttons in `window.rs` (`trigger_edge`), and step
   `Set_Zoom` by 20% in phases 19, 22 and 24; a held finger comes up
   first and goes down again after.
7. Sortie (phase 8): **written 2026-09-27, not yet tested**: placing (the
   same finger, free on the map since 2026-09-28), the sort panel, the map view,
   and the party-full dialog opening on No. Listening Point: Start taps
   SKIP while its icon shows. Colosseum target and walking drop presses;
   tutorials keep the centre tap.
   Victory terms: confirm closes them in state 4; earlier presses are
   dropped.

### Results: splitting Pitch Pearls (read from code 2026-09-28)

- Scene `Result_Main` (0x47fb8), work through the task table: `+0x0` the
  flow (`ResultFlow_Change_FlowNumber` also zeroes `+0x4`; `Result_Main`'s
  `___switchu8` at 0x47fc4): 12 `ResultFlow_DivideQuestion` (the "Use the
  Pitch Pearls earned on the Trooper you deployed?" `SysDialog`), **13
  `ResultFlow_DivideSelect`** (0x46ba4), 14 `FighterRankUp`, 17
  `EndQuestion`.
- `DivideSelect`'s sub-state `+0x4` (`___switch16` at 0x46bbc): 0 set up,
  **1 takes touches**, 2 a message (any finger-up closes it), 3 the
  "Rank up this Trooper?" `SysDialog` (`+0x44`), 4 EXIT's animation, 5 the
  end dialog. `+0xc` the selected trooper, `+0x1c` how many.
- On a finger-up at y 228-300, slot i is x 156 + 64i to 204 + 64i (i < 5):
  slots below `+0x1c` are troopers, slot 4 is EXIT. Another trooper: SE 2,
  it's selected (its status shows). The selected one: SE 9, and if its
  rank (`_savedat + 0x3264` per fighter) is at most 2 and the pearls
  (`_savedat + 0x68`) cover `getneedpitchpearls`, the rank-up dialog;
  otherwise SE 1 and a message. EXIT (never the selected slot): SE 9,
  leave (sub-state 4).
- Controller (`game_input.rs`, `result_divide`): a two-tap `Scene` group
  over the troopers (left/right select, confirm on the selected one asks
  to rank up); back taps EXIT, which the D-pad doesn't reach since one tap
  on it leaves.

### Controller coverage scan (2026-09-28)

Every function that reads touch (callers of `SysTouch_Get_EndedState`,
`_PosData`, `_BeganState`, `_TouchState`, `SysPrim_Touch_DrawRect`,
`SysTouch_Check_InRect`), sorted by what the controller does there.

**Handled:** `SysButtonMenu`, `SysDialog`, `SysDrum2`, `SysMenu_Check2`
lists, the card list, Options, Help, the Hip-O-Drome, Teammake, towns, the
world map and its location menu, cutscene and Listening Point SKIP, every
battle phase module, the Results pearl split.

**Only wait for a finger-up anywhere** (the controller's centre tap
covers them): `Ending_Main`, `Colosseum_Main`, `Shop_Main`,
`ShopFlow_Exit1st`, `ShopFlow_PasswordInput`/`_PasswordTutorial` (around
the keyboard), `StatusSlot_Check`/`2`, `ScriptTouch_End`, `odemomain`,
`TacticsHarmony_Main`, `TacticsSilentBox_Main`, `TacticsContestWin`/`Lose`,
`TacticsMapMenu_Loop_CtrlResume`, `TacticsMessage_Main` (between fights,
phase 34; see below), `TacticsMapFind_Loop_*` (the buried
treasure; run from `RunScript`, so the script flag is set), and the Result
flows (`TacticsScore`, `TacticsGetItem`, `SortieLimit`, `FighterLost`,
`Get_EnemyDrop`, `AddDropNum`, `DivideQuestion`, `FighterRankUp`,
`EndQuestion`, `FreeMapTutotrial`).

**Missed: look at where the finger is:**
- The shop: done since (the lists, the quantity dial, the password spot
  and keyboard; see above).
- `Catalog2_Main` (scene 8, the catalog): sprite hit tests, positions and
  flicks.
- `PalaceFlow_Recommend` (the Hip-O-Drome's Pick of the Pops): a sprite
  hit test (the song's artwork) next to a "tap to continue" message;
  check the centre tap doesn't land on the artwork.
- `ColosseumFlow_TetsujinGet`: positions and flicks around a talk message
  and a card; probably tap to continue, to check.
- `TitleFlow_Product`: the "other games" line-up; its icons open App Store
  URLs, which would background the app (touchHLE quits). The controller
  must only tap its back icon (`tc_icon`).

**`TacticsMessage_Main`** (0x38820, work `_tacticsmessage_work` 0x14d7c0;
decoded 2026-09-28). `TacticsMessage_Start(mode, message, y)` stores the
mode at `+0x8`. Mode 1 is a two-choice message: a finger-up at x 80-240,
y 200-240 (sprite `+0x18`) plays SE 9 and returns 1; x 240-400 (sprite
`+0x1c`) SE 3 and 2; any other still finger-up also SE 3 and 2, a flick
nothing. Any other mode: a finger-up anywhere, SE 9, returns 1. Its only
caller, `TacticsBattlingNext_Main` (between fights, phase 34, sub-phase
0x64 start, 0x65 run, 0x66 end), passes mode 0 and treats any result as
"go on": so the centre tap works, and the two-choice mode is unused.

**Debug scenes, not reachable in play:** `Test1`-`Test7`, `AnimTest`,
`AnimTest2`, `iPodTest`, `SoundTest`, `TouchTest`, `MessageTest`,
`ScriptTest`, `StageSelect_Main` (adds bench units, clears the save),
`TouchDebug_Main` (always running, draws nothing in play).

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
4. **Battle** (planned 2026-09-27, see "Battle controller plan"): tile
   cursor on the grid, command ring, move / attack targeting, zoom. Step 1
   (reading the grid and proving the projection) done 2026-09-27.
5. **Town, world map, formation, password keyboard**, one at a time.

## Tools (`inspect/`)

Extract `Payload/S.S.Encore.app/S.S.Encore` from your own IPA into the
folder you run these from. Never commit it.

- `mo.py`: Mach-O/ObjC2 parser (classes, ivars, methods, selrefs,
  classrefs, stubs, symbols). **`SYMS` clears bit 0 of every address**
  (for Thumb functions), so a data symbol at an odd address shows one
  byte low, and `ssdis.py` then names the byte after it wrongly.
  `_skip_able` is 0xa718d, not 0xa718c. Check the raw symbol table for
  byte-sized data.
- `ssdis.py <hexaddr | "-[Class sel]">`: annotated Thumb disassembly.
  Resolves literals, ivars, cfstrings, and `objc_msgSend`
  receiver/selector.
- `xref.py <hexaddr>...`: every BL/BLX caller, with the owning function
  and the `r0` immediate (handy for setter calls).
