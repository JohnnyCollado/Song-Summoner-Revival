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
