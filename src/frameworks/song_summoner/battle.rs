/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Battle (the `Tactics` scene): the map grid and where its tiles are on
//! screen, for controller input. Host-only: `game_input.rs` reads the
//! game's memory into these types (song-summoner-re.md §4, "Battle").
//!
//! The grid is flat: every tile is a 48×24 diamond, drawn over a
//! pre-rendered picture of the map, so cliffs and height are only in the
//! art. Map x runs down-right on screen and map y down-left.
//!
//! Tile (x, y)'s centre is at `origin + (24·(x − y), 12·(x + y))` in map
//! points (`TacticsMap_Get_Map2DrawPosition`), `origin` being the camera
//! (the Tactics work's `+0x14/+0x16`). The map is then drawn scaled by the
//! zoom (`_prim_scale`) about the screen centre.
//!
//! The game turns a touch into a tile two ways. A tap
//! (`TacticsMap_TouchFast_Screen2Map`) takes the tile under the finger. A
//! held finger (`TacticsMap_Touch_Screen2Map`, the cursor that follows it)
//! takes the tile 36 screen points above the finger, presumably so the
//! fingertip doesn't hide it.
//!
//! Past unit select, the controller mostly drives the game's own cursor
//! with a held virtual finger ([follow]), so the game's previews show:
//! in move select, attack select and deploy placing the cursor starts on
//! a tile in the game's list of tiles that act ([locked_start]) and moves
//! freely on the grid; the finger slides from tile to tile, and letting go
//! acts. Reaching a tile off screen, or bringing
//! the finger up, goes through a point where letting go does nothing
//! ([harmless_point]). [phase_owner] says which handler has the controller
//! in each phase.

use super::pad::Role;

/// Half a tile's width and height, in map points.
pub const TILE_HALF_W: f32 = 24.0;
pub const TILE_HALF_H: f32 = 12.0;
/// How far above a held finger the game puts its cursor, in screen points
/// (`Touch_Screen2Map` subtracts 36 / zoom in map points).
pub const HOLD_OFFSET: f32 = 36.0;
/// The game view, in its 480×320 landscape points, and its centre, which
/// the zoom scales about.
pub const SCREEN: (f32, f32) = (480.0, 320.0);
pub const SCREEN_CENTRE: (f32, f32) = (240.0, 160.0);

/// A tile: map x, map y.
pub type Tile = (i32, i32);
/// A point in the game view's points.
pub type Point = (f32, f32);

/// Where the map is on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Tile (0, 0)'s centre, in map points.
    pub origin: Point,
    /// The zoom: 1.0 is 100%.
    pub scale: f32,
}

impl Camera {
    /// A map point on screen.
    fn to_screen(&self, (x, y): Point) -> Point {
        let (cx, cy) = SCREEN_CENTRE;
        (cx + self.scale * (x - cx), cy + self.scale * (y - cy))
    }

    /// A screen point on the map.
    fn to_map(&self, (x, y): Point) -> Point {
        let (cx, cy) = SCREEN_CENTRE;
        (cx + (x - cx) / self.scale, cy + (y - cy) / self.scale)
    }

    /// The tile's centre on screen.
    pub fn tile_centre(&self, (x, y): Tile) -> Point {
        let (ox, oy) = self.origin;
        self.to_screen((
            ox + TILE_HALF_W * (x - y) as f32,
            oy + TILE_HALF_H * (x + y) as f32,
        ))
    }

    /// The tile's corners on screen: top, right, bottom, left.
    pub fn tile_corners(&self, tile: Tile) -> [Point; 4] {
        let (cx, cy) = self.tile_centre(tile);
        let (hw, hh) = (TILE_HALF_W * self.scale, TILE_HALF_H * self.scale);
        [(cx, cy - hh), (cx + hw, cy), (cx, cy + hh), (cx - hw, cy)]
    }

    /// The rectangle around the tile's diamond (x, y, w, h), for the
    /// controller's outline.
    pub fn tile_rect(&self, tile: Tile) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.tile_centre(tile);
        let (hw, hh) = (TILE_HALF_W * self.scale, TILE_HALF_H * self.scale);
        (cx - hw, cy - hh, 2.0 * hw, 2.0 * hh)
    }

    /// The tile a tap at `point` is on, whether or not it's on the map.
    pub fn tile_at(&self, point: Point) -> Tile {
        let (mx, my) = self.to_map(point);
        let (ox, oy) = self.origin;
        // x − y and x + y, in tiles; a diamond is where both round to the
        // same x and y.
        let u = (mx - ox) / TILE_HALF_W;
        let v = (my - oy) / TILE_HALF_H;
        (((u + v) / 2.0).round() as i32, ((v - u) / 2.0).round() as i32)
    }

    /// The tile the game's cursor goes to while a finger is held at
    /// `point`.
    pub fn hold_tile_at(&self, (x, y): Point) -> Tile {
        self.tile_at((x, y - HOLD_OFFSET))
    }

    /// Where to hold a finger so the game's cursor is on `tile`.
    pub fn hold_point(&self, tile: Tile) -> Point {
        let (x, y) = self.tile_centre(tile);
        (x, y + HOLD_OFFSET)
    }
}

/// Whether a point is in the game view.
pub fn on_screen((x, y): Point) -> bool {
    (0.0..SCREEN.0).contains(&x) && (0.0..SCREEN.1).contains(&y)
}

/// The map's grid (`_tacticsmap_work`), one entry per tile, row by row
/// (`y · w + x`).
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub w: i32,
    pub h: i32,
    /// The unit on each tile (`+0x30`), −1 for none.
    pub units: Vec<i32>,
    /// The highlight layer (`+0x34`): move and attack ranges, 0 for none.
    pub highlight: Vec<i32>,
}

impl Grid {
    pub fn contains(&self, (x, y): Tile) -> bool {
        (0..self.w).contains(&x) && (0..self.h).contains(&y)
    }

    fn index(&self, tile: Tile) -> Option<usize> {
        self.contains(tile).then(|| (tile.1 * self.w + tile.0) as usize)
    }

    /// The unit on a tile, if any.
    pub fn unit_at(&self, tile: Tile) -> Option<u32> {
        let unit = *self.units.get(self.index(tile)?)?;
        u32::try_from(unit).ok()
    }

    /// How many tiles the highlight layer marks.
    pub fn highlighted(&self) -> usize {
        self.highlight.iter().filter(|&&h| h != 0).count()
    }

    /// The tiles the highlight layer marks, row by row, with their values.
    pub fn highlighted_tiles(&self) -> Vec<(Tile, i32)> {
        self.tiles()
            .zip(self.highlight.iter())
            .filter(|&(_, &h)| h != 0)
            .map(|(tile, &h)| (tile, h))
            .collect()
    }

    /// Every tile, row by row.
    pub fn tiles(&self) -> impl Iterator<Item = Tile> + '_ {
        (0..self.h).flat_map(move |y| (0..self.w).map(move |x| (x, y)))
    }
}

/// The tiles only in `a` and only in `b`, each sorted, ignoring order and
/// repeats: to check the lit tiles against the game's target lists.
pub fn tile_list_diff(a: &[Tile], b: &[Tile]) -> (Vec<Tile>, Vec<Tile>) {
    use std::collections::BTreeSet;
    let a: BTreeSet<Tile> = a.iter().copied().collect();
    let b: BTreeSet<Tile> = b.iter().copied().collect();
    (
        a.difference(&b).copied().collect(),
        b.difference(&a).copied().collect(),
    )
}

/// A step along the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Right,
    Down,
    Left,
}

/// The tile one step from `tile`, staying on the map. The D-pad follows
/// the grid's axes, not the screen's (decided 2026-09-27, "option A"): up
/// is up-right on screen (y − 1), right is down-right (x + 1), down is
/// down-left (y + 1), left is up-left (x − 1).
pub fn grid_step(grid: &Grid, (x, y): Tile, dir: Dir) -> Tile {
    let next = match dir {
        Dir::Up => (x, y - 1),
        Dir::Right => (x + 1, y),
        Dir::Down => (x, y + 1),
        Dir::Left => (x - 1, y),
    };
    if grid.contains(next) {
        next
    } else {
        (x, y)
    }
}

/// What a controller button does in unit select (phase 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitSelectIntent {
    /// Move touchHLE's cursor a tile.
    Move(Dir),
    /// Tap the cursor's tile: select the unit there, or open the command
    /// ring of the selected one.
    Confirm,
    /// Tap the curved arrows: the previous or next unit.
    PrevUnit,
    NextUnit,
    /// Tap MENU.
    Menu,
    None,
}

pub fn unit_select_intent(role: Role) -> UnitSelectIntent {
    match role {
        Role::Up => UnitSelectIntent::Move(Dir::Up),
        Role::Down => UnitSelectIntent::Move(Dir::Down),
        Role::PrevSection => UnitSelectIntent::Move(Dir::Left),
        Role::NextSection => UnitSelectIntent::Move(Dir::Right),
        Role::PrevTab => UnitSelectIntent::PrevUnit,
        Role::NextTab => UnitSelectIntent::NextUnit,
        Role::Confirm => UnitSelectIntent::Confirm,
        Role::Skip => UnitSelectIntent::Menu,
        // The triggers zoom, before unit select sees them.
        Role::Back | Role::Info | Role::ZoomOut | Role::ZoomIn => UnitSelectIntent::None,
    }
}

/// Where a tap may go on the map (x, y, w, h). Unit select takes map taps
/// above y 256; below are the status panel, the curved arrows and MENU.
/// A margin keeps clear of the edges.
pub const TAP_AREA: (f32, f32, f32, f32) = (16.0, 16.0, 448.0, 224.0);
/// The middle of [TAP_AREA], where a pan brings a tile.
pub const TAP_CENTRE: Point = (240.0, 128.0);
/// The furthest a pan moves the finger from [TAP_CENTRE], so it stays in
/// the map area (and off the status panel).
pub const PAN_MAX: (f32, f32) = (200.0, 100.0);
/// Where to tap the curved arrows (`TacticsUnitSelect_CtrlRelease`'s rects
/// (56, 272, 44, 48) and (379, 256, 44, 64), their centres).
pub const PREV_UNIT_ARROW: Point = (78.0, 296.0);
pub const NEXT_UNIT_ARROW: Point = (401.0, 288.0);

// Unit select's bounds as first verified in play; the code now goes
// through `holdable_in` and `UNIT_SELECT_HOLD`, and the tests pin the two
// to each other.
#[cfg(test)]
pub fn tappable((x, y): Point) -> bool {
    let (ax, ay, aw, ah) = TAP_AREA;
    (ax..=ax + aw).contains(&x) && (ay..=ay + ah).contains(&y)
}

/// How long unit select's confirm holds the finger down, in frames.
/// `Tactics_CtrlTest` calls a still finger a hold after more than 6.
pub const SELECT_HOLD_FRAMES: u32 = 10;

/// Whether a tile whose centre is at `centre` can be picked with a held
/// finger: it goes [HOLD_OFFSET] below, and `Tactics_CtrlTest` only takes
/// a hold inside (0, 0, 480, 256), so that must stay above 256 (with a
/// margin), as well as the tile being in [TAP_AREA].
///
/// A hold, unlike a tap, picks by tile: `TacticsUnitSelect_CtrlRelease`
/// takes the unit on the game's cursor tile (`TacticsUnit_Get_Pos2Number`)
/// after a hold, but after a tap asks `TacticsMap_TouchFast_Screen2Unit`,
/// which goes by the units' sprites, so a unit drawn over the tile behind
/// it takes a tap meant for that tile. One hold also acts at once, on any
/// unit: while the finger is down, `TacticsUnitSelect_CtrlTouch` makes the
/// unit under it the current one (`Update_StatusPanelList`), so its release
/// opens the command ring (or another team's status) rather than only
/// selecting it, as a tap on an unselected unit does.
#[cfg(test)]
pub fn holdable(centre: Point) -> bool {
    // 8 points inside the rect's bottom, as verified in play.
    tappable(centre) && centre.1 + HOLD_OFFSET <= UNIT_SELECT_AREA.3 - 8.0
}

/// The finger's movement for a pan (a one-move flick from [TAP_CENTRE])
/// that brings `tile` toward the middle of the tap area, or `None` if
/// there's nothing to do: it's there already, or the last pan (which
/// started with the camera at `last_pan`) didn't move the camera, so it's
/// at the map's edge. `TacticsMap_Scroll` moves the camera by twice the
/// finger's movement on screen, taking the movement in whole points.
pub fn pan_toward(camera: &Camera, tile: Tile, last_pan: Option<Point>) -> Option<(f32, f32)> {
    if last_pan == Some(camera.origin) {
        return None;
    }
    let (x, y) = camera.tile_centre(tile);
    let dx = ((TAP_CENTRE.0 - x) / 2.0).trunc().clamp(-PAN_MAX.0, PAN_MAX.0);
    let dy = ((TAP_CENTRE.1 - y) / 2.0).trunc().clamp(-PAN_MAX.1, PAN_MAX.1);
    (dx != 0.0 || dy != 0.0).then_some((dx, dy))
}

/// Whether a confirm that has to tap (a tile too low to hold) does
/// anything on `tile`: only a unit can be picked, and a tap elsewhere
/// shows nothing. (A held confirm goes on any tile, as a finger would:
/// the game's cursor shows where it is while it's down.)
pub fn confirm_picks(grid: &Grid, tile: Tile) -> bool {
    grid.unit_at(tile).is_some()
}

/// A rectangle in the game view's points: x, y, width, height.
pub type Rect = (f32, f32, f32, f32);

/// `Tactics_CtrlTest`'s rects: unit select takes holds above the status
/// panel; move select, attack select and placing (the 5-argument
/// overload, 0x3b88) take them anywhere.
pub const UNIT_SELECT_AREA: Rect = (0.0, 0.0, 480.0, 256.0);
pub const WHOLE_SCREEN: Rect = (0.0, 0.0, SCREEN.0, SCREEN.1);
/// Unit select's area for [holdable_in]: [UNIT_SELECT_AREA] less 4 points
/// at the bottom, so the finger stays at or above y 248, as verified in
/// play (the same tiles as `holdable`).
pub const UNIT_SELECT_HOLD: Rect = (
    UNIT_SELECT_AREA.0,
    UNIT_SELECT_AREA.1,
    UNIT_SELECT_AREA.2,
    UNIT_SELECT_AREA.3 - 4.0,
);

/// Whether a tile whose centre is at `centre` can be picked with a finger
/// held [HOLD_OFFSET] below it, when `Tactics_CtrlTest` takes holds in
/// `rect`: the finger must be inside it (with a margin), and the tile
/// inside [TAP_AREA]'s sides and below its top, like a tap.
pub fn holdable_in((x, y): Point, (rx, ry, rw, rh): Rect) -> bool {
    const MARGIN: f32 = 4.0;
    let (ax, ay, aw, _) = TAP_AREA;
    let finger_y = y + HOLD_OFFSET;
    (ax..=ax + aw).contains(&x)
        && y >= ay
        && (rx + MARGIN..=rx + rw - MARGIN).contains(&x)
        && (ry + MARGIN..=ry + rh - MARGIN).contains(&finger_y)
}

/// Who takes the controller in a battle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    UnitSelect,
    Ring,
    MoveSelect,
    AttackSelect,
    AttackInfo,
    Sortie,
    /// Nothing the player drives (walking, the attack, the Colosseum
    /// roulette, phase banners): presses are dropped, since a tap in the
    /// middle would only land on whatever comes next.
    Swallow,
    /// A screen that takes a tap anywhere (victory terms, battle intro,
    /// scripts, between fights, results), or the game's own menus: the
    /// usual menu handling, whose confirm taps the middle with nothing
    /// open.
    TapAnywhere,
}

/// Who takes the controller in phase `phase` (`Tactics_Main`'s switch;
/// song-summoner-re.md, Battle). Whether the phase is ready (its sub-phase,
/// its own state) is each handler's business: until it is, presses wait.
pub fn phase_owner(phase: u32) -> Owner {
    match phase {
        8 => Owner::Sortie,
        19 => Owner::UnitSelect,
        20 => Owner::Ring,
        22 => Owner::MoveSelect,
        24 => Owner::AttackSelect,
        25 => Owner::AttackInfo,
        // Scripts (17, 18) are here rather than swallowed: their text
        // boxes advance on a tap, as in cutscenes. 27, the silent box, is
        // unread; a tap there is safer than a controller that can't go on.
        5 | 6 | 10..=13 | 17 | 18 | 21 | 27 | 29..=39 => Owner::TapAnywhere,
        _ => Owner::Swallow,
    }
}

/// The first of `items` with the smallest key (`min_by_key` takes the
/// last on a tie; list order breaks ties here).
fn first_min<T: Copy, K: PartialOrd>(items: impl Iterator<Item = T>, key: impl Fn(T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for item in items {
        let k = key(item);
        if best.as_ref().map_or(true, |(_, b)| k < *b) {
            best = Some((item, k));
        }
    }
    best.map(|(item, _)| item)
}

/// Who takes the controller now: [phase_owner], unless a script is running
/// (`_tactics_script_flag`: `Tactics_Main` runs the script loop instead of
/// the phase, and a map script can show a message in any phase, like a
/// buried treasure in unit end) or a message waits for a tap (its "tap to
/// continue" mark shows). Those take a tap anywhere.
pub fn owner_now(phase: u32, script_running: bool, message_waiting: bool) -> Owner {
    if script_running || message_waiting {
        Owner::TapAnywhere
    } else {
        phase_owner(phase)
    }
}

/// Where unit select's cursor goes when the phase starts (a new turn,
/// or back from the ring): the selected unit's tile, else the game's own
/// cursor, else the middle of the map.
///
/// `turn_start`: unit select comes straight after the turn script (phase
/// 18), a team's turn starting. The game has put its cursor on that team's
/// first unit then, while the selected unit is still whichever acted last
/// (the enemy, after its turn), so the cursor comes first.
pub fn unit_select_start(
    grid: &Grid,
    selected: Option<Tile>,
    game_cursor: Tile,
    turn_start: bool,
) -> Tile {
    let selected = selected.filter(|&t| grid.contains(t));
    let cursor = Some(game_cursor).filter(|&t| grid.contains(t));
    let first = if turn_start { cursor.or(selected) } else { selected.or(cursor) };
    first.unwrap_or((grid.w / 2, grid.h / 2))
}

/// Where the held-finger screens' cursor starts: the listed tile nearest
/// `near` by grid steps (`near` itself if it's listed, as the unit's own
/// tile is in move select), ties by list order.
pub fn locked_start(list: &[Tile], near: Tile) -> Option<Tile> {
    first_min(list.iter().copied(), |t| (t.0 - near.0).abs() + (t.1 - near.1).abs())
}

/// Where [harmless_point] and [harmless_tap] look: the screen, less a
/// margin so the finger stays on it.
pub const HARMLESS_AREA: Rect = (8.0, 8.0, 464.0, 304.0);
/// The lattice they search, in points.
const LATTICE: f32 = 8.0;

/// The points of a lattice over `area`, each with its distance (squared)
/// from `centre`.
fn lattice((ax, ay, aw, ah): Rect, centre: Point) -> impl Iterator<Item = (Point, f32)> {
    let columns = (aw / LATTICE) as i32;
    let rows = (ah / LATTICE) as i32;
    (0..=rows).flat_map(move |j| {
        (0..=columns).map(move |i| {
            let (x, y) = (ax + i as f32 * LATTICE, ay + j as f32 * LATTICE);
            let d = (x - centre.0).powi(2) + (y - centre.1).powi(2);
            ((x, y), d)
        })
    })
}

/// Scores off the map far behind every tile on it: the game may clamp a
/// point off the map to its edge, so those are a last resort.
const OFF_MAP: f32 = 1.0e7;

/// Where to let go of a held finger so nothing happens: a point in `area`
/// whose held tile isn't `accepted` (the tiles where letting go acts),
/// preferably on the map, nearest the screen centre. `None` if there's no
/// such point.
pub fn harmless_point(camera: &Camera, grid: &Grid, accepted: &[Tile], area: Rect) -> Option<Point> {
    let candidates = lattice(area, SCREEN_CENTRE).filter_map(|(point, d)| {
        let tile = camera.hold_tile_at(point);
        let penalty = if grid.contains(tile) { 0.0 } else { OFF_MAP };
        (!accepted.contains(&tile)).then_some((point, d + penalty))
    });
    first_min(candidates, |(_, score)| score).map(|(point, _)| point)
}

/// Where a quick tap picks nothing, for attack select's back and to cancel
/// placing: in [TAP_AREA], on a tile that isn't in `targets` and has no
/// unit, with none on the three tiles below it on screen either (taps on
/// units go by sprite, and a sprite reaches up about a tile). Preferably
/// on the map, nearest [TAP_CENTRE].
pub fn harmless_tap(camera: &Camera, grid: &Grid, targets: &[Tile]) -> Option<Point> {
    let candidates = lattice(TAP_AREA, TAP_CENTRE).filter_map(|(point, d)| {
        let (x, y) = camera.tile_at(point);
        let clear = [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
            .iter()
            .all(|&t| grid.unit_at(t).is_none());
        let penalty = if grid.contains((x, y)) { 0.0 } else { OFF_MAP };
        (clear && !targets.contains(&(x, y))).then_some((point, d + penalty))
    });
    first_min(candidates, |(_, score)| score).map(|(point, _)| point)
}

/// What a held finger following a locked cursor needs to know.
#[derive(Debug, Clone)]
pub struct FollowView<'a> {
    pub camera: &'a Camera,
    pub grid: &'a Grid,
    /// Where letting go acts (the whole list, including a skipped tile).
    pub accepted: &'a [Tile],
    pub cursor: Option<Tile>,
    /// Where the finger is down (or will be, once queued steps run).
    pub finger: Option<Point>,
    /// The finger should be down on the cursor's tile (false while it has
    /// to come up: zoom, attack select's back).
    pub want_down: bool,
    /// Let go on the cursor's tile once the finger is on it (confirm, and
    /// move select's back, whose cursor is then the unit's own tile).
    pub act: bool,
    /// The camera when the last pan started (see [pan_toward]).
    pub last_pan: Option<Point>,
    /// Where `Tactics_CtrlTest` takes a hold ([holdable_in]): [WHOLE_SCREEN],
    /// or [UNIT_SELECT_HOLD].
    pub hold_area: Rect,
    /// Where the finger may let go harmlessly, and a tap too low to hold
    /// may go: [HARMLESS_AREA], or in unit select [TAP_AREA] (above the
    /// status panel, MENU and the arrows).
    pub harmless_area: Rect,
}

/// The next thing a held finger following a locked cursor does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FollowAction {
    /// Nothing now.
    Wait,
    Press(Point),
    Slide(Point),
    /// Let go where the finger is.
    Lift,
    /// Slide to a point where letting go does nothing, and let go.
    LiftAt(Point),
    /// With the finger up: pan the map by this (a flick, see
    /// [pan_toward]).
    Pan(f32, f32),
    /// The cursor's tile can't be held (the map's edge): a quick tap on it
    /// instead, to act.
    Tap(Point),
    /// The cursor's tile can't be reached.
    Stuck,
}

/// The finger's next move for a cursor held in `view.hold_area`. It only
/// ever lets go on a tile that acts (`view.accepted`) to act: to reach a
/// tile off screen (or come up) it slides to a harmless point first.
pub fn follow(view: &FollowView) -> FollowAction {
    let camera = view.camera;
    let target = view.cursor.filter(|_| view.want_down);
    let holdable = |tile: Tile| holdable_in(camera.tile_centre(tile), view.hold_area);
    let come_up = |finger: Point| {
        if view.accepted.contains(&camera.hold_tile_at(finger)) {
            harmless_point(camera, view.grid, view.accepted, view.harmless_area)
                .map_or(FollowAction::Stuck, FollowAction::LiftAt)
        } else {
            FollowAction::Lift
        }
    };
    match (view.finger, target) {
        (Some(finger), Some(tile)) => {
            if camera.hold_tile_at(finger) == tile {
                if view.act {
                    FollowAction::Lift
                } else {
                    FollowAction::Wait
                }
            } else if holdable(tile) {
                FollowAction::Slide(camera.hold_point(tile))
            } else {
                come_up(finger)
            }
        }
        (Some(finger), None) => come_up(finger),
        (None, None) => FollowAction::Wait,
        (None, Some(tile)) => {
            if holdable(tile) {
                return FollowAction::Press(camera.hold_point(tile));
            }
            if let Some((dx, dy)) = pan_toward(camera, tile, view.last_pan) {
                return FollowAction::Pan(dx, dy);
            }
            let centre = camera.tile_centre(tile);
            let (ax, ay, aw, ah) = view.harmless_area;
            let tappable = (ax..=ax + aw).contains(&centre.0) && (ay..=ay + ah).contains(&centre.1);
            if view.act && tappable {
                FollowAction::Tap(centre)
            } else {
                FollowAction::Stuck
            }
        }
    }
}

/// A side of the command ring's front item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The ring item next to the front one on `side`: of the others, the one
/// whose centre is on that side of the front's (by x) and nearest it.
pub fn ring_neighbour(centres: &[Point], front: usize, side: Side) -> Option<usize> {
    let &(fx, fy) = centres.get(front)?;
    let beside = (0..centres.len()).filter(|&i| {
        let x = centres[i].0;
        i != front
            && match side {
                Side::Left => x < fx,
                Side::Right => x > fx,
            }
    });
    first_min(beside, |i| {
        let (x, y) = centres[i];
        (x - fx).powi(2) + (y - fy).powi(2)
    })
}

/// The ring item `n` steps round from the front on `side`: the neighbour
/// on that side ([ring_neighbour]) says which way round the items' order
/// goes, and the rest follow that order (by x alone, a step past the top
/// or bottom would turn back). Just the neighbour if it isn't next in the
/// order (a hidden item between).
pub fn ring_steps(centres: &[Point], front: usize, side: Side, n: usize) -> Option<usize> {
    let first = ring_neighbour(centres, front, side)?;
    let len = centres.len();
    let step = if first == (front + 1) % len {
        1
    } else if (first + 1) % len == front {
        len - 1
    } else {
        return Some(first);
    };
    Some((front + n.max(1) * step) % len)
}

/// Where back taps on the command ring: outside it (x < 100), which
/// cancels (`TacticsUnitMenu_CtrlTop`).
pub const RING_CANCEL: Point = (40.0, 160.0);

/// Where a command taps on the command ring (`centres`: its 7 items, the
/// front one `front`): left/right the neighbour on that side (the game
/// turns the ring to it), confirm the front item, back outside the ring.
pub fn ring_command(centres: &[Point], front: usize, role: Role) -> Option<Point> {
    let item = |i: Option<usize>| i.and_then(|i| centres.get(i).copied());
    match role {
        Role::PrevSection => item(ring_neighbour(centres, front, Side::Left)),
        Role::NextSection => item(ring_neighbour(centres, front, Side::Right)),
        Role::Confirm => item(Some(front)),
        Role::Back => Some(RING_CANCEL),
        _ => None,
    }
}

/// Where a command taps on a unit's full status (the ring's Status, an
/// enemy's status in unit select, the deploy map's): a finger-up at
/// x < 160 flips the page, one further right closes it. The finger must go
/// down and up on the same side.
pub fn status_command(role: Role) -> Option<Point> {
    match role {
        Role::Info | Role::Confirm => Some((80.0, 160.0)),
        Role::Back => Some((320.0, 160.0)),
        _ => None,
    }
}

/// The skill panel shows this many rows (320 points of 60-point rows from
/// y 8), and doesn't scroll.
pub const SKILL_ROWS_SHOWN: usize = 5;
/// A point on the map side of the skill panel: a still tap here closes it.
pub const SKILL_CLOSE: Point = (160.0, 160.0);

/// Where to hold a finger on skill row `i` (`TacticsUnitMenu_CtrlSkill`:
/// row (y − 8) / 60, list at x ≥ 320). It's not the map: no offset.
pub fn skill_row_point(i: usize) -> Point {
    (400.0, 38.0 + 60.0 * i as f32)
}

/// The row a finger at height `y` is on.
pub fn skill_row_at(y: f32) -> Option<usize> {
    (y >= 8.0).then(|| ((y - 8.0) / 60.0) as usize)
}

/// How many of `count` skills have a row on screen.
pub fn skill_rows(count: usize) -> usize {
    count.min(SKILL_ROWS_SHOWN)
}

/// Which of attack select's lists a finger picks from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackList {
    /// Units (`_tac_attack_select + 0x8`, count `+0x38c`).
    Targets,
    /// Tiles, for area skills (`+0x394`, count `+0x718`).
    Area,
}

/// The list attack select's mode (`+0x72c`) uses:
/// `TacticsAttackSelect_CtrlRelease`'s switch (0x2f7f8) looks 0 and 1 up
/// in the target list and 2 and 3 in the area list.
pub fn attack_list(mode: u32) -> Option<AttackList> {
    match mode {
        0 | 1 => Some(AttackList::Targets),
        2 | 3 => Some(AttackList::Area),
        _ => None,
    }
}

/// The index after (or before) `i` among `len`, wrapping round.
pub fn cycle(len: usize, i: usize, forward: bool) -> usize {
    if len == 0 {
        return i;
    }
    if forward {
        (i + 1) % len
    } else {
        (i + len - 1) % len
    }
}

/// Where attack info (phase 25) goes to the next target: x ≥ 360,
/// 180 ≤ y ≤ 228 (`TacticsAttackInfo_Main`).
pub const NEXT_TARGET: Point = (420.0, 204.0);

/// The taps a command makes in attack info, with `count` targets: confirm
/// taps the band above the panels (the attack), back the top (back to
/// attack select), R the next-target area, and L the same all the way
/// round less one, since there's no "previous".
pub fn attack_info_taps(role: Role, count: u32) -> Vec<Point> {
    match role {
        Role::Confirm => vec![(240.0, 160.0)],
        Role::Back => vec![(240.0, 20.0)],
        Role::NextTab if count > 1 => vec![NEXT_TARGET],
        Role::PrevTab if count > 1 => vec![NEXT_TARGET; count as usize - 1],
        _ => Vec::new(),
    }
}

/// The zoom's range and step, in percent (the pinch keeps it in 100–200).
pub const ZOOM_MIN: i32 = 100;
pub const ZOOM_MAX: i32 = 200;
pub const ZOOM_STEP: i32 = 20;

/// The zoom after one trigger press (`zoom_in` for R2), or `None` at the
/// limit.
pub fn zoom_step(current: i32, zoom_in: bool) -> Option<i32> {
    let step = if zoom_in { ZOOM_STEP } else { -ZOOM_STEP };
    let new = (current + step).clamp(ZOOM_MIN, ZOOM_MAX);
    (new != current).then_some(new)
}

/// The deploy sort panel's six rows, the same as Edit Troopers': x 80–400,
/// y 37 + 46·i to 83 + 46·i.
pub fn sort_panel_rows() -> Vec<Rect> {
    (0..6).map(|i| (80.0, 37.0 + 46.0 * i as f32, 320.0, 46.0)).collect()
}

/// Whether a `SysAnim` shows: a real id (the table has 512 slots), its
/// slot in use (`+0x0` ≠ 0) and shown (`+0xc` == 1).
pub fn anim_visible(id: i32, state: u32, shown: u32) -> bool {
    (0..512).contains(&id) && state != 0 && shown == 1
}

/// A line to draw, from one point to another.
pub type Line = (Point, Point);

/// The outline of every tile whose centre is on screen, for the debug
/// overlay that shows where touchHLE thinks the tiles are.
pub fn grid_lines(camera: &Camera, grid: &Grid) -> Vec<Line> {
    let mut lines = Vec::new();
    for tile in grid.tiles() {
        if !on_screen(camera.tile_centre(tile)) {
            continue;
        }
        let corners = camera.tile_corners(tile);
        for i in 0..4 {
            lines.push((corners[i], corners[(i + 1) % 4]));
        }
    }
    lines
}

/// A name for a battle phase (`Tactics_Main`'s switch), for logs.
pub fn phase_name(phase: u32) -> &'static str {
    match phase {
        0..=4 => "loading",
        5 | 6 => "victory terms",
        7 => "enemy deploy",
        8 => "sortie",
        9 => "player deploy",
        10..=12 => "battle start",
        13 => "listening point",
        14 => "colosseum target",
        15 | 16 => "phase start",
        17 => "unit check",
        18 => "turn script",
        19 => "unit select",
        20 => "command ring",
        21 => "map menu",
        22 => "move select",
        23 => "walking",
        24 => "attack select",
        25 => "attack info",
        26 => "attack",
        27 => "silent box",
        28 => "unit end",
        29..=35 | 39 => "contest",
        36..=38 => "result / game over",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    const CAMERA: Camera = Camera {
        origin: (200.0, 40.0),
        scale: 1.0,
    };

    #[test]
    fn tile_zero_is_at_the_origin() {
        assert!(close(CAMERA.tile_centre((0, 0)), (200.0, 40.0)));
    }

    #[test]
    fn map_x_runs_down_right_and_map_y_down_left() {
        // Map2DrawPosition: 24 across and 12 down per step.
        assert!(close(CAMERA.tile_centre((1, 0)), (224.0, 52.0)));
        assert!(close(CAMERA.tile_centre((0, 1)), (176.0, 52.0)));
        assert!(close(CAMERA.tile_centre((3, 2)), (224.0, 100.0)));
    }

    #[test]
    fn zoom_scales_about_the_screen_centre() {
        // A tile whose map point is 10 right of the centre ends up 20
        // right of it at 200%; the centre itself doesn't move.
        let camera = Camera {
            origin: (250.0, 160.0),
            scale: 2.0,
        };
        assert!(close(camera.tile_centre((0, 0)), (260.0, 160.0)));
        let camera = Camera {
            origin: (240.0, 160.0),
            scale: 1.3,
        };
        assert!(close(camera.tile_centre((0, 0)), (240.0, 160.0)));
    }

    #[test]
    fn corners_are_half_a_tile_out_and_scale_with_zoom() {
        let camera = Camera {
            origin: (240.0, 160.0),
            scale: 2.0,
        };
        let [top, right, bottom, left] = camera.tile_corners((0, 0));
        assert!(close(top, (240.0, 136.0)));
        assert!(close(right, (288.0, 160.0)));
        assert!(close(bottom, (240.0, 184.0)));
        assert!(close(left, (192.0, 160.0)));
    }

    #[test]
    fn a_tap_finds_the_tile_it_is_on() {
        for scale in [1.0, 1.3, 2.0] {
            let camera = Camera {
                origin: (180.0, 20.0),
                scale,
            };
            for tile in [(0, 0), (1, 0), (0, 1), (5, 3), (-2, 4), (7, 7)] {
                let (x, y) = camera.tile_centre(tile);
                assert_eq!(camera.tile_at((x, y)), tile, "centre, {scale}");
                // Anywhere well inside the diamond.
                let (dx, dy) = (TILE_HALF_W * scale * 0.4, TILE_HALF_H * scale * 0.4);
                for p in [(x + dx, y), (x - dx, y), (x, y + dy), (x, y - dy)] {
                    assert_eq!(camera.tile_at(p), tile, "{p:?}, {scale}");
                }
            }
        }
    }

    #[test]
    fn past_a_corner_is_the_next_tile() {
        let (x, y) = CAMERA.tile_centre((2, 2));
        // Past the right tip: x + 1, y − 1 (one step right on screen).
        assert_eq!(CAMERA.tile_at((x + 25.0, y)), (3, 1));
        // Past the bottom tip: x + 1, y + 1.
        assert_eq!(CAMERA.tile_at((x, y + 13.0)), (3, 3));
        // Just past the lower-right edge: x + 1.
        assert_eq!(CAMERA.tile_at((x + 13.0, y + 7.0)), (3, 2));
    }

    #[test]
    fn a_held_finger_puts_the_cursor_36_points_above_it() {
        for scale in [1.0, 1.5, 2.0] {
            let camera = Camera {
                origin: (180.0, 20.0),
                scale,
            };
            let tile = (4, 2);
            let (x, y) = camera.hold_point(tile);
            assert!(close((x, y), {
                let (cx, cy) = camera.tile_centre(tile);
                (cx, cy + 36.0)
            }));
            assert_eq!(camera.hold_tile_at((x, y)), tile);
            // A tap there would be another tile.
            assert_ne!(camera.tile_at((x, y)), tile);
        }
    }

    fn grid() -> Grid {
        // 3×2, a unit (7) on (2, 1).
        Grid {
            w: 3,
            h: 2,
            units: vec![-1, -1, -1, -1, -1, 7],
            highlight: vec![0; 6],
        }
    }

    #[test]
    fn grid_tiles_and_units() {
        let grid = grid();
        assert!(grid.contains((0, 0)) && grid.contains((2, 1)));
        assert!(!grid.contains((3, 0)) && !grid.contains((0, 2)) && !grid.contains((-1, 0)));
        assert_eq!(grid.unit_at((2, 1)), Some(7));
        assert_eq!(grid.unit_at((1, 1)), None);
        assert_eq!(grid.unit_at((5, 5)), None);
        assert_eq!(grid.tiles().count(), 6);
        assert_eq!(grid.tiles().last(), Some((2, 1)));
    }

    #[test]
    fn highlighted_counts_marked_tiles() {
        let mut grid = grid();
        assert_eq!(grid.highlighted(), 0);
        grid.highlight = vec![0, 1, 0, 3, 0, -1];
        assert_eq!(grid.highlighted(), 3);
    }

    #[test]
    fn highlighted_tiles_lists_them_with_their_values() {
        let mut grid = grid();
        grid.highlight = vec![0, 1, 0, 3, 0, -1];
        // Row by row: (1, 0), then (0, 1) and (2, 1).
        assert_eq!(
            grid.highlighted_tiles(),
            vec![((1, 0), 1), ((0, 1), 3), ((2, 1), -1)]
        );
    }

    #[test]
    fn tile_list_differences_ignore_order_and_repeats() {
        let a = [(1, 1), (2, 1), (3, 1), (2, 1)];
        let b = [(3, 1), (1, 1), (4, 4)];
        // In a only, in b only, each sorted.
        assert_eq!(tile_list_diff(&a, &b), (vec![(2, 1)], vec![(4, 4)]));
        assert_eq!(tile_list_diff(&b, &b), (vec![], vec![]));
    }

    #[test]
    fn the_overlay_outlines_tiles_on_screen() {
        let grid = grid();
        let lines = grid_lines(&CAMERA, &grid);
        // All six tiles are on screen: four sides each.
        assert_eq!(lines.len(), 24);
        // Tile (0, 0)'s top-right side.
        assert!(lines
            .iter()
            .any(|&(a, b)| close(a, (200.0, 28.0)) && close(b, (224.0, 40.0))));
        // With the map scrolled far left, no centre is on screen.
        let away = Camera {
            origin: (-500.0, 40.0),
            scale: 1.0,
        };
        assert!(grid_lines(&away, &grid).is_empty());
    }

    #[test]
    fn the_d_pad_steps_along_the_grid_axes() {
        // Option A: up is up-right on screen (y − 1), right is down-right
        // (x + 1), down is down-left (y + 1), left is up-left (x − 1).
        let grid = Grid {
            w: 8,
            h: 12,
            units: vec![-1; 96],
            highlight: vec![0; 96],
        };
        assert_eq!(grid_step(&grid, (3, 5), Dir::Up), (3, 4));
        assert_eq!(grid_step(&grid, (3, 5), Dir::Right), (4, 5));
        assert_eq!(grid_step(&grid, (3, 5), Dir::Down), (3, 6));
        assert_eq!(grid_step(&grid, (3, 5), Dir::Left), (2, 5));
        // On screen, up really is up-right and right down-right.
        let (x0, y0) = CAMERA.tile_centre((3, 5));
        let (x1, y1) = CAMERA.tile_centre(grid_step(&grid, (3, 5), Dir::Up));
        assert!(x1 > x0 && y1 < y0);
        let (x1, y1) = CAMERA.tile_centre(grid_step(&grid, (3, 5), Dir::Right));
        assert!(x1 > x0 && y1 > y0);
        // It stays on the map.
        assert_eq!(grid_step(&grid, (0, 0), Dir::Up), (0, 0));
        assert_eq!(grid_step(&grid, (0, 0), Dir::Left), (0, 0));
        assert_eq!(grid_step(&grid, (7, 11), Dir::Right), (7, 11));
        assert_eq!(grid_step(&grid, (7, 11), Dir::Down), (7, 11));
    }

    #[test]
    fn unit_select_buttons() {
        use UnitSelectIntent as I;
        assert_eq!(unit_select_intent(Role::Up), I::Move(Dir::Up));
        assert_eq!(unit_select_intent(Role::Down), I::Move(Dir::Down));
        assert_eq!(unit_select_intent(Role::PrevSection), I::Move(Dir::Left));
        assert_eq!(unit_select_intent(Role::NextSection), I::Move(Dir::Right));
        assert_eq!(unit_select_intent(Role::PrevTab), I::PrevUnit);
        assert_eq!(unit_select_intent(Role::NextTab), I::NextUnit);
        assert_eq!(unit_select_intent(Role::Confirm), I::Confirm);
        assert_eq!(unit_select_intent(Role::Skip), I::Menu);
        assert_eq!(unit_select_intent(Role::Back), I::None);
        assert_eq!(unit_select_intent(Role::Info), I::None);
    }

    #[test]
    fn taps_stay_above_the_status_panel() {
        // The map takes taps above y 256 (the panel, arrows and MENU are
        // below); keep a margin all round.
        assert!(tappable((240.0, 128.0)));
        assert!(tappable((20.0, 20.0)) && tappable((460.0, 236.0)));
        assert!(!tappable((240.0, 250.0)));
        assert!(!tappable((240.0, 300.0)));
        assert!(!tappable((5.0, 100.0)) && !tappable((475.0, 100.0)));
        assert!(!tappable((240.0, 5.0)));
    }

    #[test]
    fn a_pan_moves_the_finger_half_way_to_the_middle() {
        // The camera moves twice the finger's movement on screen
        // (TacticsMap_Scroll), so half the distance brings the tile to the
        // middle of the tappable area.
        let camera = Camera {
            origin: (240.0, 128.0),
            scale: 1.0,
        };
        // Tile (0, 0) is at (240, 128) already: nothing to do.
        assert_eq!(pan_toward(&camera, (0, 0), None), None);
        // Tile (4, 0) is 96 right and 48 down.
        assert_eq!(pan_toward(&camera, (4, 0), None), Some((-48.0, -24.0)));
        // Far away: the finger stays on screen.
        let (dx, dy) = pan_toward(&camera, (40, 0), None).unwrap();
        assert_eq!((dx, dy), (-PAN_MAX.0, -PAN_MAX.1));
        // Whole points, as the game takes them.
        assert_eq!(pan_toward(&camera, (1, 0), None), Some((-12.0, -6.0)));
        let odd = Camera {
            origin: (241.0, 129.0),
            scale: 1.0,
        };
        assert_eq!(pan_toward(&odd, (0, 0), None), None);
    }

    #[test]
    fn a_pan_that_went_nowhere_stops() {
        // The camera hit the map's edge: panning again is pointless.
        let camera = Camera {
            origin: (240.0, 128.0),
            scale: 1.0,
        };
        assert_eq!(pan_toward(&camera, (4, 0), Some((240.0, 128.0))), None);
        assert!(pan_toward(&camera, (4, 0), Some((250.0, 128.0))).is_some());
    }

    #[test]
    fn the_cursor_outline_is_the_tile() {
        let camera = Camera {
            origin: (240.0, 160.0),
            scale: 2.0,
        };
        assert_eq!(camera.tile_rect((0, 0)), (192.0, 136.0, 96.0, 48.0));
    }

    #[test]
    fn a_held_finger_must_stay_on_the_map_part_of_the_screen() {
        // Tactics_CtrlTest only calls a finger held on (0, 0, 480, 256) a
        // hold, and the finger goes 36 below the tile's centre.
        for centre in [(240.0, 128.0), (16.0, 16.0), (464.0, 212.0)] {
            assert!(holdable(centre), "{centre:?}");
            let (_, y) = (centre.0, centre.1 + HOLD_OFFSET);
            assert!(y < 256.0, "{centre:?}");
        }
        // Tappable, but the finger would land on the status panel.
        assert!(tappable((240.0, 225.0)));
        assert!(!holdable((240.0, 225.0)));
        // Off the sides and top, as for a tap.
        for centre in [(10.0, 128.0), (470.0, 128.0), (240.0, 10.0)] {
            assert!(!holdable(centre), "{centre:?}");
        }
    }

    #[test]
    fn a_confirm_tap_only_goes_on_a_tile_with_a_unit() {
        // A tap on a tile with no unit does nothing to see; skip it.
        let grid = grid();
        assert!(confirm_picks(&grid, (2, 1)));
        assert!(!confirm_picks(&grid, (1, 1)));
        assert!(!confirm_picks(&grid, (9, 9)));
    }

    #[test]
    fn a_pan_brings_a_tile_where_it_can_be_held() {
        // Pans aim at TAP_CENTRE, which must be holdable.
        assert!(holdable(TAP_CENTRE));
    }

    // B0.1: who takes the controller in each phase (song-summoner-re.md,
    // Battle's phase table).

    #[test]
    fn the_player_phases_have_their_own_handlers() {
        assert_eq!(phase_owner(8), Owner::Sortie);
        assert_eq!(phase_owner(19), Owner::UnitSelect);
        assert_eq!(phase_owner(20), Owner::Ring);
        assert_eq!(phase_owner(22), Owner::MoveSelect);
        assert_eq!(phase_owner(24), Owner::AttackSelect);
        assert_eq!(phase_owner(25), Owner::AttackInfo);
    }

    #[test]
    fn phases_nobody_drives_swallow_presses() {
        // Walking, the attack itself, unit end, the Colosseum roulette,
        // enemy deploy and the phase start banners read no touch: a tap in
        // the middle there would only land on the next phase.
        for phase in [7, 9, 14, 15, 16, 23, 26, 28] {
            assert_eq!(phase_owner(phase), Owner::Swallow, "{phase}");
        }
    }

    #[test]
    fn tap_anywhere_screens_and_menus_keep_the_centre_tap() {
        // Victory terms, battle start, the Listening Point bonus, MENU,
        // scripts (their text advances on a tap), the silent box, contests
        // and results (between fights, game over).
        for phase in [5, 6, 10, 11, 12, 13, 17, 18, 21, 27, 29, 35, 36, 38, 39] {
            assert_eq!(phase_owner(phase), Owner::TapAnywhere, "{phase}");
        }
    }

    #[test]
    fn a_script_or_a_waiting_message_takes_a_tap_in_any_phase() {
        // Seen 2026-09-28: a map script's "You found a buried treasure
        // chest!" in unit end (28), which otherwise drops presses.
        // Tactics_Main runs the script loop instead of the phase while
        // _tactics_script_flag is set.
        assert_eq!(owner_now(28, true, false), Owner::TapAnywhere);
        assert_eq!(owner_now(23, false, true), Owner::TapAnywhere);
        assert_eq!(owner_now(19, true, false), Owner::TapAnywhere);
        // Otherwise the phase's own owner.
        assert_eq!(owner_now(28, false, false), Owner::Swallow);
        assert_eq!(owner_now(19, false, false), Owner::UnitSelect);
    }

    // B0.3: where the held-finger screens' cursor starts (it then moves
    // freely on the grid, like unit select's: the user's choice,
    // 2026-09-28).

    #[test]
    fn a_locked_cursor_starts_on_the_nearest_listed_tile() {
        let list = [(0, 0), (5, 5), (3, 4), (4, 3)];
        // (3, 4) and (4, 3) are both 1 from (3, 3): the first listed.
        assert_eq!(locked_start(&list, (3, 3)), Some((3, 4)));
        assert_eq!(locked_start(&[], (3, 3)), None);
    }

    #[test]
    fn move_select_starts_on_the_units_own_tile() {
        // The reachable tiles include the unit's own, and the cursor rests
        // there when Move is chosen (the user's choice, 2026-09-28): letting
        // go there only cancels the move, and nothing lets go unasked.
        let own = (4, 4);
        let reachable = [(4, 3), (3, 4), own, (5, 4)];
        assert_eq!(locked_start(&reachable, own), Some(own));
    }

    #[test]
    fn unit_select_starts_on_the_selected_unit() {
        // When the next unit's turn comes, the cursor goes to it, not the
        // tile the last one was on.
        let grid = open_grid(8, 8);
        assert_eq!(unit_select_start(&grid, Some((5, 6)), (1, 1), false), (5, 6));
        // No selected unit: the game's cursor; hidden (−1) too: the middle.
        assert_eq!(unit_select_start(&grid, None, (1, 1), false), (1, 1));
        assert_eq!(unit_select_start(&grid, Some((20, 20)), (-1, -1), false), (4, 4));
    }

    #[test]
    fn a_new_turn_starts_on_the_games_cursor() {
        // After the enemy's turn the selected unit is still the enemy that
        // acted last, while the game's cursor is on the player's first
        // unit (2026-09-28 log: selected (2, 6), cursor (2, 8)).
        let grid = open_grid(10, 10);
        assert_eq!(unit_select_start(&grid, Some((2, 6)), (2, 8), true), (2, 8));
        // A hidden cursor still falls back to the selected unit.
        assert_eq!(unit_select_start(&grid, Some((2, 6)), (-1, -1), true), (2, 6));
    }

    #[test]
    fn follow_goes_to_and_lets_go_on_a_tile_off_the_list() {
        // With a free cursor, confirm on a tile that isn't listed still
        // lets go there (the game then does nothing, as for a finger).
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let accepted = [(3, 3)];
        let off = (4, 4);
        let finger = camera.hold_point((3, 3));
        let v = view(&camera, &grid, &accepted, Some(off), Some(finger));
        assert_eq!(follow(&v), FollowAction::Slide(camera.hold_point(off)));
        let mut v = view(&camera, &grid, &accepted, Some(off), Some(camera.hold_point(off)));
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Lift);
        // Without confirm, it stays down there.
        v.act = false;
        assert_eq!(follow(&v), FollowAction::Wait);
    }

    // B0.4: where letting go of a held finger does nothing.

    fn open_grid(w: i32, h: i32) -> Grid {
        Grid {
            w,
            h,
            units: vec![-1; (w * h) as usize],
            highlight: vec![0; (w * h) as usize],
        }
    }

    #[test]
    fn a_harmless_point_never_holds_an_accepted_tile() {
        let camera = Camera {
            origin: (240.0, 40.0),
            scale: 1.0,
        };
        let grid = open_grid(10, 10);
        let accepted: Vec<Tile> = (2..8).flat_map(|x| (2..8).map(move |y| (x, y))).collect();
        let point = harmless_point(&camera, &grid, &accepted, HARMLESS_AREA).unwrap();
        assert!(!accepted.contains(&camera.hold_tile_at(point)), "{point:?}");
        // It prefers the middle of the screen: not in a corner.
        let (x, y) = point;
        assert!((x - SCREEN_CENTRE.0).abs() < 120.0 && (y - SCREEN_CENTRE.1).abs() < 120.0);
    }

    #[test]
    fn a_harmless_point_prefers_a_tile_on_the_map() {
        // The game may clamp a held cursor to the map, so a point off the
        // map is only a last resort.
        let camera = Camera {
            origin: (240.0, 40.0),
            scale: 1.0,
        };
        let grid = open_grid(10, 10);
        let accepted = [(4, 4)];
        let point = harmless_point(&camera, &grid, &accepted, HARMLESS_AREA).unwrap();
        assert!(grid.contains(camera.hold_tile_at(point)), "{point:?}");
    }

    #[test]
    fn with_every_tile_accepted_a_harmless_point_is_off_the_map() {
        // A 2×2 map in the middle of the screen, all of it accepted.
        let camera = Camera {
            origin: (240.0, 140.0),
            scale: 1.0,
        };
        let grid = open_grid(2, 2);
        let accepted: Vec<Tile> = grid.tiles().collect();
        let point = harmless_point(&camera, &grid, &accepted, HARMLESS_AREA).unwrap();
        assert!(!grid.contains(camera.hold_tile_at(point)), "{point:?}");
    }

    #[test]
    fn no_harmless_point_when_nothing_qualifies() {
        // A one-point area whose held tile is accepted.
        let camera = CAMERA;
        let area = (200.0, 76.0, 0.0, 0.0);
        let tile = camera.hold_tile_at((200.0, 76.0));
        assert_eq!(harmless_point(&camera, &open_grid(5, 5), &[tile], area), None);
    }

    // B0.5: where a held finger counts.

    #[test]
    fn the_whole_screen_takes_holds_lower_down() {
        // Move select, attack select and placing hold on the whole screen
        // (the 5-argument Tactics_CtrlTest): a tile at y 260 has its finger
        // at 296, inside 320 less the margin.
        assert!(holdable_in((240.0, 260.0), WHOLE_SCREEN));
        assert!(!holdable_in((240.0, 260.0), UNIT_SELECT_AREA));
        assert!(!holdable_in((240.0, 290.0), WHOLE_SCREEN));
        // Unit select's rect, as `holdable` has it.
        assert!(holdable_in((240.0, 128.0), UNIT_SELECT_AREA));
        assert!(!holdable_in((240.0, 225.0), UNIT_SELECT_AREA));
        // Off the sides and top, as for a tap.
        for centre in [(10.0, 128.0), (470.0, 128.0), (240.0, 10.0)] {
            assert!(!holdable_in(centre, WHOLE_SCREEN), "{centre:?}");
        }
    }

    // B0 (move select, attack select, placing): the held finger that
    // follows a locked cursor.

    fn follow_camera() -> Camera {
        Camera {
            origin: (240.0, 40.0),
            scale: 1.0,
        }
    }

    fn view<'a>(
        camera: &'a Camera,
        grid: &'a Grid,
        accepted: &'a [Tile],
        cursor: Option<Tile>,
        finger: Option<Point>,
    ) -> FollowView<'a> {
        FollowView {
            camera,
            grid,
            accepted,
            cursor,
            finger,
            want_down: true,
            act: false,
            last_pan: None,
            hold_area: WHOLE_SCREEN,
            harmless_area: HARMLESS_AREA,
        }
    }

    // Unit select on the held finger (the user's choice, 2026-09-28): the
    // game's own cursor, drawn under the units, instead of touchHLE's.

    #[test]
    fn unit_selects_hold_area_is_where_holds_were_verified() {
        // The same tiles `holdable` allows (the finger at or above 248).
        for x in (0..=480).step_by(4) {
            for y in (0..=320).step_by(4) {
                let centre = (x as f32, y as f32);
                assert_eq!(holdable_in(centre, UNIT_SELECT_HOLD), holdable(centre), "{centre:?}");
            }
        }
    }

    fn unit_view<'a>(
        camera: &'a Camera,
        grid: &'a Grid,
        units: &'a [Tile],
        cursor: Tile,
        finger: Option<Point>,
    ) -> FollowView<'a> {
        FollowView {
            hold_area: UNIT_SELECT_HOLD,
            harmless_area: TAP_AREA,
            ..view(camera, grid, units, Some(cursor), finger)
        }
    }

    #[test]
    fn unit_select_lets_go_off_the_units_above_the_panel() {
        // Coming up (for L/R, MENU or a zoom), the finger lets go on a tile
        // with no unit, and above the status panel, MENU and the arrows.
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let units = [(3, 3), (4, 4), (3, 4), (4, 3)];
        let mut v = unit_view(&camera, &grid, &units, (3, 3), Some(camera.hold_point((3, 3))));
        v.want_down = false;
        let FollowAction::LiftAt((x, y)) = follow(&v) else {
            panic!("{:?}", follow(&v));
        };
        assert!(!units.contains(&camera.hold_tile_at((x, y))));
        assert!(tappable((x, y)), "{:?}", (x, y));
    }

    #[test]
    fn unit_select_taps_a_tile_too_low_to_hold() {
        // Tile (8, 8)'s centre is at y 232: its finger would be on the
        // status panel. With the map at its edge (no pan), confirm taps.
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let tile = (8, 8);
        let units = [tile];
        let mut v = unit_view(&camera, &grid, &units, tile, None);
        v.last_pan = Some(camera.origin);
        assert_eq!(follow(&v), FollowAction::Stuck);
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Tap(camera.tile_centre(tile)));
        // A pan first, while one still moves the map.
        v.last_pan = None;
        assert!(matches!(follow(&v), FollowAction::Pan(..)), "{:?}", follow(&v));
    }

    #[test]
    fn follow_presses_on_the_cursor_first() {
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let accepted = [(3, 3), (3, 4)];
        let v = view(&camera, &grid, &accepted, Some((3, 4)), None);
        assert_eq!(follow(&v), FollowAction::Press(camera.hold_point((3, 4))));
    }

    #[test]
    fn follow_slides_to_a_tile_on_screen() {
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let accepted = [(3, 3), (3, 4)];
        let finger = camera.hold_point((3, 3));
        let v = view(&camera, &grid, &accepted, Some((3, 4)), Some(finger));
        assert_eq!(follow(&v), FollowAction::Slide(camera.hold_point((3, 4))));
        // Already there: nothing to do.
        let v = view(&camera, &grid, &accepted, Some((3, 3)), Some(finger));
        assert_eq!(follow(&v), FollowAction::Wait);
    }

    #[test]
    fn follow_to_a_tile_off_screen_lifts_harmlessly_pans_then_presses() {
        let (camera, grid) = (follow_camera(), open_grid(40, 40));
        let accepted = [(3, 3), (30, 30)];
        let finger = camera.hold_point((3, 3));
        // Down on (3, 3); (30, 30) is far below the screen.
        let v = view(&camera, &grid, &accepted, Some((30, 30)), Some(finger));
        let FollowAction::LiftAt(point) = follow(&v) else {
            panic!("{:?}", follow(&v));
        };
        assert!(!accepted.contains(&camera.hold_tile_at(point)));
        // Up: pan toward it.
        let v = view(&camera, &grid, &accepted, Some((30, 30)), None);
        assert!(matches!(follow(&v), FollowAction::Pan(_, _)), "{:?}", follow(&v));
        // Once it's on screen: press.
        let near = Camera {
            origin: (240.0, 128.0 - 12.0 * 60.0),
            scale: 1.0,
        };
        let v = view(&near, &grid, &accepted, Some((30, 30)), None);
        assert_eq!(follow(&v), FollowAction::Press(near.hold_point((30, 30))));
    }

    #[test]
    fn follow_confirm_lets_go_on_the_cursor() {
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let accepted = [(3, 3), (3, 4)];
        let finger = camera.hold_point((3, 4));
        let mut v = view(&camera, &grid, &accepted, Some((3, 4)), Some(finger));
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Lift);
        // Not there yet: slide first, lift after.
        let mut v = view(&camera, &grid, &accepted, Some((3, 3)), Some(finger));
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Slide(camera.hold_point((3, 3))));
    }

    #[test]
    fn follow_back_home_is_slide_then_lift() {
        // Move select's back: the cursor goes to the unit's own tile, and
        // letting go there cancels the move.
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let own = (2, 2);
        let accepted = [own, (3, 3)];
        let finger = camera.hold_point((3, 3));
        let mut v = view(&camera, &grid, &accepted, Some(own), Some(finger));
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Slide(camera.hold_point(own)));
        let mut v = view(&camera, &grid, &accepted, Some(own), Some(camera.hold_point(own)));
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Lift);
    }

    #[test]
    fn follow_back_home_off_screen_lifts_harmlessly_first() {
        let (camera, grid) = (follow_camera(), open_grid(40, 40));
        let own = (30, 30);
        let accepted = [own, (3, 3)];
        let mut v = view(&camera, &grid, &accepted, Some(own), Some(camera.hold_point((3, 3))));
        v.act = true;
        assert!(matches!(follow(&v), FollowAction::LiftAt(_)), "{:?}", follow(&v));
    }

    #[test]
    fn follow_with_the_finger_wanted_up_lifts_where_nothing_happens() {
        // Zoom, and back in attack select: the finger comes up harmlessly.
        let (camera, grid) = (follow_camera(), open_grid(10, 10));
        let accepted = [(3, 3), (3, 4)];
        let mut v = view(&camera, &grid, &accepted, Some((3, 3)), Some(camera.hold_point((3, 3))));
        v.want_down = false;
        let FollowAction::LiftAt(point) = follow(&v) else {
            panic!("{:?}", follow(&v));
        };
        assert!(!accepted.contains(&camera.hold_tile_at(point)));
        // On a tile that isn't accepted already: just lift there.
        let mut v = view(&camera, &grid, &accepted, Some((3, 3)), Some(camera.hold_point((6, 6))));
        v.want_down = false;
        assert_eq!(follow(&v), FollowAction::Lift);
        // Up and wanted up: nothing.
        let mut v = view(&camera, &grid, &accepted, Some((3, 3)), None);
        v.want_down = false;
        assert_eq!(follow(&v), FollowAction::Wait);
    }

    #[test]
    fn follow_taps_a_tile_it_cannot_hold_on_confirm() {
        // The map's bottom row at its edge: too low for a finger 36 below,
        // and no pan brings it up. Confirm taps it instead.
        let camera = Camera {
            origin: (240.0, 40.0),
            scale: 1.0,
        };
        let grid = open_grid(12, 12);
        // Tile (11, 11)'s centre is at y 304: its finger would be at 340.
        let tile = (11, 11);
        let accepted = [tile];
        let last_pan = Some(camera.origin);
        let mut v = view(&camera, &grid, &accepted, Some(tile), None);
        v.last_pan = last_pan;
        assert_eq!(follow(&v), FollowAction::Stuck);
        v.act = true;
        assert_eq!(follow(&v), FollowAction::Tap(camera.tile_centre(tile)));
    }

    #[test]
    fn follow_never_lets_go_on_a_listed_tile_but_to_act() {
        // Every finger position and cursor on a small map: a Lift (without
        // act) never happens with the finger on an accepted tile, and a
        // LiftAt never goes to one.
        let camera = Camera {
            origin: (240.0, 60.0),
            scale: 1.0,
        };
        let grid = open_grid(6, 6);
        let accepted = [(1, 1), (1, 2), (2, 2), (3, 2), (4, 4)];
        for want_down in [true, false] {
            for cursor in accepted.iter().copied().map(Some).chain([None]) {
                for finger_tile in grid.tiles() {
                    let finger = camera.hold_point(finger_tile);
                    let mut v = view(&camera, &grid, &accepted, cursor, Some(finger));
                    v.want_down = want_down;
                    match follow(&v) {
                        FollowAction::Lift => {
                            assert!(!accepted.contains(&finger_tile), "{v:?}");
                        }
                        FollowAction::LiftAt(p) => {
                            assert!(!accepted.contains(&camera.hold_tile_at(p)), "{v:?}");
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // B1: the command ring.

    /// Seven item centres on a circle round (240, 160), item 0 at `start`
    /// degrees (screen y down, so 90 is straight down).
    fn ring(start: f32) -> Vec<Point> {
        (0..7)
            .map(|i| {
                let a = (start + i as f32 * 360.0 / 7.0).to_radians();
                (240.0 + 90.0 * a.cos(), 160.0 + 90.0 * a.sin())
            })
            .collect()
    }

    #[test]
    fn ring_neighbours_are_the_nearest_items_either_side() {
        // Front item 0 at the bottom (90°): item 1 at 141° is down-left,
        // item 6 at 39° down-right.
        let centres = ring(90.0);
        assert_eq!(ring_neighbour(&centres, 0, Side::Left), Some(1));
        assert_eq!(ring_neighbour(&centres, 0, Side::Right), Some(6));
        // Front item 3 at the top (270°): item 2 is up-left, item 4
        // up-right.
        let centres = ring(270.0 - 3.0 * 360.0 / 7.0);
        assert_eq!(ring_neighbour(&centres, 3, Side::Left), Some(2));
        assert_eq!(ring_neighbour(&centres, 3, Side::Right), Some(4));
    }

    #[test]
    fn ring_neighbour_when_turned_part_way() {
        // Front at 100°, a little left of the bottom.
        let centres = ring(100.0);
        assert_eq!(ring_neighbour(&centres, 0, Side::Left), Some(1));
        assert_eq!(ring_neighbour(&centres, 0, Side::Right), Some(6));
        // A hidden item (read as nowhere) is skipped.
        let mut hidden = centres.clone();
        hidden[1] = (f32::NAN, f32::NAN);
        assert_eq!(ring_neighbour(&hidden, 0, Side::Left), Some(2));
        // No items at all, or a front that isn't one.
        assert_eq!(ring_neighbour(&[], 0, Side::Left), None);
        assert_eq!(ring_neighbour(&centres, 9, Side::Left), None);
    }

    #[test]
    fn ring_steps_go_round_by_item_order() {
        // Several presses one way tap the item that many steps round, so
        // the ring turns once. Past the top, x no longer tells the sides
        // apart, so the steps follow the items' order.
        let centres = ring(90.0);
        assert_eq!(ring_steps(&centres, 0, Side::Left, 1), Some(1));
        assert_eq!(ring_steps(&centres, 0, Side::Left, 2), Some(2));
        assert_eq!(ring_steps(&centres, 0, Side::Left, 3), Some(3));
        assert_eq!(ring_steps(&centres, 0, Side::Right, 2), Some(5));
        // From a front at 5: right is 4, then 3.
        let centres = ring(90.0 - 5.0 * 360.0 / 7.0);
        assert_eq!(ring_steps(&centres, 5, Side::Right, 2), Some(3));
        assert_eq!(ring_steps(&centres, 5, Side::Left, 2), Some(0));
        // No neighbour that way: nothing.
        assert_eq!(ring_steps(&[], 0, Side::Left, 2), None);
    }

    #[test]
    fn ring_buttons() {
        let centres = ring(90.0);
        assert_eq!(ring_command(&centres, 0, Role::PrevSection), Some(centres[1]));
        assert_eq!(ring_command(&centres, 0, Role::NextSection), Some(centres[6]));
        assert_eq!(ring_command(&centres, 0, Role::Confirm), Some(centres[0]));
        // Back taps outside the ring (x < 100), which cancels.
        assert_eq!(ring_command(&centres, 0, Role::Back), Some(RING_CANCEL));
        assert!(RING_CANCEL.0 < 100.0);
        for role in [Role::Up, Role::Down, Role::PrevTab, Role::NextTab, Role::Info] {
            assert_eq!(ring_command(&centres, 0, role), None, "{role:?}");
        }
    }

    // B2: the ring's sub-screens.

    #[test]
    fn status_taps_flip_on_the_left_and_close_on_the_right() {
        // TacticsUnitMenu_CtrlStatus: x < 160 flips the page, x ≥ 160
        // goes back.
        assert_eq!(status_command(Role::Info), Some((80.0, 160.0)));
        assert_eq!(status_command(Role::Confirm), Some((80.0, 160.0)));
        assert_eq!(status_command(Role::Back), Some((320.0, 160.0)));
        assert_eq!(status_command(Role::Up), None);
    }

    #[test]
    fn skill_rows_are_60_high_from_8_on_the_right() {
        // TacticsUnitMenu_CtrlSkill: row = (y − 8) / 60, list at x ≥ 320;
        // the skill panel isn't the map, so there's no 36-point offset.
        for i in 0..5 {
            let (x, y) = skill_row_point(i);
            assert!(x >= 320.0 && x < 480.0);
            assert_eq!(skill_row_at(y), Some(i));
        }
        assert_eq!(skill_row_at(7.0), None);
        assert_eq!(skill_row_at(67.9), Some(0));
        assert_eq!(skill_row_at(68.0), Some(1));
    }

    #[test]
    fn only_the_rows_on_screen_are_used() {
        // The panel doesn't scroll: rows 0–4 fit in 320 points.
        assert_eq!(skill_rows(0), 0);
        assert_eq!(skill_rows(3), 3);
        assert_eq!(skill_rows(5), 5);
        assert_eq!(skill_rows(9), 5);
        assert!(skill_row_point(4).1 < 320.0);
    }

    // B4: attack select and attack info.

    #[test]
    fn attack_select_mode_picks_the_list() {
        // TacticsAttackSelect_CtrlRelease's switch on +0x72c: 0 and 1 look
        // units up in the target list, 2 and 3 tiles in the area list.
        assert_eq!(attack_list(0), Some(AttackList::Targets));
        assert_eq!(attack_list(1), Some(AttackList::Targets));
        assert_eq!(attack_list(2), Some(AttackList::Area));
        assert_eq!(attack_list(3), Some(AttackList::Area));
        assert_eq!(attack_list(4), None);
    }

    #[test]
    fn target_cycling_wraps() {
        assert_eq!(cycle(3, 0, true), 1);
        assert_eq!(cycle(3, 2, true), 0);
        assert_eq!(cycle(3, 0, false), 2);
        assert_eq!(cycle(1, 0, false), 0);
        assert_eq!(cycle(0, 0, true), 0);
    }

    #[test]
    fn a_harmless_tap_misses_targets_and_units() {
        let camera = follow_camera();
        let mut grid = open_grid(10, 10);
        // A target unit on (4, 4), another unit on (6, 3).
        grid.units[4 * 10 + 4] = 1;
        grid.units[3 * 10 + 6] = 2;
        let targets = [(4, 4), (5, 5)];
        let (x, y) = harmless_tap(&camera, &grid, &targets).unwrap();
        let tile = camera.tile_at((x, y));
        assert!(!targets.contains(&tile), "{tile:?}");
        assert_eq!(grid.unit_at(tile), None);
        // Not just above a unit: its sprite reaches up about a tile.
        for below in [(tile.0 + 1, tile.1), (tile.0, tile.1 + 1), (tile.0 + 1, tile.1 + 1)] {
            assert_eq!(grid.unit_at(below), None, "{below:?}");
        }
        assert!(tappable((x, y)));
    }

    #[test]
    fn a_harmless_tap_is_never_just_above_a_target() {
        // Units everywhere but a few tiles: the tap must pick one of those
        // whose lower neighbours are empty too.
        let camera = follow_camera();
        let mut grid = open_grid(10, 10);
        grid.units = vec![7; 100];
        for tile in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            grid.units[(tile.1 * 10 + tile.0) as usize] = -1;
        }
        let tap = harmless_tap(&camera, &grid, &[(0, 0)]).unwrap();
        assert_eq!(camera.tile_at(tap), (2, 2));
        // Nowhere safe at all: at 200% the map covers the whole tap area,
        // and there are units everywhere.
        grid.units = vec![7; 100];
        let everywhere = Camera {
            origin: (240.0, 36.0),
            scale: 2.0,
        };
        assert_eq!(harmless_tap(&everywhere, &grid, &[]), None);
    }

    #[test]
    fn attack_info_buttons() {
        // TacticsAttackInfo_Main: the band above the panels confirms, the
        // top cancels, x ≥ 360, 180–228 is the next target.
        assert_eq!(attack_info_taps(Role::Confirm, 1), vec![(240.0, 160.0)]);
        assert_eq!(attack_info_taps(Role::Back, 1), vec![(240.0, 20.0)]);
        assert_eq!(attack_info_taps(Role::NextTab, 3), vec![NEXT_TARGET]);
        // No "previous": all the way round.
        assert_eq!(attack_info_taps(Role::PrevTab, 3), vec![NEXT_TARGET; 2]);
        // One target: the shoulders do nothing.
        assert!(attack_info_taps(Role::NextTab, 1).is_empty());
        assert!(attack_info_taps(Role::PrevTab, 1).is_empty());
        assert!(attack_info_taps(Role::Up, 3).is_empty());
    }

    // B5: zoom.

    #[test]
    fn zoom_steps_by_20_within_100_to_200() {
        assert_eq!(zoom_step(100, true), Some(120));
        assert_eq!(zoom_step(130, true), Some(150));
        assert_eq!(zoom_step(190, true), Some(200));
        assert_eq!(zoom_step(200, true), None);
        assert_eq!(zoom_step(200, false), Some(180));
        assert_eq!(zoom_step(110, false), Some(100));
        assert_eq!(zoom_step(100, false), None);
        // Out of range (a misread): back into it.
        assert_eq!(zoom_step(250, false), Some(200));
    }

    // B6: the sortie's sort panel.

    #[test]
    fn sort_panel_rows_are_edit_troopers_rows() {
        let rows = sort_panel_rows();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0], (80.0, 37.0, 320.0, 46.0));
        assert_eq!(rows[5], (80.0, 267.0, 320.0, 46.0));
    }

    // B7: SKIP in the Listening Point scene.

    #[test]
    fn an_animation_shows_when_its_slot_is_in_use_and_shown() {
        assert!(anim_visible(3, 4, 1));
        assert!(!anim_visible(3, 0, 1)); // free slot
        assert!(!anim_visible(3, 4, 0)); // hidden
        assert!(!anim_visible(-1, 4, 1)); // no animation
        assert!(!anim_visible(512, 4, 1)); // past the table
    }

    #[test]
    fn phase_names_for_the_player_phases() {
        assert_eq!(phase_name(19), "unit select");
        assert_eq!(phase_name(22), "move select");
        assert_eq!(phase_name(24), "attack select");
        assert_eq!(phase_name(99), "other");
    }
}
