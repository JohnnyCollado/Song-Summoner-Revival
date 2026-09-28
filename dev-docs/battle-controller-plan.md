# Song Summoner: battle controller support (implementation plan)

Status: **B0–B9 written 2026-09-27 in one pass; unit-tested only, not yet
built or tried in play.** See "Changed in implementation" at the end for
where the code differs from this plan. Steps 1 and 2 of the battle
plan in `song-summoner-re.md` ("Battle controller plan") are done and
verified in play: the grid projection, the diamond tile cursor, and unit
select (phase 19). This file covers everything left, written to be
implemented in one pass by someone who hasn't followed the conversation.
Read it top to bottom before starting; §1–§3 apply to every task.

Sources of truth, in order: the user's decisions (§2), then
`dev-docs/song-summoner-re.md` §"Battle (`Tactics_Main`)" (addresses,
struct layouts, game behaviour confirmed in play), then the disassembly
(`inspect/ssdis.py`). Where this plan and the RE doc disagree, the RE doc
wins; fix this plan.

---

## 1. Working rules (from `CLAUDE.md`, and lessons from steps 1–2)

1. **Tests first.** For every behaviour, write the unit tests from this
   spec (or the RE doc) before the code. Pure logic goes in the host-only
   `src/frameworks/song_summoner/battle.rs` (no `Environment`, no guest
   memory) so it can be tested; `game_input.rs` only reads guest memory
   and injects touches.
2. **No guest memory writes.** Input only through the game's own
   functions, once per frame, from `before_frame`: `SysTouch_Began_F1` /
   `Moved_F1` / `Ended_F1` (via `TapQueue`), and plain setters/getters.
   **Never call a function that uses `SysTask_Get_WorkAdr()`** (most map
   and phase functions): between frames it reads past the task table.
   `TacticsMapCursor_Set_Zoom` is known safe (RE doc, "Zoom").
3. **Decode state switches before using a state value.** Step 2 first
   gated on `_tac_unit_select` state 0 because it "looked idle"; the real
   "taking touches" state was 1, and nothing worked while every test
   passed. For every `+0x0 state` this plan uses, disassemble the owning
   `*_Main` / `*_MainCtrl` switch with `inspect/ssdis.py` (Thumb: pass the
   address + 1, e.g. `python ssdis.py 399c5`, run from `inspect/`), name
   the value as a `const` with the switch address in its comment, and add
   it to the RE doc.
4. **Log why presses wait.** Every gated handler logs, once per change,
   why queued presses are waiting (see `unit_select`'s "presses waiting"
   line). Game-state and finger logs are debug-build only
   (`cfg!(debug_assertions)`), like `battle_debug`.
5. `log!` isn't an expression: wrap it in a block in `match` arms
   (`=> { log!(...); }`); grep for `=> log!` before handing off.
6. Code style: `dev-docs/code-style.md` (MPL header, `//` comments ≤ 80
   columns, comments explain *why*). `rustfmt` can't format `msg!` /
   `objc_classes!`; do those by hand.
7. Don't run builds, tests or the game yourself unless the user says so;
   hand the user the commands (PowerShell, one command per fenced `bash`
   block, from the repo root). Don't commit unless asked.
8. The user tests in play and sends `debug/windows/touchHLE_log.txt`.
   Each task below lists the log lines that prove it works: make sure
   they exist.

Commands for the user (repo root):

```bash
cargo test battle::
```

```bash
cargo test game_input::
```

```bash
cargo debug-windows
```

---

## 2. Decisions already made (don't reopen)

- **Tile cursor on grid axes**, D-pad option A: Up = y − 1, Right =
  x + 1, Down = y + 1, Left = x − 1 (`battle::grid_step`). touchHLE draws
  it as a gold diamond (`FocusShape::Diamond`).
- **Unit select:** free cursor; L/R tap the curved arrows; Start taps
  MENU; confirm holds a virtual finger 36 points below the tile for as
  long as the button is held (at least 10 frames), on any tile; release
  acts (done).
- **Held virtual finger (finger-follow)** wherever the game shows a
  preview while a finger is down: move select, attack select, the skill
  panel, deploy placing. The game's own cursor and previews show.
- **Move select:** cursor **free on the map** (changed 2026-09-28; was
  locked to the reachable tiles); no confirmation step (the game moves at
  once). Letting go on a tile that isn't reachable does nothing.
- **Attack select:** cursor free on the map too (changed 2026-09-28; was
  locked to the target list); shoulders still cycle the targets.
- **Zoom:** `TacticsMapCursor_Set_Zoom`, not two-finger injection; 100–
  200%; only in phases 19, 22, 24. It isn't saved (accepted by the user).
- **Deploy "party full" dialog:** focus starts on **No** (button 1), so
  a fast presser isn't locked into the battle.
- **Tutorials:** battle input stands down while one shows; confirm is a
  plain tap anywhere (already works via `battle_tutorial`).
- Known and accepted: in unit select, the game's own cursor doesn't
  visibly move while the finger is held; it updates on release.

---

## 3. What exists (read these first)

`src/frameworks/song_summoner/battle.rs` (host-only, tested):
- `Camera { origin, scale }`: `tile_centre`, `tile_rect`, `tile_at` (tap
  → tile), `hold_tile_at` (held finger → tile, 36 pt above),
  `hold_point` (tile → where to hold).
- `Grid { w, h, units, highlight }`: `contains`, `unit_at`.
- `grid_step`, `Dir`, `unit_select_intent`, `UnitSelectIntent`.
- `TAP_AREA`, `TAP_CENTRE`, `tappable`, `holdable` (finger must stay in
  `Tactics_CtrlTest`'s rect (0, 0, 480, 256) for unit select),
  `SELECT_HOLD_FRAMES`, `pan_toward` (a one-move flick that scrolls the
  map; `TacticsMap_Scroll` moves the camera 2× the finger), `PAN_MAX`,
  `confirm_picks`, `HOLD_OFFSET`, `PREV_UNIT_ARROW`, `NEXT_UNIT_ARROW`.

`src/frameworks/song_summoner/game_input.rs`:
- `Game` (symbols, looked up once in `lookup`; optional ones default to
  0), `read_battle` → `Battle { work, phase, sub, gesture, camera, grid,
  cursor, zoom }` (phase ≥ 5 only).
- `run_commands`: battle's unit select first (phase 19, no widgets, no
  tutorial), else `menu_commands` (dialogs, button menus, lists, card
  list, the no-menu fallback that taps the centre, SKIP).
- `unit_select`: the pattern to copy (state gate, camera-still gate,
  follow the game's cursor except during our own gestures via
  `battle_touching`, pan then act, "presses waiting" log).
- `TapQueue` + `Gesture::{Tap, Drag, Slide, Pan, Hold}` + `keep_down`;
  `before_frame` makes one `TouchStep` call per frame.
- `handle_pad_button` gets presses **and releases** (`confirm_down`).
- Debug: `battle_debug` (state, touch tile, overlay), `read_tile_list`,
  `battle_targets` (move list; attack lists with mode `+0x72c`),
  `log_battle_targets`, frame-stamped `confirm down/up` and `finger
  Began/Ended` lines.

`src/frameworks/song_summoner/pad.rs`: `Role` (Up, Down, PrevSection =
D-pad left, NextSection = D-pad right, PrevTab = L, NextTab = R,
Confirm, Back, Info = north, Skip = Start). `pad::repeats`.

Rendering: `show_focus(env, main_view, Option<FocusMarker>)`, shapes
`FocusShape::{Brackets, Diamond}` (`src/gles/present.rs`).

---

## 4. Task list

Order matters: B0 is shared infrastructure the rest depend on. Each task
has **Decode** (RE to do first), **Behaviour**, **Tests first**,
**Implement**, **Verify in play** (what the user should see and the log
lines).

### B0. Shared infrastructure

**B0.1 Phase dispatch.**
- Behaviour: `run_commands` hands commands to one battle handler per
  phase whenever `read_battle` succeeds, no tutorial shows, and no
  widget is open over the map (dialogs, the MENU's button menu, the item
  window's list and Options still win). Phases 20, 22, 24, 25 and 8 get
  handlers (B1–B6). In phases the player can't drive (23 walking, 26+
  attack animations, 17/18 scripts, 14 Colosseum target) **swallow** the
  commands instead of falling through to the centre tap, except confirm
  on the tap-anywhere screens (battle intro, between fights, game over),
  which keep the existing centre tap. Reset `battle_confirm`,
  `battle_pan`, the held finger (B0.2) and the cursors when the phase
  changes.
- Tests first: a pure `battle::phase_owner(phase, sub) -> Owner` (enum:
  `UnitSelect`, `Ring`, `MoveSelect`, `AttackSelect`, `AttackInfo`,
  `Sortie`, `Swallow`, `TapAnywhere`) with one test per row of the phase
  table in the RE doc.
- Implement: split the phase-19 branch of `run_commands` into a
  `battle_commands` that matches on the owner. Keep `unit_select` as is.

**B0.2 A held finger that slides.** `Gesture::Hold` + `keep_down`
covers press-and-release on one spot. Move/attack select, the skill
panel and placing need a finger that stays down and moves between spots.
- Behaviour (a small state machine on `TapQueue`, or a sibling struct
  driven from `before_frame`; one `TouchStep` per frame, never Began and
  Ended before the same frame):
  - `press(x, y)`: Began. The finger is down until `lift`.
  - `slide_to(x, y)`: one `Moved` to the new point. **Not before
    `MIN_STILL_FRAMES` (8) frames after the press**: `Tactics_CtrlTest`
    only calls a finger a hold after more than 6 still frames, and
    `SysTouch_Moved_F1` sets the flick flag at once, so an early move
    would turn the hold into a map scroll. Queue it until then.
  - `lift()`: Ended where the finger is, again no earlier than
    `MIN_STILL_FRAMES` after the press (a quick confirm still has to
    count as a hold, as in unit select).
  - `finger()` → `Option<Point>`: where the finger is (or will be once
    queued steps run). `is_idle()` keeps meaning "no queued steps"; a
    finger that is simply down and still counts as idle so handlers can
    queue the next step.
  - `clear()` drops a held finger without an Ended (as today); handlers
    must lift instead where the game would notice.
- Tests first (in `game_input.rs` tests, like the `TapQueue` ones):
  press → Began then nothing while held; slide before 8 frames waits,
  after 8 is one Moved(to, from); lift after a slide is Ended at the new
  point; a quick press+lift still lasts 8 frames; two slides queued are
  two Moved on two frames; `finger()` tracks queued steps.
- Verify in play (first thing in B3): the log's battle lines show
  `gesture 2` **staying 2** across slides, and the game's cursor tile
  (`game cursor (x, y)`) following each slide on the next frame. If a
  slide flips the gesture to 1 (flick), stop and report: the whole
  finger-follow design depends on this (RE doc: a real finger dragged
  after a hold does keep it, 167/167).

**B0.3 Locked cursor steps.**
- Behaviour: `battle::locked_step(list: &[Tile], from: Tile, dir: Dir,
  skip: &[Tile]) -> Tile`. Moving `dir` (grid axes, option A): among
  the listed tiles not in `skip` that lie in that direction (positive
  component along the axis), pick the best by `along + 2·|across|`
  (smallest), ties by list order; if none, stay on `from`. Also
  `locked_start(list, near: Tile, skip) -> Option<Tile>`: the listed tile
  nearest `near` (Manhattan, ties by list order), for entering a phase.
- Tests first: straight line picks the next tile; a gap is jumped; a
  tile off to the side is reached when nothing is straight ahead; the
  skip list is honoured; empty list → stays; `locked_start` on an empty
  list → None.

**B0.4 A harmless spot to lift the finger.**
- Behaviour: letting go of a held finger on a tile the phase doesn't
  accept does nothing (move select: not reachable and not the unit's own
  tile; attack select: not in the active list; placing: not a deploy
  tile). Needed to lift before a pan and for "back".
  `battle::harmless_point(camera, grid, accepted: &[Tile], area) ->
  Option<Point>`: a finger point inside `area` whose `hold_tile_at` isn't
  in `accepted`, preferring the one closest to the screen centre (search
  a coarse lattice of points, e.g. every 8 pt).
- Tests first: the result's hold tile is never in `accepted`; with every
  on-screen tile accepted, a point whose hold tile is off the grid is
  used; None only if nothing qualifies.

**B0.5 Holdable areas per phase.** Unit select's `CtrlTest` rect is
(0, 0, 480, 256); move select, attack select and placing use the whole
screen (the 5-argument overload, (0, 0, 480, 320)) — **decode** which
overload attack select and placing call. Generalise `holdable` to take
the rect: `holdable_in(centre, rect)` (finger = centre + 36 must be
inside the rect with a 4 pt margin, and the tile in `TAP_AREA`'s x
range). Keep `holdable` as the unit-select wrapper. Tests: the existing
ones, plus the whole-screen rect accepting a tile at y 260.

**B0.6 Triggers as buttons** (for B5).
- Behaviour: L2/R2 are axes in SDL. `window.rs` turns
  `ControllerAxisMotion` for `TriggerLeft` / `TriggerRight` into
  `Event::ControllerButton` with new `PadButton::LeftTrigger` /
  `RightTrigger`, pressed above 50% and released below 25% (hysteresis,
  so a half-pull doesn't chatter). `pad::role` maps them to new
  `Role::ZoomOut` / `Role::ZoomIn`; the picker ignores them (check every
  exhaustive `match` on `Role` / `PadButton`, e.g. in `picker_view.rs`
  and `glyphs.rs`); `pad::repeats` stays false for them.
- Tests first: a pure `trigger_edge(was_down, value) -> Option<bool>`
  for the hysteresis; `pad::role` for both triggers.

---

### B1. Command ring (phase 20)

**Decode:** `TacticsUnitMenu_Main` / its ctrl switch: which field of
`_tacticsunitmenu_work` (0x149b80) says whether the top ring, the skill
panel, the item window or the status is running, and which value means
"taking touches" at the top level (name the constants). Also: can a ring
item be greyed out, and what does tapping one do (open question in the
RE doc)? Look for an enabled flag in `TacticsUnitMenu_CtrlTop`'s hit
test.

**Behaviour** (RE doc "Command ring"):
- D-pad left / right: tap the neighbouring item on that side of the
  front one. The game turns the ring to it with its own animation and
  sound. Wait until the turn has finished (`+0x14` angle unchanged for 2
  frames) before the next tap.
- Confirm: tap the front item (`+0x18` index; its sprite is at `+0x54 +
  4·i`, a centred prim: use the existing `read_prim_rect`).
- Back: tap outside the ring, (40, 160) (x < 100 cancels, SE 3, back
  to unit select).
- Up/down, L/R: nothing. No outline (the game shows the front item).
- A greyed item (if B1's decode finds them): confirm on it does what a
  tap does (probably SE 1, nothing); no special casing unless the decode
  shows a tap on it misbehaves.

**Tests first:** `battle::ring_neighbour(item_centres: &[Point], front,
side) -> Option<usize>`: of the items other than the front one, the one
on that side (x less / greater than the front's) whose centre is
nearest the front's. Tests with 7 centres on a circle at a few angles;
front at the top and at the bottom.

**Implement:** read the 7 sprite rects and `+0x14`/`+0x18` each frame;
gate on the decoded state and "ring still"; one tap at a time.

**Verify in play:** ring turns one step per D-pad press in the pressed
direction; confirm on Move opens move select (log `phase 22`), on Skill
the skill panel, on Item the item list, on Status the status; back
returns to unit select (`phase 19`). Log line on each tap: `input: ring,
tapping item i at (x, y) (front f)`.

### B2. Ring sub-screens

**B2.1 Status** (`TacticsUnitMenu_CtrlStatus`): north or confirm tap
(80, 160) (flip the page), back taps (320, 160) (back to the ring).
Same shape as unit select's state-3 status; share the helper. Verify:
page flips, back returns to the ring (`phase 20`, top level).

**B2.2 Item window** (`TacticsItemWindow`, a `SysMenu` at (120, 60),
240×200): **decode** whether it runs as a `Menumenumain` task (then
`open_widgets` already finds it as `Widget::List` and nothing is
needed) or is driven inside the ring's work (then add a reader from
`_itemwindow_work` 0x14e198 `+0x38`). Behaviour: the list as elsewhere
(up/down rows, confirm picks, back taps off the rows → SE 3 back to the
ring). A picked item goes to attack select (B4). Verify: log `menus
open: ["list"]` while it shows.

**B2.3 Skill panel** (`TacticsUnitMenu_CtrlSkill`, RE doc "Skill
panel"): finger-follow.
- Data: `_status_work` (0x14d4ec): `+0x24` skill count, `+0xc + 4·i`
  the skill in row i (> 0 = a skill). Rows are 60 high from y 8, list at
  x ≥ 320. **Decode:** does the panel scroll when count > 5 (only rows
  0–4 fit)? If it does, find how; if not, clamp to the visible rows.
  Also confirm the skill panel's touch is **not** offset by 36 (it isn't
  the map): hold at the row itself, e.g. (400, 38 + 60·i).
- Behaviour: on entering, press the finger on row 0 (the game's cursor
  and, for a skill, its area preview show). Up/down slide to the
  neighbouring row (clamped). Confirm: lift (a usable skill → attack
  select with its tiles; otherwise SE 1 and nothing: then press again on
  the same row so the preview is back). Back: slide off the list to
  (160, 160) and lift (a drag counts as a flick: the preview clears,
  nothing else), then a still tap at (160, 160), which closes the panel
  (back to the ring).
- Tests first: `battle::skill_row_point(i)`, `skill_row_at(y)`, clamp.
- Verify: holding shows each skill's area as you move; confirm on a
  usable skill → `phase 24` with the skill's tiles; back → ring.

### B3. Move select (phase 22)

RE doc "Move select". Reachable tiles: `*_tactics_move_select_target_list`
× `_cnt` (already read by `battle_targets`); includes the unit's own tile
(letting go there cancels to the ring); ready one frame before the tiles
light (wait for count > 0). CtrlTest rect: whole screen.

**Decode:** the "taking touches" condition for `TacticsMoveSelect`
(`_tacticsmoveselect_work` 0x14a924 state, switch in
`TacticsMoveSelect_Main`).

**Behaviour:**
- Enter: the cursor starts on `locked_start(list, own tile, skip =
  [own])`. Draw the diamond on it. Pan (`pan_toward`, as unit select)
  until it's holdable, then press the finger at its `hold_point` (the
  game's cursor goes there).
- D-pad: `locked_step(list, cursor, dir, skip = [own])`. If the new tile
  is holdable where the finger is (whole-screen rect): `slide_to` its
  hold point. If not: slide to `harmless_point(accepted = list)`, lift,
  pan toward the tile (repeat as needed, waiting for the camera to be
  still), press on it.
- Confirm: lift on the cursor's tile → the unit walks at once (phase 23,
  then back to the ring on arrival; no input in between).
- Back: slide to the own tile's hold point and lift → move cancelled,
  ring back. If the own tile isn't holdable: harmless lift, pan, press
  on the own tile, lift.
- Zoom (B5) while the finger is down: lift at a harmless point first,
  zoom, press again once the zoom animation ends (`_tacticsmap_work
  + 0x5e8` animating flag clear).
- Follow-the-game rules: ignore the game's cursor entirely here; ours is
  authoritative (the game's only moves with our finger).

**Tests first** (`battle.rs`): a pure planner
`move_plan(state, input, camera, list, own, finger) -> Vec<FingerStep>`
(`Press(pt)`, `Slide(pt)`, `Lift`, `Pan(dx, dy)`), covering: first
press after entry; a step to an on-screen tile is one Slide; a step to
an off-screen tile is Slide-to-harmless, Lift, Pan…, Press; confirm is
Lift; back is Slide-to-own, Lift; back with the own tile off screen.
The planner never emits a `Lift` while the finger is on a listed tile
other than for confirm/back (property test over a small map).

**Verify in play:** the diamond only ever sits on lit tiles; the game's
cursor follows it while held (log: `gesture 2` throughout and `game
cursor` = our tile); confirm walks there; back returns to the ring
without moving. Log `input: move select, <step> …` for every step.

### B4. Attack select (phase 24) and attack info (phase 25)

RE doc "Attack select" / "Attack info". `_tac_attack_select`
(0x14b70c): unit targets `(short x, short y)` at `+0x8`, count `+0x38c`;
area tiles at `+0x394`, count `+0x718`; `+0x72c` the mode (which list is
active). A hold's release on a listed tile picks it; anywhere else does
nothing. A **quick tap** off the targets is the game's back (SE 3, to the
ring).

**Decode:** `+0x72c` values (which one means "unit targets" and which
"area tiles"; `TacticsAttackSelect_CtrlRelease` 0x2f7b4 branches on it);
attack select's "taking touches" state (`_tac_attack_select + 0x0`,
`TacticsAttackSelect_Main` switch) and its CtrlTest rect;
`_tactics_attack_info_target_list_cnt` for phase 25.

**Behaviour (24):**
- Active list = by mode. Cursor starts on the first entry (the game's
  order). D-pad: `locked_step` in the active list. L/R: previous / next
  entry in list order, wrapping. Finger-follow exactly as B3 (press,
  slide, harmless lift + pan + press for off-screen tiles); the game
  shows facing and the spread area while held.
- Confirm: lift on the cursor's tile → picked (SE 9) → phase 25.
- Back: slide to a harmless point, lift (nothing happens), then a
  **quick tap** on a spot that picks nothing: `harmless_tap(camera,
  grid, targets)` → a point whose *tap* tile isn't a target and has no
  unit, and whose tile below it on screen (x + 1, y + 1) has no target
  unit either (taps on units go by sprite, and sprites reach up about a
  tile). → SE 3, back to the ring.
- Items (B2.2) and skills (B2.3) arrive here too; same handling.

**Behaviour (25):** confirm taps (240, 160) (the attack, phase 26);
back taps (240, 20) (back to 24); with more than one target, R taps
(420, 204) (next), L taps it count − 1 times (previous). One tap at a
time; wait for the zoom-in animation to finish (sub-phase 0x65 and the
map zoom not animating).

**Tests first:** `harmless_tap` (never a target tile, never a unit tile,
never above a target unit); list cycling with wrap; mode → list.

**Verify in play:** melee attack, a ranged attack and an area skill:
the diamond stays on valid tiles, confirm reaches attack info, back from
attack select returns to the ring, back from attack info returns to
target selection, confirm attacks. Log the active list and mode on entry.

### B5. Zoom on the triggers

**Decode:** `TacticsMapCursor_Set_Zoom`'s exact mangled symbol
(`inspect/mo.py` `SYMS`) and the `mode` argument the pinch passes
(`TacticsMap_Pinch` → its call; use the same mode and a short animation).

**Behaviour:** in phases 19, 22, 24 only (sub 0x65, no tutorial, no
widget), R2 zooms in and L2 out by 20 percentage points, clamped to
100–200, animated over 6 frames: call `Set_Zoom(new, 6, mode)` from
`before_frame` (a direct guest call, like `began`). Ignore presses while
a zoom is animating (`_tacticsmap_work + 0x5e8`). If a finger is held
(B3/B4), lift it at a harmless point first and press again after the
zoom (the hold point moves with the scale). The diamond follows
automatically (it's projected every frame).

**Tests first:** `battle::zoom_step(current, dir) -> Option<i32>`
(clamping; None at the limit).

**Verify in play:** triggers zoom in/out in steps, the diamond stays on
its tile, `zoom N%` in the battle log changes; nothing happens in the
ring or menus.

### B6. Sortie / deploy (phase 8)

RE doc "Sortie / deploy". `_sortieselect_work` (0x14d6e4): `+0x0`
state, `+0x28` deploy tile count, `+0x2c` → deploy tiles (0x14 bytes:
`short` x, y; `+0xc` card, −1 when free), `+0x40` the open dialog,
`+0x68` the sort panel's back icon, `+0xd8` the map view's back icon.

**Decode:** name the states used below from `TacticsSortieSelect_Main`
(0x37120) — card list 3, placing 4, after-place 5, common menu 6, sort 7,
map view 8, party-full dialog handler 13 — and which `Tactics_CtrlTest`
rect placing and the map view pass.

**B6.1 Party-full dialog → focus No.** When a `Dialog` widget appears
whose work is `read_u32(_sortieselect_work + 0x40)`, set the focus to
button 1 ("No"). Also fix the general rule: reset a dialog's focus to its
default whenever a dialog appears where none was open, not only when its
work address differs (a new dialog at a reused address kept a stale
index). Tests first: `choose_menu` / focus tests for "same address,
reopened" and for a per-dialog default index.

**B6.2 Placing (state 4)**, finger-follow like B3 with the list = deploy
tiles. While held the unit's sprite stands on the tile (tinted if taken).
D-pad `locked_step` over the deploy tiles (all of them, free or not: a
taken one is replaced). Confirm: lift → placed (or replaced), back to
the card list. Back: harmless lift (nothing), then a quick tap off the
deploy tiles → cancel to the card list (use `harmless_tap` with the
deploy tiles as targets). Start from the first free tile.

**B6.3 Sort panel (state 7):** a `SCENE_BUTTONS`-style group with Edit
Troopers' geometry (six rows, x 80–400, y 37 + 46·i to 83 + 46·i),
back taps the back icon (`+0x68`'s rect), or outside (x < 60) to cancel.

**B6.4 Map view (state 8):** the unit-select pattern (free cursor,
diamond, pan; confirm holds a finger while pressed, release on a unit
opens its full status); back taps the back icon (456, 296). In the
status (sub-state 3): north/confirm tap (80, 160) (flip), back taps
(320, 160) (close).

**B6.5 Existing widgets:** the card list, the common menu (a 5-button
`SysButtonMenu`) and its sub-menu, dialogs, Options: check in play that
each is found (`menus open: [...]` log) over the map; fix only what
isn't.

**Verify in play:** deploy a full party with the controller only; the
party-full dialog opens on No; back from placing returns to the card
list; sort and map view work; start the battle from the common menu.

### B7. Listening Point

RE doc "Listening Point". The scene's work (`lpwork`) is the
`__Z19ListeningPoint_Mainv` task's work (task table, like the world
map). SKIP is the `SysAnim` whose id is at `lpwork + 0x8c`; visible when
`0 ≤ id < 512`, `_anim_work` (0x13dcd8) slot `id · 0x5c`: `+0x0 ≠ 0`
and `+0xc == 1`. `lpwork + 0x80`: 0 idle, 1 asked, 2 dialog open.

**Behaviour:** Start taps (460, 20) only while SKIP is visible and
`+0x80 == 0` (a tap there while hidden would count as "tap to
continue"). Confirm: the existing centre tap. The "Skip?" dialog is an
ordinary `SysDialog`. In battle, phase 13's bonus dialog is a plain
`SysDialog`: nothing to add.

**Tests first:** `anim_visible(id, state, shown)` pure check.

**Verify in play:** Start skips (dialog → Yes → the results), confirm
advances each "tap to continue" step.

### B8. Victory terms (`TacticsMapTerms`)

`_tacticsmapterms_work` (0x1494d8) `+0x0` state; **4** = any finger-up
closes it (state 3 clears touch first, so earlier taps are lost). Decode
the switch in `TacticsMapTerms_Main` (0x1a194) to confirm 4. Behaviour:
confirm or back tap the centre only in state 4; swallow presses before
that (don't queue a lost tap). Verify: at a battle's start and from the
MENU and the deploy menu, one press closes the terms.

### B9. Docs and clean-up

- Update `dev-docs/song-summoner-re.md` "Battle controller plan" steps
  3–7 with what was built and verified, every decoded state constant,
  and anything this plan got wrong.
- Update the module doc at the top of `game_input.rs` (the battle
  bullets) and `battle.rs`.
- Remove debug-only logging that's no longer useful, keep the "presses
  waiting" style lines and the frame-stamped finger lines.
- Mark this file's status line with what's done.

---

## 5. Risks and how to catch them early

| Risk | Where | Early check |
|---|---|---|
| A slid synthetic finger breaks the hold (gesture 2 → 1) | B0.2, all finger-follow | First in-play test of B3: the log must show gesture 2 throughout |
| State constants guessed wrong | every handler | §1 rule 3; "presses waiting" logs show the real state |
| Lifting on a "harmless" tile isn't harmless in some phase | B3, B4, B6.2 | Log the tile and the phase after every lift |
| Quick-tap "back" in attack select lands on a unit sprite | B4 | `harmless_tap` keeps clear of units; log the tap point |
| Zoom during a held finger moves the target tile | B5 | Lift first, re-press after the zoom |
| The skill list scrolls past 5 rows | B2.3 | Decode first; clamp if unsure |
| Widgets over the map (item list, dialogs) swallowed by the battle handler | B0.1 | Widgets always win; log `menus open` |

## 6. Suggested in-play test script (for the user, after everything)

1. Start a battle: terms close with one press; tutorial pages advance.
2. Unit select: move, L/R, hold confirm on a unit → ring.
3. Ring: left/right turn it, back → unit select, confirm on Move.
4. Move select: D-pad only visits lit tiles; back → ring; confirm walks.
5. Ring → Attack: cycle with L/R, back → ring, confirm → attack info,
   back → attack select, confirm → attack.
6. Ring → Skill: hold previews, pick an area skill, attack.
7. Ring → Item: pick an item, target, use.
8. Ring → Status: flip, back.
9. Triggers zoom in unit, move and attack select.
10. Next battle with deploy: place a full party, dialog opens on No,
    sort, map view, start.
11. After the battle: Listening Point, Start skips.

## 7. Changed in implementation (2026-09-27)

- **B0.1:** `phase_owner(phase)` takes no sub-phase: each handler waits
  for its own "running" state (sub 0x65 and the like) instead, so presses
  made as a phase starts aren't lost. Scripts (17, 18) and the silent box
  (27) are `TapAnywhere`, not swallowed: script text advances on a tap, and
  a controller that can't go on is worse than a stray centre tap.
- **B0.4:** `harmless_point` prefers tiles on the map, since the game may
  clamp a held cursor that's off it onto an edge tile. `harmless_tap` keeps
  clear of any unit on the three tiles below it, not only target units.
- **B0.5:** `holdable` keeps its verified bound (finger ≤ 248);
  `holdable_in` uses the 4-point margin for the other rects.
- **B1:** the ring waits for its front item to be shown rather than all 7
  (a hidden item is skipped as a neighbour). Items can be greyed out (see
  the RE doc); a tap on one plays SE 1, so nothing special is done.
- **B3:** the decode found a quick tap off the reachable tiles also
  cancels move select; back still slides home and lets go, as planned.
- **B3/B4/B6.2:** if the cursor's tile can't be held (the map's bottom
  edge, and no pan brings it up), confirm taps it instead.
- **B6.1:** the party-full dialog gets "No" whenever the deploy is in
  state 13, rather than matching the dialog's handle at `+0x40` (an id,
  not the work address).
- **B6.3:** the sort panel is handled in state 7 even while the card list
  under it is detected.
- **B8:** the terms count as up only while their animation or window id
  (`+0x10`, `+0x14`) is set and their state is below 7, in phases 5, 6, 21
  and the deploy's state 10; presses before state 4 are dropped.
- Real touches drop the virtual finger (without a lift).
- **2026-09-28, user's decision:** move and attack select's cursor is no
  longer locked to the game's list: the D-pad moves it along the grid
  anywhere on the map (`battle::cursor_step`, free), and confirm lets go on
  whatever tile it's on (on one that doesn't act, nothing happens). It
  still starts on the nearest listed tile, L/R still cycle attack select's
  targets, and the list still decides where a harmless lift can go.
  Placing was unlocked the same way the same day, so `locked_step` (B0.3)
  is gone; only `locked_start` remains.
- **2026-09-28, bug fix:** after a unit's turn ended, unit select's cursor
  stayed on the last unit's tile. A confirm hold that opens the ring left
  `battle_touching` set (unit select stops running before the finger's
  release is seen), so on returning to phase 19 the game's move of its
  cursor to the next unit was taken for our own. Every phase change now
  clears it, and unit select starts on the selected unit
  (`battle::unit_select_start`).
- **2026-09-28, user's choice:** move select's cursor starts on the unit's
  own tile (the finger rests there) instead of the nearest reachable tile;
  confirm or back there cancels the move, as a finger would. `locked_start`
  no longer takes a skip list.
- **2026-09-28, the ring turned slowly:** each tap waited for the turn to
  stop, and the game eases it out (a quarter of the gap a frame, down to
  1°). Now a left/right tap goes as soon as the item last tapped is the
  front one (`+0x18`), and queued presses the same way make one tap up to
  3 items round (`battle::ring_steps`, by item order). The ring doesn't
  turn while a finger is down, so the tapped item stays put under it.
  Confirm and back still wait for the ring to stop.
