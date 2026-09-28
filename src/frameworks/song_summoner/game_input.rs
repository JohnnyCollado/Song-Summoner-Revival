/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Controller input for the game itself (everything but our picker).
//!
//! The game reads touch only through one struct, `_touch_work`, which
//! `-[MainView touchesBegan/Moved/Ended:]` fill by calling
//! `SysTouch_Began_F1` etc. (song-summoner-re.md §4). So a controller
//! "tap" is those same calls, made by us just before a frame runs.
//!
//! The per-frame hook is an override of `-[MainView mainLoop]` (the game's
//! frame timer callback) that does our work and then calls the game's own
//! `mainLoop`. We call the game's functions and read its memory, but never
//! write it.
//!
//! Frame order matters: `MainLoop_Main` runs the scene (which reads touch),
//! then `SysTouch_Main`, which clears the "began" flag and, after an
//! "ended", the whole struct. So a tap is Began before one frame and Ended
//! before a later one; both before the same frame would hide the Began.
//!
//! Menus are tasks in the game's task table, so we find open ones there and
//! read their layout from the task's work struct. Two kinds so far:
//! - `SysButtonMenu`, a column of buttons. The D-pad moves a focus between
//!   them (outlined by touchHLE, since the game has no focus of its own),
//!   confirm taps the focused button, back taps outside the menu, which
//!   cancels menus that allow that.
//! - `SysDialog`, a message box with buttons side by side (like "Start
//!   from last Auto save?" No / Yes). The same focus/confirm/back as
//!   above; with no buttons, confirm taps it to advance the message.
//! - `SysDrum2`, a rotating drum (the title menu). It already has a cursor:
//!   the item in its middle band. Up/down tap below/above that band, which
//!   rotates the drum a step (a tap on one side brings in the item from
//!   the other), and confirm taps the band.
//!
//! - The card list (`_cardlist_work`; picking a trooper to delete, and
//!   more): left/right tap beside the middle card, which turns the list a
//!   card, confirm taps the middle card, down moves to the icons along the
//!   bottom. The north button opens the status panel (its icon), or flips
//!   the open panel's page. Back closes the panel if it's open, otherwise
//!   taps the exit icon.
//! - `SysMenu`, a scrolling list (items). Up/down move a focus row by row
//!   (left/right a page); when it leaves the rows on screen, a short drag
//!   scrolls the list a row toward it. Confirm taps the focused row once
//!   it's on screen, back taps off the list (its cancel).
//! - Screens that draw and hit-test their own buttons (the Hip-O-Drome's
//!   main menu): [SCENE_BUTTONS] says where their button sprites' ids are,
//!   and the sprites' rectangles come from the game's sprite table, the
//!   same ones `SysPrim_Touch_DrawRect` tests. These work like a button
//!   menu, but only while no menu or dialog task is open over them.
//!
//! - The Options screen (pause menu > Options): up/down move over the
//!   volume slider, the three switches and the back icon. Left/right drag
//!   the volume knob a tenth, or turn a switch off/on; confirm flips a
//!   switch. It covers the pause menu, which stays open under it, so while
//!   it's up nothing else counts.
//! - The Help screen (pause menu > Help), which also covers the pause
//!   menu. On its list, up/down move a focus row by row, gliding the list
//!   with a short drag to show it, confirm opens the item, left/right or
//!   the shoulders change tab, and back taps Exit. On an item's page,
//!   up/down scroll the text, left/right or the shoulders go to the
//!   previous/next item, and back closes it.
//!
//! With no menu open, confirm taps the middle of the screen, which is what
//! "press start" and "tap to continue" text want.
//!
//! - Battle's unit select (the map with the status panel): the D-pad moves
//!   a tile cursor along the grid's axes (a gold diamond drawn by touchHLE
//!   inside the tile's edges), panning the map with a flick when it leaves
//!   the area a finger can reach; confirm holds a finger on its tile (the
//!   game picks a held finger's unit by tile, a tap's by sprite), the
//!   shoulders tap the curved arrows (previous/next unit) and
//!   Start taps MENU. An enemy's status (after tapping one): confirm or back
//!   close it, north flips its page.
//! - The rest of battle, one handler per phase (`battle::phase_owner`): the
//!   command ring (left/right tap the neighbouring item, confirm the front
//!   one, back outside), its skill panel and status; move select, attack
//!   select and deploy placing, where a held virtual finger ([Finger])
//!   follows a cursor that moves freely on the map, so the game's own
//!   previews show, and letting go acts; attack info; the deploy's
//!   sort panel and map view; the victory terms. The triggers zoom the map
//!   (`TacticsMapCursor_Set_Zoom`) in unit, move and attack select. Phases
//!   nobody drives (walking, the attack) drop presses. See
//!   dev-docs/battle-controller-plan.md.
//!
//! In debug builds, [battle_debug] also logs battle's state, and can
//! outline every map tile where `battle.rs` thinks it is
//! ([DRAW_TILE_GRID], off).

use super::battle::{self, Camera, Grid, Tile};
use super::pad::{self, Role};
use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::gles::present::{FocusMarker, FocusShape};
use crate::mem::{ConstPtr, MutPtr, Ptr};
use crate::objc::{id, msg, nil, Override, SEL};
use crate::window::PadButton;
use crate::Environment;
use std::collections::VecDeque;

/// Frames a synthetic finger stays down: Began before the first, Ended
/// before the last.
const HOLD_FRAMES: u32 = 2;
/// Taps queued beyond this are dropped, so mashing a button while the game
/// is busy (loading) doesn't replay a burst of taps later.
const MAX_QUEUED_TAPS: usize = 2;
/// The same for D-pad presses and the like.
const MAX_QUEUED_COMMANDS: usize = 4;

/// Screen centre, in the game view's 480×320 landscape points.
const CENTER: (f32, f32) = (240.0, 160.0);
/// A point off every menu: the top-left corner.
const OUTSIDE: (f32, f32) = (2.0, 2.0);
/// Where `SysButtonMenu_Enable_CancelButton` puts the cancel icon
/// (`tc_icon.png`, 48×48, centred here).
const CANCEL_BUTTON: (f32, f32) = (456.0, 296.0);
/// The cutscenes' SKIP button (`ui_skip.yas`, drawn at (440, 12)).
/// `Script_Main` opens its "Skip?" dialog on a finger-up at x >= 440,
/// y <= 40 while the button is enabled.
const SKIP_BUTTON: (f32, f32) = (460.0, 20.0);

/// Slots in the game's task table (`_task_manage`), and bytes per slot.
const TASK_SLOTS: u32 = 16;
const TASK_SLOT_SIZE: u32 = 0x1c;
/// A task slot's state when the task is running.
const TASK_RUNNING: u32 = 2;
/// A `SysButtonMenu`'s own state once its buttons are shown and it takes
/// touches (`SysButtonMenu_Main` sets it; `SysButtonMenu_Check` needs it).
const MENU_ACCEPTING: u32 = 2;

/// A group of buttons a screen draws and hit-tests itself.
pub struct SceneButtons {
    /// The scene task's main function.
    pub main: &'static str,
    /// Offsets in the scene's work struct of the buttons' sprite ids.
    pub buttons: &'static [u32],
    /// Offset of the sprite id of a back icon, which back taps.
    pub back: Option<u32>,
    /// A panel shown over the card list: checked before the card list,
    /// which otherwise comes first.
    pub over_cards: bool,
    /// Offset of the `int` index of the button the game has selected (−1
    /// for none), for screens where the first tap only selects a button
    /// and a tap on the selected one presses it.
    pub selected: Option<u32>,
}

/// Screens that draw their own buttons (song-summoner-re.md §4). The first
/// group of a scene with a button shown is used, so more specific groups
/// (a panel over a menu) come first.
const SCENE_BUTTONS: &[SceneButtons] = &[
    // Edit Troopers' sort panel: six rows (tap to change that row's
    // setting), and the back icon (Teammake_Check).
    SceneButtons {
        main: "__Z13Teammake_Mainv",
        buttons: &[0x4c, 0x50, 0x54, 0x58, 0x5c, 0x60],
        back: Some(0x3c),
        over_cards: true,
        selected: None,
    },
    // The Hip-O-Drome's panel after picking a song: Create Trooper, No,
    // and the back icon.
    SceneButtons {
        main: "__Z11Palace_Mainv",
        buttons: &[0x47c, 0x480],
        back: Some(0x48c),
        over_cards: false,
        selected: None,
    },
    // The Hip-O-Drome menu: Create Trooper, Pick of the Pops, Delete
    // Trooper, Learn About Tune Troopers, Leave.
    SceneButtons {
        main: "__Z11Palace_Mainv",
        buttons: &[0x454, 0x458, 0x45c, 0x460, 0x464],
        back: None,
        over_cards: false,
        selected: None,
    },
    // The world map's location menu (WorldmapMenu_Ctrl, phase 5): three
    // icons and the back icon, shown only while it's open. A first tap on
    // an icon selects it (`+0x14`), a second presses it; back takes one.
    SceneButtons {
        main: "__Z13Worldmap_Mainv",
        buttons: &[0x2f0, 0x2f4, 0x2f8],
        back: Some(0x2fc),
        over_cards: false,
        selected: Some(0x14),
    },
    // A town's icon row (Soul Master's Place: Hip-O-Drome, troopers, data,
    // map). Up to seven icons; each town shows its own subset. Like the
    // location menu, TownMenu_Ctrl selects on the first tap (`+0x14`, a
    // slot) and presses on the second.
    SceneButtons {
        main: "__Z9Town_Mainv",
        buttons: &[0x60, 0x64, 0x68, 0x6c, 0x70, 0x74, 0x78],
        back: None,
        over_cards: false,
        selected: Some(0x14),
    },
];
const SCENE_GROUPS: usize = SCENE_BUTTONS.len();
/// How far one drag step moves the finger to scroll a `SysMenu` list by a
/// row: `SysMenu_Check2` turns it into a speed of 4.8 / 24 = 0.2 rows per
/// frame, which `menumain` slows by 10% a frame and snaps to a whole row
/// below 0.08, which comes to about 1.2 rows, rounded to 1.
const LIST_DRAG: f32 = 4.8;
/// A `SysMenu` row's height (`SysMenu_Check2`'s hit test).
const LIST_ROW_H: f32 = 40.0;
/// A point on the card list's status panel (x <= 160 in status mode).
const STATUS_PANEL: (f32, f32) = (80.0, 160.0);

/// `_cardlist_work` offsets of the bottom icons' sprite ids, in the order
/// `CardList_Flow_Select` tests them (regions 5 to 8).
const CARD_LIST_ICONS: [u32; 4] = [0x3c8, 0x3c4, 0x3cc, 0x3d0];
/// Region 5: `CardList_Main` toggles the status panel (`+0x58`).
const CARD_LIST_STATUS_ICON: u32 = 0x3c8;
/// Region 6: `CardList_Main` closes the panel and leaves the list.
const CARD_LIST_EXIT_ICON: u32 = 0x3c4;
/// A height in the card list's card band (40 < y < 224), where a tap more
/// than 55 points from the centre turns the list a card and a tap within
/// 55 picks the middle card.
const CARD_Y: f32 = 150.0;
/// How far beside the middle card to tap to turn the list one card: past
/// 55, short of the two-card zones (x <= 60, x >= 420), and right of x =
/// 160, which the status panel takes in one mode.
const CARD_STEP: f32 = 75.0;

/// Bytes per entry of the sprite table `_prim_work`.
const PRIM_SIZE: u32 = 0x54;

/// One synthetic touch call, made before a frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TouchStep {
    Began(f32, f32),
    /// The finger's new position, then its previous one.
    Moved(f32, f32, f32, f32),
    Ended(f32, f32),
}

/// A synthetic finger's whole movement.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Gesture {
    Tap(f32, f32),
    /// Down at (x, y), then one move of `dy`, then up: the game sees a
    /// flick, which scrolls lists and never picks anything.
    Drag { x: f32, y: f32, dy: f32 },
    /// Down at (x, y), then one move across to `to_x`, then up: dragging
    /// the Options screen's volume knob.
    Slide { x: f32, y: f32, to_x: f32 },
    /// Down at (x, y), then one move by (dx, dy), then up: a flick that pans
    /// the world map.
    Pan { x: f32, y: f32, dx: f32, dy: f32 },
    /// Down at (x, y) for `frames` frames without moving, then up: a hold
    /// (battle's map takes the tile under a held finger, not the sprite).
    Hold { x: f32, y: f32, frames: u32 },
}

impl Gesture {
    fn start(self) -> (f32, f32) {
        match self {
            Gesture::Tap(x, y)
            | Gesture::Drag { x, y, .. }
            | Gesture::Slide { x, y, .. }
            | Gesture::Pan { x, y, .. }
            | Gesture::Hold { x, y, .. } => (x, y),
        }
    }

    fn end(self) -> (f32, f32) {
        match self {
            Gesture::Tap(x, y) | Gesture::Hold { x, y, .. } => (x, y),
            Gesture::Drag { x, y, dy } => (x, y + dy),
            Gesture::Slide { y, to_x, .. } => (to_x, y),
            Gesture::Pan { x, y, dx, dy } => (x + dx, y + dy),
        }
    }
}

/// Turns gestures into touch calls spread over frames.
#[derive(Default)]
pub struct TapQueue {
    pending: VecDeque<Gesture>,
    /// The gesture under way, and frames since its Began.
    current: Option<(Gesture, u32)>,
    /// Keep a hold's finger down past its frames (confirm is held).
    keep: bool,
}

impl TapQueue {
    pub fn tap(&mut self, x: f32, y: f32) {
        self.push(Gesture::Tap(x, y));
    }

    pub fn drag(&mut self, x: f32, y: f32, dy: f32) {
        self.push(Gesture::Drag { x, y, dy });
    }

    pub fn push(&mut self, gesture: Gesture) {
        if self.pending.len() < MAX_QUEUED_TAPS {
            self.pending.push_back(gesture);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.current.is_none() && self.pending.is_empty()
    }

    /// Drop everything, e.g. taps made while the game can't take them.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.current = None;
        self.keep = false;
    }

    /// Keep a [Gesture::Hold]'s finger down after its frames are up, until
    /// called with `false` (a button that's still held). Its frames are
    /// still the least it's down for.
    pub fn keep_down(&mut self, keep: bool) {
        self.keep = keep;
    }

    /// The call to make before the next frame, if any.
    pub fn next_frame(&mut self) -> Option<TouchStep> {
        match self.current {
            // A tap or hold: still until its time is up, then up.
            Some((still @ (Gesture::Tap(..) | Gesture::Hold { .. }), frames)) => {
                let (down, kept) = match still {
                    Gesture::Hold { frames, .. } => (frames, self.keep),
                    _ => (HOLD_FRAMES, false),
                };
                if frames + 1 >= down && !kept {
                    self.current = None;
                    let (x, y) = still.end();
                    Some(TouchStep::Ended(x, y))
                } else {
                    self.current = Some((still, frames + 1));
                    None
                }
            }
            // A drag or slide: one move, then up.
            Some((moving, 0)) => {
                self.current = Some((moving, 1));
                let ((x, y), (to_x, to_y)) = (moving.start(), moving.end());
                Some(TouchStep::Moved(to_x, to_y, x, y))
            }
            Some((moving, _)) => {
                self.current = None;
                let (x, y) = moving.end();
                Some(TouchStep::Ended(x, y))
            }
            None => {
                let gesture = self.pending.pop_front()?;
                self.current = Some((gesture, 0));
                let (x, y) = gesture.start();
                Some(TouchStep::Began(x, y))
            }
        }
    }
}

/// Frames a held finger stays still after going down before it may move or
/// lift. `Tactics_CtrlTest` only calls a finger a hold after more than 6
/// still frames, and `SysTouch_Moved_F1` sets the flick flag at once, so an
/// earlier move would turn the hold into a map scroll (and an earlier lift
/// into a tap).
const MIN_STILL_FRAMES: u32 = 8;

/// A queued step of a [Finger].
#[derive(Copy, Clone, Debug, PartialEq)]
enum FingerStep {
    Press(f32, f32),
    Slide(f32, f32),
    Lift,
}

/// A virtual finger that stays down and moves between spots, for the
/// battle screens that preview while a finger is held (move and attack
/// select, the skill panel, deploy placing). Like [TapQueue], it makes at
/// most one touch call per frame; the two are never used at once.
#[derive(Default)]
pub struct Finger {
    /// Where the finger is down, and frames since its Began.
    down: Option<((f32, f32), u32)>,
    steps: VecDeque<FingerStep>,
}

impl Finger {
    /// Put the finger down (or, if it will already be down, slide it).
    pub fn press(&mut self, x: f32, y: f32) {
        self.steps.push_back(FingerStep::Press(x, y));
    }

    /// Move the finger, once it has been down [MIN_STILL_FRAMES].
    pub fn slide_to(&mut self, x: f32, y: f32) {
        self.steps.push_back(FingerStep::Slide(x, y));
    }

    /// Let go where the finger is, once it has been down
    /// [MIN_STILL_FRAMES].
    pub fn lift(&mut self) {
        self.steps.push_back(FingerStep::Lift);
    }

    /// Where the finger is down, or will be once the queued steps have run.
    pub fn finger(&self) -> Option<(f32, f32)> {
        self.steps
            .iter()
            .fold(self.down.map(|(at, _)| at), |at, step| match *step {
                FingerStep::Press(x, y) => Some((x, y)),
                FingerStep::Slide(x, y) => at.map(|_| (x, y)),
                FingerStep::Lift => None,
            })
    }

    /// Nothing queued: a finger that's simply down and still is idle.
    pub fn is_idle(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn is_down(&self) -> bool {
        self.down.is_some()
    }

    /// Drop the finger without an Ended (as [TapQueue::clear]), e.g. when
    /// the phase it was held in has gone.
    pub fn clear(&mut self) {
        self.down = None;
        self.steps.clear();
    }

    /// The call to make before the next frame, if any.
    pub fn next_frame(&mut self) -> Option<TouchStep> {
        if let Some((_, frames)) = self.down.as_mut() {
            *frames += 1;
        }
        loop {
            let step = *self.steps.front()?;
            match (step, self.down) {
                (FingerStep::Press(x, y), None) => {
                    self.steps.pop_front();
                    self.down = Some(((x, y), 0));
                    return Some(TouchStep::Began(x, y));
                }
                // Nothing to move or lift: drop it.
                (FingerStep::Slide(..) | FingerStep::Lift, None) => {
                    self.steps.pop_front();
                }
                (_, Some((_, frames))) if frames < MIN_STILL_FRAMES => return None,
                (FingerStep::Press(x, y) | FingerStep::Slide(x, y), Some((from, frames))) => {
                    self.steps.pop_front();
                    self.down = Some(((x, y), frames));
                    return Some(TouchStep::Moved(x, y, from.0, from.1));
                }
                (FingerStep::Lift, Some((at, _))) => {
                    self.steps.pop_front();
                    self.down = None;
                    return Some(TouchStep::Ended(at.0, at.1));
                }
            }
        }
    }
}

/// The button to focus in a widget: `default` when it has just appeared
/// (`appeared`: it wasn't open last frame, even at the same address) or
/// the focus was elsewhere; else where the focus was.
pub fn widget_focus(
    focus: Option<(u32, usize)>,
    work: u32,
    appeared: bool,
    default: usize,
    len: usize,
) -> usize {
    match focus {
        Some((w, i)) if !appeared && w == work && i < len => i,
        _ if default < len => default,
        _ => 0,
    }
}

/// An open `SysButtonMenu`, from its work struct (song-summoner-re.md §4):
/// `+0xc` count, `+0x18/+0x1a` position, `+0x1c/+0x1e` button size (all
/// but the count are `short`s), `+0x20` "tap outside cancels", `+0x28` the
/// cancel icon's prim (-1 if none), `+0x2c` a pointer to one "enabled"
/// `int` per button.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    /// Address of the work struct, which identifies the menu while open.
    pub work: u32,
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
    pub enabled: Vec<bool>,
    pub outrange_cancel: bool,
    pub cancel_button: bool,
}

impl Menu {
    /// The centre of button `i`. The buttons are a column centred on the
    /// menu's position, 2 points apart (SysButtonMenu_Position).
    pub fn button_center(&self, i: usize) -> (f32, f32) {
        let count = self.enabled.len() as i32;
        let h = i32::from(self.h);
        // C integer division, as in the game.
        let first = i32::from(self.y) - (h * (count - 1)) / 2;
        (f32::from(self.x), (first + i as i32 * (h + 2)) as f32)
    }

    /// Button `i` as (x, y, width, height).
    pub fn button_rect(&self, i: usize) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.button_center(i);
        let (w, h) = (f32::from(self.w), f32::from(self.h));
        (cx - w / 2.0, cy - h / 2.0, w, h)
    }
}

/// The next enabled button `step` away from `from`, wrapping round. Stays
/// at `from` if none is enabled.
pub fn next_enabled(enabled: &[bool], from: usize, step: i32) -> usize {
    let n = enabled.len() as i32;
    if n == 0 {
        return from;
    }
    let mut i = from as i32;
    for _ in 0..n {
        i = (i + step.signum()).rem_euclid(n);
        if enabled[i as usize] {
            return i as usize;
        }
    }
    from
}

/// Which of the `open` menus (work addresses, in task table order) gets the
/// focus: one that just opened (a menu opened over another, like a
/// confirmation, is where the player is), else the focused one while it's
/// still open, else the last.
pub fn choose_menu(open: &[u32], known: &[u32], focused: Option<u32>) -> Option<usize> {
    open.iter()
        .rposition(|work| !known.contains(work))
        .or_else(|| focused.and_then(|f| open.iter().position(|&w| w == f)))
        .or_else(|| open.len().checked_sub(1))
}

/// A `SysDialog` (song-summoner-re.md §4), from its work struct: `+0x1c`
/// button count, then 0x14-byte button entries at `+0x28` (`short` x, y,
/// w, h of the top-left-based rect, `int` enabled at `+8`), and `+0xf0`
/// "a tap off the buttons cancels".
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub work: u32,
    pub rects: Vec<(f32, f32, f32, f32)>,
    pub enabled: Vec<bool>,
    pub outside_cancels: bool,
}

/// What the focus/confirm/back logic needs from a button menu or dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonSet {
    pub rects: Vec<(f32, f32, f32, f32)>,
    pub enabled: Vec<bool>,
    /// Where back taps, if it can cancel.
    pub back: Option<(f32, f32)>,
    /// The first tap on a button only selects it (the game highlights it
    /// and describes it); a tap on the selected one presses it.
    pub two_tap: bool,
    /// For those, the button the game has selected.
    pub selected: Option<usize>,
}

impl From<&Menu> for ButtonSet {
    fn from(menu: &Menu) -> ButtonSet {
        ButtonSet {
            rects: (0..menu.enabled.len()).map(|i| menu.button_rect(i)).collect(),
            enabled: menu.enabled.clone(),
            // The cancel icon, if the menu shows one, is what the player
            // would tap. Else a tap off the menu, where that cancels.
            back: if menu.cancel_button {
                Some(CANCEL_BUTTON)
            } else {
                menu.outrange_cancel.then_some(OUTSIDE)
            },
            two_tap: false,
            selected: None,
        }
    }
}

impl From<&Dialog> for ButtonSet {
    fn from(dialog: &Dialog) -> ButtonSet {
        ButtonSet {
            rects: dialog.rects.clone(),
            enabled: dialog.enabled.clone(),
            back: dialog.outside_cancels.then_some(OUTSIDE),
            two_tap: false,
            selected: None,
        }
    }
}

fn rect_center((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// The part of a `SysDialog` button's touch area that its art covers, for
/// the outline. The touch area (157×64) reaches well past the button
/// (142×44) on the right and below; measured on screen.
pub fn dialog_button_art((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (x + 2.0, y + 4.0, (w - 15.0).max(1.0), (h - 20.0).max(1.0))
}

/// Pull an outline's edges at least 4 points inside the 480×320 screen, so
/// the marker drawn just outside them (about 3 points) is all visible.
/// Help's rows, for one, span the whole width.
pub fn keep_on_screen((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    const MARGIN: f32 = 4.0;
    let (x0, y0) = (x.max(MARGIN), y.max(MARGIN));
    let (x1, y1) = ((x + w).min(480.0 - MARGIN), (y + h).min(320.0 - MARGIN));
    (x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
}

/// The outline for a `SysDialog` button: its art with the same space around
/// it as the title menu's buttons get (about 9 points either side, 5 above
/// and below), so the two look alike.
pub fn dialog_outline(area: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = dialog_button_art(area);
    (x - 9.0, y - 5.0, w + 18.0, h + 10.0)
}

/// One command on a button set with button `index` focused: the new
/// focus, and where to tap, if anywhere.
pub fn button_command(set: &ButtonSet, index: usize, role: Role) -> (usize, Option<(f32, f32)>) {
    if set.rects.is_empty() {
        // Just a message: confirm taps it, to show the rest or close it.
        return (0, (role == Role::Confirm).then_some(CENTER));
    }
    match role {
        // Menus are columns and dialogs rows, so both axes move.
        Role::Up | Role::PrevSection => (next_enabled(&set.enabled, index, -1), None),
        Role::Down | Role::NextSection => (next_enabled(&set.enabled, index, 1), None),
        // A disabled button still gets its tap: the game plays its "can't"
        // sound, which is better than silence.
        Role::Confirm => (index, Some(rect_center(set.rects[index]))),
        Role::Back => (index, set.back),
        _ => (index, None),
    }
}

/// Where the game's selected slot (−1 for none) is among the shown
/// buttons, `shown` holding each one's slot.
pub fn selected_position(shown: &[usize], slot: i32) -> Option<usize> {
    let slot = usize::try_from(slot).ok()?;
    shown.iter().position(|&s| s == slot)
}

/// One command on a [ButtonSet::two_tap] set: the new focus, and the taps
/// to make. Moving taps the new button, so the game selects and describes
/// it; confirm taps the focused one until it's pressed.
pub fn select_command(set: &ButtonSet, index: usize, role: Role) -> (usize, Vec<(f32, f32)>) {
    let Some(&rect) = set.rects.get(index) else {
        return (index, Vec::new());
    };
    let step = match role {
        Role::Up | Role::PrevSection => -1,
        Role::Down | Role::NextSection => 1,
        // Selected, it's pressed by one tap; else the first only selects it.
        Role::Confirm if set.selected == Some(index) => return (index, vec![rect_center(rect)]),
        Role::Confirm => return (index, vec![rect_center(rect); 2]),
        Role::Back => return (index, set.back.into_iter().collect()),
        _ => return (index, Vec::new()),
    };
    let next = next_enabled(&set.enabled, index, step);
    // Never tap the selected button by moving: that would press it.
    if set.selected == Some(next) {
        return (next, Vec::new());
    }
    (next, vec![rect_center(set.rects[next])])
}

/// A sprite's rectangle as `SysPrim_Touch_DrawRect` tests it: `_prim_work`
/// entries hold float x, y, w, h at `+0xc..+0x18`, and x, y are the centre
/// when the "centred" flag at `+0x38` is set.
pub fn prim_rect(x: f32, y: f32, w: f32, h: f32, centred: bool) -> (f32, f32, f32, f32) {
    if centred {
        (x - w / 2.0, y - h / 2.0, w, h)
    } else {
        (x, y, w, h)
    }
}

/// The card list (song-summoner-re.md §4): the centre of its card strip,
/// and its shown bottom icons, left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct CardList {
    pub centre_x: f32,
    /// The shown bottom icons, left to right.
    pub icons: Vec<(f32, f32, f32, f32)>,
    /// The icon that opens and closes the status panel, if shown.
    pub status_icon: Option<(f32, f32, f32, f32)>,
    /// The icon that leaves the list, if shown.
    pub exit_icon: Option<(f32, f32, f32, f32)>,
    /// The status panel is open on the left (`+0x58`).
    pub status_panel: bool,
}

/// Whether a card list icon slot holds a real icon. Before the list is set
/// up (and after it closes) the slots hold sprite 0, the full-screen
/// backdrop, which would pass for an open list centred at x = 0.
pub fn is_card_icon((_, _, w, h): (f32, f32, f32, f32)) -> bool {
    // The real icons are about 48×48.
    w <= 120.0 && h <= 120.0
}

/// Where the controller is in the card list: on the cards, or on icon `i`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CardFocus {
    Cards,
    Icon(usize),
}

impl CardList {
    /// The middle card, for the outline: from its rank header to the bottom
    /// of its song bar (measured on screen; the card art itself is drawn
    /// by the game, so there's no rect to read).
    pub fn card_rect(&self) -> (f32, f32, f32, f32) {
        (self.centre_x - 54.0, 57.0, 108.0, 165.0)
    }

    pub fn focus_rect(&self, focus: CardFocus) -> (f32, f32, f32, f32) {
        match focus {
            CardFocus::Icon(i) if i < self.icons.len() => self.icons[i],
            _ => self.card_rect(),
        }
    }
}

/// One command on the card list: the new focus, and where to tap.
pub fn card_list_command(
    list: &CardList,
    focus: CardFocus,
    role: Role,
) -> (CardFocus, Option<(f32, f32)>) {
    let focus = match focus {
        CardFocus::Icon(i) if i >= list.icons.len() => CardFocus::Cards,
        other => other,
    };
    let c = list.centre_x;
    let last_icon = list.icons.len().checked_sub(1);
    match (focus, role) {
        (CardFocus::Cards, Role::PrevSection) => (focus, Some((c - CARD_STEP, CARD_Y))),
        (CardFocus::Cards, Role::NextSection) => (focus, Some((c + CARD_STEP, CARD_Y))),
        (CardFocus::Cards, Role::Confirm) => (focus, Some((c, CARD_Y))),
        (CardFocus::Cards, Role::Down) if last_icon.is_some() => (CardFocus::Icon(0), None),
        (CardFocus::Icon(_), Role::Up) => (CardFocus::Cards, None),
        (CardFocus::Icon(i), Role::PrevSection) => (CardFocus::Icon(i.saturating_sub(1)), None),
        (CardFocus::Icon(i), Role::NextSection) => {
            (CardFocus::Icon((i + 1).min(last_icon.unwrap_or(0))), None)
        }
        (CardFocus::Icon(i), Role::Confirm) => (focus, Some(rect_center(list.icons[i]))),
        // North opens the status panel, or flips an open one between stats
        // and skills (tapping it).
        (_, Role::Info) if list.status_panel => (focus, Some(STATUS_PANEL)),
        (_, Role::Info) => (focus, list.status_icon.map(rect_center)),
        // Back closes an open panel, else leaves the list.
        (_, Role::Back) if list.status_panel => (focus, list.status_icon.map(rect_center)),
        (_, Role::Back) => (focus, list.exit_icon.map(rect_center)),
        _ => (focus, None),
    }
}

/// A `SysMenu` list (song-summoner-re.md §4), from its work struct: item i
/// is 0x34 bytes at `+0xc`, with an "enabled" byte at `+0x14` and its row's
/// `short` x, y at `+0x30`/`+0x32`; `+2` the cursor, `+4` the count, `+5`
/// the rows shown, `+0xd0c` the row width, `+0xd18` the scroll position (in
/// rows) and `+0xd1c` its speed.
#[derive(Clone, Debug, PartialEq)]
pub struct ListMenu {
    pub work: u32,
    pub rows: Vec<(f32, f32, f32, f32)>,
    pub enabled: Vec<bool>,
    /// The first row on screen (the scroll position).
    pub first: f32,
    pub shown: usize,
    /// Not scrolling.
    pub settled: bool,
    pub cursor: usize,
    /// The menu's own box across (x, width: `+0xd08`, `+0xd0c`), which
    /// its rows overhang on the right.
    pub span: Option<(f32, f32)>,
}

/// How far the controller's outline is pulled in from a list row: the
/// marker is drawn about 3 points outside the rectangle, and rows are
/// packed edge to edge.
const LIST_OUTLINE_INSET: (f32, f32) = (4.0, 3.0);

/// The outline for a `SysMenu` row: inside the menu's box across (`span`,
/// if it overlaps the row), pulled in on the right and at the top and
/// bottom. A row's touch area starts inside the panel but is as wide as
/// the menu, so it overhangs the panel's right edge.
pub fn list_outline((x, y, w, h): (f32, f32, f32, f32), span: Option<(f32, f32)>) -> (f32, f32, f32, f32) {
    let (ix, iy) = LIST_OUTLINE_INSET;
    let mut right = x + w;
    if let Some((sx, sw)) = span {
        let clipped = right.min(sx + sw);
        if clipped > x.max(sx) {
            right = clipped;
        }
    }
    (x, y + iy, (right - ix - x).max(1.0), (h - 2.0 * iy).max(1.0))
}

impl ListMenu {
    /// Row `i` is on screen, and the list is still.
    pub fn row_visible(&self, i: usize) -> bool {
        let first = self.first.round().max(0.0) as usize;
        self.settled && i >= first && i < first + self.shown
    }

    /// A drag scrolling the list a row toward row `i`, if it's off screen
    /// and the list is still.
    pub fn scroll_toward(&self, i: usize) -> Option<Gesture> {
        if !self.settled || self.row_visible(i) || self.rows.is_empty() {
            return None;
        }
        let first = (self.first.round().max(0.0) as usize).min(self.rows.len() - 1);
        let (x, y) = rect_center(self.rows[first]);
        // Finger up moves the list on to later rows.
        let dy = if i < first { LIST_DRAG } else { -LIST_DRAG };
        Some(Gesture::Drag { x, y, dy })
    }
}

/// What a command does to a list: the new focus, and a tap to make, or
/// `None` for the tap if the command has to wait (for a scroll).
pub fn list_command(
    list: &ListMenu,
    focus: usize,
    role: Role,
) -> (usize, Option<Option<(f32, f32)>>) {
    let last = list.rows.len().saturating_sub(1);
    let page = list.shown.max(1);
    match role {
        Role::Up => (focus.saturating_sub(1), Some(None)),
        Role::Down => ((focus + 1).min(last), Some(None)),
        Role::PrevSection => (focus.saturating_sub(page), Some(None)),
        Role::NextSection => ((focus + page).min(last), Some(None)),
        Role::Confirm if list.rows.is_empty() => (focus, Some(None)),
        Role::Confirm if list.row_visible(focus) => {
            (focus, Some(Some(rect_center(list.rows[focus]))))
        }
        Role::Confirm => (focus, None),
        // A tap off every row is the list's cancel (SysMenu_Check2: -2).
        Role::Back => (focus, Some(Some(OUTSIDE))),
        _ => (focus, Some(None)),
    }
}

/// Entries in `_worldmap_symbol`, and bytes per entry.
const WORLD_SYMBOLS: u32 = 32;
const WORLD_SYMBOL_SIZE: u32 = 0x1c;
/// `Worldmap_Check_SymbolTouch`'s rect around a location: this far either
/// side, and above and below, in map points.
const WORLD_HIT_W: f32 = 36.0;
const WORLD_HIT_H: f32 = 24.0;
/// The furthest a pan moves the finger from [CENTER], so it stays on
/// screen. The camera moves twice as far.
const WORLD_PAN_MAX: (f32, f32) = (200.0, 120.0);

/// A location on the world map (`_worldmap_symbol`), in map points.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldSymbol {
    pub x: f32,
    pub y: f32,
    /// Selectable (`_savedat + 0x8a78 + i·4` is 2).
    pub open: bool,
    /// The locations its roads lead to, as the table lists them (the
    /// other end may not list this one).
    pub links: Vec<usize>,
}

/// The world map while it's up (song-summoner-re.md, World map).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldMap {
    pub work: u32,
    pub symbols: Vec<WorldSymbol>,
    /// Where the player stands.
    pub current: usize,
    /// The location the game has highlighted (`+0x38`): a tap on it
    /// confirms it, a tap on any other highlights that one instead.
    pub highlighted: usize,
    /// The camera's centre, in map points (`+0x28`/`+0x2a`).
    pub camera: (f32, f32),
    pub zoom: f32,
    /// Phase 3 with the camera still: the map takes touches.
    pub ready: bool,
}

impl WorldMap {
    /// Map points per screen point, as `Worldmap_Check_SymbolTouch`
    /// works it out.
    fn scale(&self) -> f32 {
        (1.0 - self.zoom) + (1.0 - self.zoom) + 1.0
    }

    /// A screen point in map points, the way `Check_SymbolTouch` does it.
    pub fn to_map(&self, (x, y): (f32, f32)) -> (f32, f32) {
        let s = self.scale();
        (
            self.camera.0 + s * -CENTER.0 + s * x,
            self.camera.1 + s * -CENTER.1 + s * y,
        )
    }

    /// Where location `i` is on screen: the inverse of [Self::to_map].
    pub fn to_screen(&self, i: usize) -> (f32, f32) {
        let s = self.scale();
        let symbol = &self.symbols[i];
        (
            CENTER.0 + (symbol.x - self.camera.0) / s,
            CENTER.1 + (symbol.y - self.camera.1) / s,
        )
    }

    /// The location a tap at `point` hits: `Check_SymbolTouch`, the first
    /// open location whose rect holds it, edges included.
    pub fn symbol_at(&self, point: (f32, f32)) -> Option<usize> {
        let (x, y) = self.to_map(point);
        self.symbols.iter().position(|s| {
            let (left, top) = (s.x - WORLD_HIT_W, s.y - WORLD_HIT_H);
            s.open
                && x >= left
                && x <= left + 2.0 * WORLD_HIT_W
                && y >= top
                && y <= top + 2.0 * WORLD_HIT_H
        })
    }

    /// All of location `i`'s rect is on screen, so a tap can reach it.
    pub fn on_screen(&self, i: usize) -> bool {
        let s = self.scale();
        let (x, y) = self.to_screen(i);
        let (w, h) = (WORLD_HIT_W / s, WORLD_HIT_H / s);
        x - w >= 0.0 && x + w <= 2.0 * CENTER.0 && y - h >= 0.0 && y + h <= 2.0 * CENTER.1
    }

    /// Where the roads from `i` lead, either way.
    fn roads(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let out = self.symbols[i].links.iter().copied();
        let back = (0..self.symbols.len()).filter(move |&j| self.symbols[j].links.contains(&i));
        out.chain(back).filter(|&j| j < self.symbols.len())
    }

    /// The location to move to from `from` in direction `dir` (screen
    /// axes, y down). First the next open locations along the roads (past
    /// closed ones), the one most in line; failing that, the nearest open
    /// location within 60° of `dir`.
    pub fn neighbour(&self, from: usize, dir: (f32, f32)) -> Option<usize> {
        let origin = &self.symbols[from];
        // How far along `dir` location j is (the cosine) and its distance.
        let aim = |j: usize| {
            let (dx, dy) = (self.symbols[j].x - origin.x, self.symbols[j].y - origin.y);
            let dist = (dx * dx + dy * dy).sqrt();
            let cos = if dist > 0.0 { (dx * dir.0 + dy * dir.1) / dist } else { -1.0 };
            (cos, dist)
        };
        let in_cone = |&j: &usize| aim(j).0 >= 0.5;

        // Along the roads, stopping at each open location.
        let mut stops = Vec::new();
        let mut seen = vec![false; self.symbols.len()];
        seen[from] = true;
        let mut queue = VecDeque::from([from]);
        while let Some(i) = queue.pop_front() {
            for j in self.roads(i) {
                if std::mem::replace(&mut seen[j], true) {
                    continue;
                }
                if self.symbols[j].open {
                    stops.push(j);
                } else {
                    queue.push_back(j);
                }
            }
        }
        let by_road = stops
            .into_iter()
            .filter(in_cone)
            .max_by(|&a, &b| aim(a).0.total_cmp(&aim(b).0));
        if by_road.is_some() {
            return by_road;
        }

        // Nearest that way, favouring those more in line.
        (0..self.symbols.len())
            .filter(|&j| j != from && self.symbols[j].open)
            .filter(in_cone)
            .min_by(|&a, &b| {
                let score = |j| {
                    let (cos, dist) = aim(j);
                    dist / cos
                };
                score(a).total_cmp(&score(b))
            })
    }

    /// The next (or previous) open location after `from`, in table order,
    /// wrapping round; `None` if there's no other.
    pub fn cycle(&self, from: usize, forward: bool) -> Option<usize> {
        let n = self.symbols.len();
        (1..n)
            .map(|k| if forward { (from + k) % n } else { (from + n - k) % n })
            .find(|&j| self.symbols[j].open)
    }
}

/// The location a command on the world map is after, and whether to
/// confirm it (tap it once it's highlighted) or just highlight it.
pub fn world_target(map: &WorldMap, role: Role) -> Option<(usize, bool)> {
    let from = map.highlighted;
    let dir = match role {
        Role::Up => (0.0, -1.0),
        Role::Down => (0.0, 1.0),
        Role::PrevSection => (-1.0, 0.0),
        Role::NextSection => (1.0, 0.0),
        Role::PrevTab => return map.cycle(from, false).map(|t| (t, false)),
        Role::NextTab => return map.cycle(from, true).map(|t| (t, false)),
        Role::Confirm => return Some((from, true)),
        Role::Back | Role::Info | Role::Skip | Role::ZoomOut | Role::ZoomIn => return None,
    };
    map.neighbour(from, dir).map(|t| (t, false))
}

/// What to do next about a world map target.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WorldStep {
    /// Nothing more (done, or it can't be reached).
    Done,
    /// Pan the map to bring it on screen, then look again.
    Pan(Gesture),
    /// Tap it; `again`: then tap it once more (to confirm it).
    Tap { at: (f32, f32), again: bool },
}

/// The next step toward highlighting (or confirming) location `target`.
/// `last_pan`: the camera when the last pan for it started, to notice a pan
/// that went nowhere (the map's edge).
pub fn world_step(
    map: &WorldMap,
    target: usize,
    confirm: bool,
    last_pan: Option<(f32, f32)>,
) -> WorldStep {
    let Some(symbol) = map.symbols.get(target) else {
        return WorldStep::Done;
    };
    // A tap on the highlighted location confirms it, so only when asked.
    if !symbol.open || (!confirm && map.highlighted == target) {
        return WorldStep::Done;
    }
    if map.on_screen(target) {
        let at = map.to_screen(target);
        // Only if the game's hit test agrees: it takes the first open
        // location whose rect holds the point, which could be another.
        if map.symbol_at(at) != Some(target) {
            return WorldStep::Done;
        }
        return WorldStep::Tap {
            at,
            again: confirm && map.highlighted != target,
        };
    }
    if last_pan == Some(map.camera) {
        return WorldStep::Done;
    }
    // Centre it: the camera moves twice the finger's movement, the other
    // way, and the game takes the movement in whole points.
    let (dx, dy) = ((map.camera.0 - symbol.x) / 2.0, (map.camera.1 - symbol.y) / 2.0);
    let dx = dx.trunc().clamp(-WORLD_PAN_MAX.0, WORLD_PAN_MAX.0);
    let dy = dy.trunc().clamp(-WORLD_PAN_MAX.1, WORLD_PAN_MAX.1);
    if dx == 0.0 && dy == 0.0 {
        return WorldStep::Done;
    }
    WorldStep::Pan(Gesture::Pan {
        x: CENTER.0,
        y: CENTER.1,
        dx,
        dy,
    })
}

/// An open `SysDrum2` (song-summoner-re.md §4), from its work struct:
/// `+0x290/+0x292` position, `+0x294/+0x296` size (`short`s), and the spin
/// (`+0x2a0` velocity, `+0x2a8` offset) that must be 0 before a tap turns
/// it again.
#[derive(Clone, Debug, PartialEq)]
pub struct Drum {
    pub work: u32,
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
    pub settled: bool,
}

impl Drum {
    /// `SysDrum2_Check`'s dividing line: a tap above it turns the drum one
    /// way, one more than a third of the height below it the other, and one
    /// in between picks the middle item.
    fn centre_line(&self) -> i32 {
        i32::from(self.y) - 8 + i32::from(self.h) / 2
    }

    fn third(&self) -> i32 {
        i32::from(self.h) / 3
    }

    fn mid_x(&self) -> f32 {
        f32::from(self.x) + f32::from(self.w) / 2.0
    }

    /// The band that picks, (x, y, width, height): where the cursor is.
    pub fn select_rect(&self) -> (f32, f32, f32, f32) {
        (
            f32::from(self.x),
            self.centre_line() as f32,
            f32::from(self.w),
            self.third() as f32,
        )
    }

    pub fn select_point(&self) -> (f32, f32) {
        let c = self.centre_line();
        (self.mid_x(), (c + self.third() / 2) as f32)
    }

    /// Halfway between the drum's top and the centre line. A tap here turns
    /// the next item (the one below the band) into it.
    pub fn up_point(&self) -> (f32, f32) {
        let top = i32::from(self.y);
        (self.mid_x(), ((top + self.centre_line()) / 2) as f32)
    }

    /// Halfway between the select band and the drum's bottom. A tap here
    /// turns the previous item (above the band) into it.
    pub fn down_point(&self) -> (f32, f32) {
        let below = self.centre_line() + self.third();
        let bottom = i32::from(self.y) + i32::from(self.h);
        (self.mid_x(), ((below + bottom) / 2) as f32)
    }
}

/// Where a command taps a drum, if anywhere. Tapping above or below the
/// band turns the drum away from that side (as in testing on the title
/// menu), so up taps below the band to bring in the item above it.
pub fn drum_command(drum: &Drum, role: Role) -> Option<(f32, f32)> {
    match role {
        Role::Up | Role::PrevSection => Some(drum.down_point()),
        Role::Down | Role::NextSection => Some(drum.up_point()),
        Role::Confirm => Some(drum.select_point()),
        _ => None,
    }
}

/// The Options screen's volume knob: its centre's height, and the range
/// its centre moves over (`Option_Check` sets the volume to (x - 254) / 200).
const OPTION_KNOB_Y: f32 = 64.0;
const OPTION_KNOB_MIN_X: f32 = 254.0;
const OPTION_KNOB_MAX_X: f32 = 454.0;
/// One left/right press moves the volume 10%.
const OPTION_VOLUME_STEP: f32 = 20.0;
/// The Options screen's slider, for the outline: the knob's whole travel.
const OPTION_SLIDER: (f32, f32, f32, f32) = (238.0, 48.0, 232.0, 32.0);
/// The BGM, SE and lock-orientation switches: where a tap (at the end of a
/// touch) flips each one, from `Option_Check`.
const OPTION_SWITCHES: [(f32, f32, f32, f32); 3] = [
    (368.0, 84.0, 88.0, 40.0),
    (368.0, 164.0, 88.0, 40.0),
    (368.0, 216.0, 88.0, 40.0),
];
/// Offsets in the Options work struct of the switches' knob sprite ids.
/// A knob sits at x = 368 when its switch is off, 412 when on, and a touch
/// that starts on it drags it instead of tapping the switch.
const OPTION_SWITCH_KNOBS: [u32; 3] = [0x58, 0x5c, 0x60];

/// The Options screen (`Option_Main`'s task, from the pause menu), from
/// its work struct: `+0` 2 once shown, `+4` at most 1 while `Option_Check`
/// waits for a touch (2 and 3 are a button's or switch's animation), and
/// sprite ids: the volume knob at `+0x48`, the switches' knobs, a button at
/// `+0x64` (shown when `+0x1c` is 1) and the back icon at `+0x68`.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub work: u32,
    /// The volume knob's centre x, 254 to 454.
    pub knob_x: f32,
    /// BGM, SE and lock orientation are on.
    pub switches: [bool; 3],
    pub button: Option<(f32, f32, f32, f32)>,
    pub back: Option<(f32, f32, f32, f32)>,
    /// Takes a touch now.
    pub ready: bool,
}

impl Options {
    /// The rows the focus moves over, top to bottom: the volume slider,
    /// the three switches, then the button and back icon if shown.
    pub fn rows(&self) -> Vec<(f32, f32, f32, f32)> {
        let mut rows = vec![OPTION_SLIDER];
        rows.extend(OPTION_SWITCHES);
        rows.extend(self.button);
        rows.extend(self.back);
        rows
    }

    /// A tap that flips switch `i`: on the half of it its knob isn't on.
    pub fn switch_tap(&self, i: usize) -> (f32, f32) {
        let (x, y, w, h) = OPTION_SWITCHES[i];
        let x = if self.switches[i] {
            x + w / 4.0
        } else {
            x + w * 3.0 / 4.0
        };
        (x, y + h / 2.0)
    }
}

/// One command on the Options screen with row `focus` focused: the new
/// focus, and the gesture to make, if any.
pub fn options_command(options: &Options, focus: usize, role: Role) -> (usize, Option<Gesture>) {
    let rows = options.rows();
    let last = rows.len() - 1;
    let focus = focus.min(last);
    match (focus, role) {
        (_, Role::Up) => (focus.saturating_sub(1), None),
        (_, Role::Down) => ((focus + 1).min(last), None),
        // Left/right drag the volume knob a step.
        (0, Role::PrevSection | Role::NextSection) => {
            let step = if role == Role::NextSection {
                OPTION_VOLUME_STEP
            } else {
                -OPTION_VOLUME_STEP
            };
            let x = options.knob_x;
            let to_x = (x + step).clamp(OPTION_KNOB_MIN_X, OPTION_KNOB_MAX_X);
            let slide = Gesture::Slide {
                x,
                y: OPTION_KNOB_Y,
                to_x,
            };
            (focus, (to_x != x).then_some(slide))
        }
        // Confirm flips a switch; left turns it off and right on, like
        // sliding its knob.
        (1..=3, Role::Confirm | Role::PrevSection | Role::NextSection) => {
            let i = focus - 1;
            let on = options.switches[i];
            let wanted = match role {
                Role::PrevSection => false,
                Role::NextSection => true,
                _ => !on,
            };
            let tap = options.switch_tap(i);
            (focus, (wanted != on).then_some(Gesture::Tap(tap.0, tap.1)))
        }
        (4.., Role::Confirm) => {
            let (x, y) = rect_center(rows[focus]);
            (focus, Some(Gesture::Tap(x, y)))
        }
        (_, Role::Back) => (
            focus,
            options.back.map(rect_center).map(|(x, y)| Gesture::Tap(x, y)),
        ),
        _ => (focus, None),
    }
}

/// The Help screen's list: rows show between these heights (a title bar
/// above, the tab bar below).
const HELP_LIST_TOP: f32 = 32.0;
const HELP_LIST_BOTTOM: f32 = 256.0;
/// How far a row may stick out of the list and still count as shown, and
/// how far past the edge a scroll aims, since the glide is approximate.
const HELP_SLACK: f32 = 3.0;
/// Where a scroll drag starts: mid-list, off the tabs and the side edges.
const HELP_DRAG_FROM: (f32, f32) = (240.0, 144.0);
/// `Help2_Main` adds a drag's last move to the scroll position and keeps
/// adding it, 10% less each frame: ten times the move in all.
const HELP_GLIDE: f32 = 10.0;
/// An item page scrolls half its 272-point view per up/down press.
const HELP_PAGE_STEP: f32 = 136.0;
/// Tabs are 80 points wide along the bottom; the sixth is Exit.
const HELP_TAB_W: f32 = 80.0;
const HELP_TAB_Y: f32 = 288.0;
const HELP_TABS: u32 = 5;
const HELP_EXIT_TAB: u32 = 5;
/// On an item page: the close icon (x > 400, y < 72), and the edges that go
/// to the previous (x < 48) and next (x > 432) item.
const HELP_CLOSE: (f32, f32) = (440.0, 36.0);
const HELP_PREV_ITEM: (f32, f32) = (24.0, 160.0);
const HELP_NEXT_ITEM: (f32, f32) = (456.0, 160.0);

/// The Help screen (`Help2_Main`'s task: the pause menu's Help, and
/// more), from its work struct: `+0` state (3 the list, 5 an item's page),
/// `+0x10` the tab, `+0x14` the item count, `+0x18` the item last touched
/// or opened (-1 for none), `+0x24` the list's scroll speed, and the rows'
/// sprite ids at `+0x80 + 4i`.
#[derive(Clone, Debug, PartialEq)]
pub struct Help {
    pub work: u32,
    /// Showing an item's page rather than the list.
    pub page: bool,
    pub tab: u32,
    /// The items, of which `rows` are shown (all of them on the list; the
    /// page hides them).
    pub count: usize,
    pub rows: Vec<(f32, f32, f32, f32)>,
    pub selected: Option<usize>,
    /// The list isn't gliding.
    pub settled: bool,
    /// Takes a touch now (not opening, closing or turning a page).
    pub ready: bool,
}

impl Help {
    /// Identifies the list or page (and tab) for the focus, so it starts
    /// afresh on each.
    pub fn id(&self) -> u32 {
        if self.page {
            self.work + 0x10
        } else {
            self.work + self.tab
        }
    }

    pub fn row_visible(&self, i: usize) -> bool {
        let (_, y, _, h) = self.rows[i];
        self.settled && y >= HELP_LIST_TOP - HELP_SLACK && y + h <= HELP_LIST_BOTTOM + HELP_SLACK
    }

    /// A drag gliding the list far enough to show row `i`, if it's off
    /// screen and the list is still.
    pub fn scroll_toward(&self, i: usize) -> Option<Gesture> {
        if i >= self.rows.len() || !self.settled || self.row_visible(i) {
            return None;
        }
        let (_, y, _, h) = self.rows[i];
        // How far the rows must move up (negative: down).
        let need = if y < HELP_LIST_TOP {
            y - HELP_LIST_TOP - HELP_SLACK
        } else {
            y + h - HELP_LIST_BOTTOM + HELP_SLACK
        };
        let (x, y) = HELP_DRAG_FROM;
        // A finger moving up scrolls the rows up.
        Some(Gesture::Drag {
            x,
            y,
            dy: -need / HELP_GLIDE,
        })
    }

    fn tab_tap(tab: u32) -> Gesture {
        Gesture::Tap(tab as f32 * HELP_TAB_W + HELP_TAB_W / 2.0, HELP_TAB_Y)
    }
}

/// One command on the Help screen with row `focus` focused (on the list):
/// the new focus, and a gesture to make, or `None` for the gesture if the
/// command has to wait (for a scroll).
pub fn help_command(help: &Help, focus: usize, role: Role) -> (usize, Option<Option<Gesture>>) {
    let prev = matches!(role, Role::PrevSection | Role::PrevTab);
    let next = matches!(role, Role::NextSection | Role::NextTab);
    if help.page {
        let tap = |(x, y): (f32, f32)| Some(Some(Gesture::Tap(x, y)));
        let selected = help.selected.unwrap_or(0);
        let scroll = |dy: f32| {
            let (x, y) = HELP_DRAG_FROM;
            Some(Some(Gesture::Drag { x, y, dy }))
        };
        return match role {
            // Finger down shows the text above.
            Role::Up => (focus, scroll(HELP_PAGE_STEP / HELP_GLIDE)),
            Role::Down => (focus, scroll(-HELP_PAGE_STEP / HELP_GLIDE)),
            _ if prev && selected > 0 => (focus, tap(HELP_PREV_ITEM)),
            _ if next && selected + 1 < help.count => (focus, tap(HELP_NEXT_ITEM)),
            Role::Back => (focus, tap(HELP_CLOSE)),
            _ => (focus, Some(None)),
        };
    }
    let last = help.rows.len().saturating_sub(1);
    match role {
        Role::Up => (focus.saturating_sub(1), Some(None)),
        Role::Down => ((focus + 1).min(last), Some(None)),
        _ if prev && help.tab > 0 => (focus, Some(Some(Help::tab_tap(help.tab - 1)))),
        _ if next && help.tab + 1 < HELP_TABS => {
            (focus, Some(Some(Help::tab_tap(help.tab + 1))))
        }
        Role::Confirm if help.rows.is_empty() => (focus, Some(None)),
        Role::Confirm if help.row_visible(focus) => {
            let (x, y) = rect_center(help.rows[focus]);
            (focus, Some(Some(Gesture::Tap(x, y))))
        }
        Role::Confirm => (focus, None),
        Role::Back => (focus, Some(Some(Help::tab_tap(HELP_EXIT_TAB)))),
        _ => (focus, Some(None)),
    }
}

/// Something on screen the controller drives.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    /// The Help screen, which also covers everything under it.
    Help(Help),
    /// The Options screen, which takes over from everything under it.
    Options(Options),
    Buttons(Menu),
    Dialog(Dialog),
    Drum(Drum),
    /// A screen's own buttons ([SCENE_BUTTONS]). `work` identifies the
    /// group: the scene's work address plus the group's index.
    Scene { work: u32, set: ButtonSet },
    /// The card list, identified by `_cardlist_work`'s address.
    Cards { work: u32, list: CardList },
    List(ListMenu),
    /// The world map, which highlights its own locations.
    World(WorldMap),
}

impl Widget {
    pub fn work(&self) -> u32 {
        match self {
            Widget::World(map) => map.work,
            Widget::Help(help) => help.id(),
            Widget::Options(options) => options.work,
            Widget::Buttons(menu) => menu.work,
            Widget::Dialog(dialog) => dialog.work,
            Widget::Scene { work, .. } | Widget::Cards { work, .. } => *work,
            Widget::List(list) => list.work,
            Widget::Drum(drum) => drum.work,
        }
    }
}

pub fn first_enabled(enabled: &[bool]) -> usize {
    enabled.iter().position(|&e| e).unwrap_or(0)
}

/// The game's functions and data we use, looked up by symbol once.
#[derive(Copy, Clone)]
struct Game {
    began: GuestFunction,
    moved: GuestFunction,
    ended: GuestFunction,
    /// Compared with task slots' main functions, without the Thumb bit.
    button_menu_main: u32,
    dialog_main: u32,
    drum2_main: u32,
    list_menu_main: u32,
    /// 0 if this copy of the game lacks them.
    option_main: u32,
    help_main: u32,
    /// The world map's task main, its location table and the save data
    /// (open locations, where the player is, the zoom); 0 if missing.
    worldmap_main: u32,
    worldmap_symbol: u32,
    savedat: u32,
    /// `_skip_able` (an odd address, 0xa718d): a byte set while a
    /// cutscene shows its SKIP button (`ScriptSkip_Enable`/`_Disable`).
    /// 0 if missing.
    skip_able: u32,
    /// Battle: the scene's task main, the map grid (`_tacticsmap_work`),
    /// the zoom (`_prim_scale`) and the touch struct; 0 if missing.
    tactics_main: u32,
    tacticsmap_work: u32,
    prim_scale: u32,
    touch_work: u32,
    /// Battle's target lists: move select's reachable tiles (a pointer to
    /// `(short x, short y)` pairs, and their count), and attack select's
    /// work (unit targets and area tiles); 0 if missing.
    move_targets: u32,
    move_target_count: u32,
    attack_select: u32,
    /// Unit select's work (`+0x0` state, 1 = taking touches; `+0x24` the
    /// MENU sprite), the tutorial's (`+0x8` set while one shows) and the
    /// selected unit (`+0x4` → its data, position at `+0x100`); 0 if
    /// missing.
    unit_select: u32,
    tutorial_work: u32,
    g_select: u32,
    /// The rest of battle (0 if missing): the command ring's work
    /// (`_tacticsunitmenu_work`), the skill list (`_status_work`), move
    /// select's work, attack info's target count, the deploy's work, the
    /// victory terms' work, the `SysAnim` table and the Listening Point
    /// scene's task main.
    unit_menu_work: u32,
    status_work: u32,
    move_select_work: u32,
    attack_info_count: u32,
    sortie_work: u32,
    terms_work: u32,
    anim_work: u32,
    listening_point_main: u32,
    /// `TacticsMapCursor_Set_Zoom(percent, frames, mode)`, which doesn't
    /// use the task work, so the host can call it; `None` if missing.
    set_zoom: Option<GuestFunction>,
    task_manage: u32,
    prim_work: u32,
    card_list_work: u32,
    /// Each [SCENE_BUTTONS] group's main function, resolved (0 if this
    /// copy of the game lacks it).
    scene_mains: [u32; SCENE_GROUPS],
    /// Holds the offset of `MainView`'s `m_mode` ivar.
    m_mode_offset: ConstPtr<u32>,
}

/// What the battle handlers past unit select keep between frames
/// (song-summoner-re.md, "Battle controller plan"). Reset whenever the
/// phase or the handler's own screen changes.
#[derive(Default)]
struct BattleInput {
    /// The Tactics work and phase last seen, and the handler's own screen
    /// in it (the ring's state, the deploy's), to start afresh on a change.
    phase: Option<(u32, u32)>,
    screen: Option<u32>,
    /// The cursor: locked to a list (move and attack select, placing), or
    /// free (the deploy's map view).
    cursor: Option<Tile>,
    /// Let go on the cursor's tile once the finger is on it.
    act: bool,
    /// Back in attack select and placing: bring the finger up where that
    /// does nothing, then tap where nothing is picked.
    back_tap: bool,
    /// The camera when the last pan started ([battle::pan_toward]).
    last_pan: Option<(f32, f32)>,
    /// A zoom to apply once the finger is up (percent).
    zoom: Option<i32>,
    /// The ring's angle last seen, and how many idle frames it's been
    /// still.
    ring_angle: Option<(i32, u32)>,
    /// The ring item last tapped to bring it round.
    ring_target: Option<usize>,
    /// The skill panel's row, and its back under way (the finger has
    /// slid off and come up; a tap on the map side closes it).
    skill_row: usize,
    skill_back: bool,
    /// The deploy sort panel's focused row.
    sort_row: usize,
    /// Taps still to make, one per idle frame (attack info's "previous
    /// target" is the next one, all the way round).
    taps: VecDeque<(f32, f32)>,
    /// The camera last frame, to act only while it's still.
    origin: Option<(f32, f32)>,
    /// Why presses were last waiting, to log each reason once.
    waiting: Option<String>,
}

#[derive(Default)]
pub struct State {
    taps: TapQueue,
    /// Controller commands waiting for the next frame, where the menus
    /// they act on are read.
    commands: VecDeque<Role>,
    /// The controller was used more recently than touch: show its focus.
    pad_mode: bool,
    /// The menu with the focus (its work address) and the focused button.
    focus: Option<(u32, usize)>,
    /// Where the controller is in the card list.
    card_focus: Option<CardFocus>,
    /// The world map location being brought on screen and tapped: the
    /// map's work address, the location, and whether to confirm it. Kept
    /// as a location, not a command, because a pan resets the game's
    /// highlight, which the command was relative to.
    world_target: Option<(u32, usize, bool)>,
    /// The camera when the last pan toward it started.
    world_pan: Option<(f32, f32)>,
    /// Frames since the last scroll drag, so a drag's glide shows up in the
    /// list's speed before the next is judged.
    since_drag: u32,
    /// Menus open last frame (work addresses), to spot a new one.
    known_menus: Vec<u32>,
    /// Running tasks' main functions last frame, and the outline last
    /// frame (game rect, window rect and shape), both only for logging
    /// changes.
    logged_tasks: Vec<u32>,
    logged_outline: Option<Option<((f32, f32, f32, f32), FocusMarker)>>,
    /// The battle state last logged, and whether the tile overlay is up
    /// (debug builds only; see [battle_debug]).
    logged_battle: Option<BattleLog>,
    battle_overlay: bool,
    /// The lit tiles and target lists last logged (phase, lit tiles, each
    /// list's name and tiles).
    logged_targets: Option<(u32, Vec<(Tile, i32)>, Vec<(String, Vec<Tile>)>)>,
    /// Battle, unit select: touchHLE's tile cursor (with the Tactics work it
    /// belongs to), the game's cursor last seen (to follow it when the game
    /// moves it), the camera last frame (to act only while it's still), the
    /// camera when the last pan started, a confirm waiting for the cursor's
    /// tile to be holdable, and our own pan or hold under way (whose
    /// release moves the game's cursor, which mustn't move ours).
    battle_cursor: Option<(u32, Tile)>,
    battle_seen: Option<Tile>,
    battle_origin: Option<(f32, f32)>,
    battle_pan: Option<(f32, f32)>,
    battle_confirm: bool,
    battle_touching: bool,
    /// Whether the confirm button is down now (battle's confirm holds a
    /// finger for as long as it is).
    confirm_down: bool,
    /// Why presses were last waiting in unit select (its state, camera
    /// still, no tap under way), to log each reason once.
    battle_waiting: Option<(u32, bool, bool)>,
    /// The held finger of the battle screens past unit select.
    finger: Finger,
    /// Those screens' own state.
    battle_input: BattleInput,
    /// The button a dialog that appears starts on, when not the first
    /// enabled one (the deploy's party-full dialog: No).
    dialog_default: Option<usize>,
    /// `None` until looked up; `Some(None)` if this copy lacks a symbol.
    game: Option<Option<Game>>,
    /// Two `CGPoint`s of guest memory for the touch calls' arguments.
    points: Option<MutPtr<CGPoint>>,
    /// Frames run, for timing in debug logs.
    frame: u64,
}

fn lookup(env: &Environment) -> Option<Game> {
    let symbols = &env.bins.first()?.exported_symbols;
    let get = |name: &str| {
        let addr = symbols.get(name).copied();
        if addr.is_none() {
            log!("input: {} not found, no controller input for the game", name);
        }
        addr
    };
    // The symbol table lacks the Thumb flag for the game's C++ functions,
    // but they are all Thumb (song-summoner-re.md §2).
    let thumb = |addr: u32| GuestFunction::from_addr_and_thumb_flag(addr & !1, true);
    let optional = |name: &str| symbols.get(name).copied().unwrap_or(0);
    Some(Game {
        began: thumb(get("__Z17SysTouch_Began_F1P7CGPointS0_ii")?),
        moved: thumb(get("__Z17SysTouch_Moved_F1P7CGPointS0_i")?),
        ended: thumb(get("__Z17SysTouch_Ended_F1P7CGPointi")?),
        button_menu_main: get("__Z18SysButtonMenu_Mainv")? & !1,
        dialog_main: get("__ZL10dialogmainv")? & !1,
        drum2_main: get("__ZL13drummenumain2v")? & !1,
        list_menu_main: get("__ZL12Menumenumainv")? & !1,
        option_main: symbols.get("__Z11Option_Mainv").map_or(0, |a| a & !1),
        help_main: symbols.get("__Z10Help2_Mainv").map_or(0, |a| a & !1),
        worldmap_main: symbols.get("__Z13Worldmap_Mainv").map_or(0, |a| a & !1),
        worldmap_symbol: symbols.get("_worldmap_symbol").copied().unwrap_or(0),
        savedat: symbols.get("_savedat").copied().unwrap_or(0),
        skip_able: symbols.get("_skip_able").copied().unwrap_or(0),
        tactics_main: symbols.get("__Z12Tactics_Mainv").map_or(0, |a| a & !1),
        tacticsmap_work: symbols.get("_tacticsmap_work").copied().unwrap_or(0),
        prim_scale: symbols.get("_prim_scale").copied().unwrap_or(0),
        touch_work: symbols.get("_touch_work").copied().unwrap_or(0),
        move_targets: symbols
            .get("_tactics_move_select_target_list")
            .copied()
            .unwrap_or(0),
        move_target_count: symbols
            .get("_tactics_move_select_target_list_cnt")
            .copied()
            .unwrap_or(0),
        attack_select: symbols.get("_tac_attack_select").copied().unwrap_or(0),
        unit_select: symbols.get("_tac_unit_select").copied().unwrap_or(0),
        tutorial_work: symbols.get("_tacticstutorial_work").copied().unwrap_or(0),
        g_select: symbols.get("_g_select").copied().unwrap_or(0),
        unit_menu_work: optional("_tacticsunitmenu_work"),
        status_work: optional("_status_work"),
        move_select_work: optional("_tacticsmoveselect_work"),
        attack_info_count: optional("_tactics_attack_info_target_list_cnt"),
        sortie_work: optional("_sortieselect_work"),
        terms_work: optional("_tacticsmapterms_work"),
        anim_work: optional("_anim_work"),
        listening_point_main: optional("__Z19ListeningPoint_Mainv") & !1,
        set_zoom: symbols
            .get("__Z25TacticsMapCursor_Set_Zoomiii")
            .map(|&a| thumb(a)),
        task_manage: get("_task_manage")?,
        prim_work: get("_prim_work")?,
        card_list_work: get("_cardlist_work")?,
        scene_mains: std::array::from_fn(|i| {
            symbols.get(SCENE_BUTTONS[i].main).map_or(0, |a| a & !1)
        }),
        m_mode_offset: Ptr::from_bits(get("_OBJC_IVAR_$_MainView.m_mode")?),
    })
}

fn game(env: &mut Environment) -> Option<Game> {
    if let Some(game) = env.framework_state.song_summoner.game_input.game {
        return game;
    }
    let game = lookup(env);
    env.framework_state.song_summoner.game_input.game = Some(game);
    game
}

fn read_u32(env: &Environment, addr: u32) -> u32 {
    env.mem.read(ConstPtr::<u32>::from_bits(addr))
}

fn read_i16(env: &Environment, addr: u32) -> i16 {
    env.mem.read(ConstPtr::<i16>::from_bits(addr))
}

fn read_f32(env: &Environment, addr: u32) -> f32 {
    env.mem.read(ConstPtr::<f32>::from_bits(addr))
}

fn read_drum(env: &Environment, work: u32) -> Option<Drum> {
    // drummenumain2 handles touches in states 2 and 3 (a signed byte).
    let state = env.mem.read(ConstPtr::<i8>::from_bits(work));
    if state != 2 && state != 3 {
        return None;
    }
    Some(Drum {
        work,
        x: read_i16(env, work + 0x290),
        y: read_i16(env, work + 0x292),
        w: read_i16(env, work + 0x294),
        h: read_i16(env, work + 0x296),
        // SysDrum2_CheckCursordisp's test.
        settled: read_f32(env, work + 0x2a0) == 0.0 && read_f32(env, work + 0x2a8).abs() < 1e-5,
    })
}

fn read_dialog(env: &Environment, work: u32) -> Option<Dialog> {
    // SysDialog_Check takes touches in states above 2; 0x63 and up are a
    // pressed button's animation and closing.
    let state = read_u32(env, work) as i32;
    if state <= 2 || state >= 0x63 {
        return None;
    }
    let count = read_u32(env, work + 0x1c);
    if count > 8 {
        return None;
    }
    let mut rects = Vec::new();
    let mut enabled = Vec::new();
    for i in 0..count {
        let entry = work + 0x28 + i * 0x14;
        rects.push((
            f32::from(read_i16(env, entry)),
            f32::from(read_i16(env, entry + 2)),
            f32::from(read_i16(env, entry + 4)),
            f32::from(read_i16(env, entry + 6)),
        ));
        enabled.push(read_u32(env, entry + 8) != 0);
    }
    Some(Dialog {
        work,
        rects,
        enabled,
        outside_cancels: read_u32(env, work + 0xf0) == 1,
    })
}

/// The name of the game function at `addr`, for logs.
fn symbol_name(env: &Environment, addr: u32) -> String {
    env.bins
        .first()
        .and_then(|bin| {
            bin.exported_symbols
                .iter()
                .find(|(_, &a)| a & !1 == addr)
                .map(|(name, _)| name.clone())
        })
        .unwrap_or_else(|| format!("{addr:#x}"))
}

/// Log the running tasks whenever they change, so a menu touchHLE doesn't
/// know yet can be identified from a log.
fn log_tasks(env: &mut Environment, game: Game) {
    let mains: Vec<u32> = (0..TASK_SLOTS)
        .map(|slot| game.task_manage + slot * TASK_SLOT_SIZE)
        .filter(|&base| read_u32(env, base) == TASK_RUNNING)
        .map(|base| read_u32(env, base + 0x10) & !1)
        .collect();
    if mains == env.framework_state.song_summoner.game_input.logged_tasks {
        return;
    }
    let names: Vec<String> = mains.iter().map(|&m| symbol_name(env, m)).collect();
    log!("input: tasks {:?}", names);
    env.framework_state.song_summoner.game_input.logged_tasks = mains;
}

/// A shown sprite's rectangle, or `None` if the sprite is unused or hidden.
fn read_prim_rect(env: &Environment, game: Game, id: u32) -> Option<(f32, f32, f32, f32)> {
    // Ids are small table indices; anything else means a misread.
    if id >= 0x1000 {
        return None;
    }
    let entry = game.prim_work + id * PRIM_SIZE;
    if read_u32(env, entry) == 0 || read_u32(env, entry + 8) == 0 {
        return None;
    }
    Some(prim_rect(
        read_f32(env, entry + 0xc),
        read_f32(env, entry + 0x10),
        read_f32(env, entry + 0x14),
        read_f32(env, entry + 0x18),
        read_u32(env, entry + 0x38) != 0,
    ))
}

/// The running scene's own buttons: the first [SCENE_BUTTONS] group of it
/// with a button shown, among the groups over the card list or the rest.
fn scene_buttons(env: &Environment, game: Game, over_cards: bool) -> Option<(u32, ButtonSet)> {
    for slot in 0..TASK_SLOTS {
        let base = game.task_manage + slot * TASK_SLOT_SIZE;
        if read_u32(env, base) != TASK_RUNNING {
            continue;
        }
        let main = read_u32(env, base + 0x10) & !1;
        let work = read_u32(env, base + 0x18);
        if work < 0x1000 {
            continue;
        }
        for (i, group) in SCENE_BUTTONS.iter().enumerate() {
            if game.scene_mains[i] == 0
                || game.scene_mains[i] != main
                || group.over_cards != over_cards
            {
                continue;
            }
            let sprite = |offset: u32| read_prim_rect(env, game, read_u32(env, work + offset));
            // The shown buttons, and which of the group's slots each is.
            let (slots, rects): (Vec<usize>, Vec<_>) = group
                .buttons
                .iter()
                .enumerate()
                .filter_map(|(slot, &o)| sprite(o).map(|rect| (slot, rect)))
                .unzip();
            if rects.is_empty() {
                continue;
            }
            let selected = group
                .selected
                .and_then(|offset| selected_position(&slots, read_u32(env, work + offset) as i32));
            let set = ButtonSet {
                enabled: vec![true; rects.len()],
                rects,
                back: group.back.and_then(sprite).map(rect_center),
                two_tap: group.selected.is_some(),
                selected,
            };
            return Some((work + i as u32, set));
        }
    }
    None
}

fn read_i8(env: &Environment, addr: u32) -> i8 {
    env.mem.read(ConstPtr::<i8>::from_bits(addr))
}

fn read_list(env: &Environment, work: u32) -> Option<ListMenu> {
    let count = read_i8(env, work + 4);
    if count <= 0 {
        return None;
    }
    let count = count as u32;
    let width = f32::from(read_i16(env, work + 0xd0c));
    let mut rows = Vec::new();
    let mut enabled = Vec::new();
    for i in 0..count {
        let item = work + i * 0x34;
        rows.push((
            f32::from(read_i16(env, item + 0x30)),
            f32::from(read_i16(env, item + 0x32)),
            width,
            LIST_ROW_H,
        ));
        enabled.push(env.mem.read(ConstPtr::<u8>::from_bits(item + 0x14)) != 0);
    }
    Some(ListMenu {
        work,
        rows,
        enabled,
        first: read_f32(env, work + 0xd18),
        shown: read_i8(env, work + 5).max(1) as usize,
        settled: read_f32(env, work + 0xd1c) == 0.0,
        cursor: (read_i8(env, work + 2).max(0) as usize).min(count as usize - 1),
        span: Some((f32::from(read_i16(env, work + 0xd08)), width)).filter(|&(_, w)| w > 0.0),
    })
}

/// The card list, if it's on screen and taking touches: its key lock
/// (`+0x4c`) off, and at least one of its bottom icons shown.
fn card_list(env: &Environment, game: Game) -> Option<CardList> {
    let work = game.card_list_work;
    if read_u32(env, work + 0x4c) == 1 {
        return None;
    }
    let icon = |offset: u32| {
        read_prim_rect(env, game, read_u32(env, work + offset)).filter(|&r| is_card_icon(r))
    };
    let mut icons: Vec<_> = CARD_LIST_ICONS.iter().filter_map(|&o| icon(o)).collect();
    if icons.is_empty() {
        return None;
    }
    icons.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(CardList {
        centre_x: read_u32(env, work + 0x34) as i32 as f32,
        icons,
        status_icon: icon(CARD_LIST_STATUS_ICON),
        exit_icon: icon(CARD_LIST_EXIT_ICON),
        status_panel: read_u32(env, work + 0x58) == 1,
    })
}

fn read_options(env: &Environment, game: Game, work: u32) -> Options {
    let sprite = |offset: u32| read_prim_rect(env, game, read_u32(env, work + offset));
    let knob_x = sprite(0x48).map_or(OPTION_KNOB_MIN_X, |r| rect_center(r).0);
    Options {
        work,
        knob_x,
        // Off at x = 368, on at 412.
        switches: OPTION_SWITCH_KNOBS.map(|o| sprite(o).is_some_and(|r| r.0 >= 390.0)),
        button: if read_u32(env, work + 0x1c) == 1 {
            sprite(0x64)
        } else {
            None
        },
        back: sprite(0x68),
        ready: read_u32(env, work) == 2 && read_u32(env, work + 4) <= 1,
    }
}

fn read_help(env: &Environment, game: Game, work: u32) -> Help {
    let state = read_u32(env, work);
    // Help2_Main's item pages hold 32 or so entries at most.
    let count = read_u32(env, work + 0x14).min(64);
    let rows = (0..count)
        .map_while(|i| read_prim_rect(env, game, read_u32(env, work + 0x80 + i * 4)))
        .collect();
    let selected = read_u32(env, work + 0x18) as i32;
    Help {
        work,
        // 4 opens a page, 5 shows it, 6 closes it, 7-12 turn pages.
        page: (4..=12).contains(&state),
        tab: read_u32(env, work + 0x10),
        count: count as usize,
        rows,
        selected: usize::try_from(selected).ok(),
        settled: read_f32(env, work + 0x24).abs() < 0.05,
        ready: state == 3 || state == 5,
    }
}

/// The world map, if its task is running and in phase 3 (taking touches)
/// or 4 (the player walking). Other phases (loading, the location menu)
/// aren't handled here.
fn world_map(env: &Environment, game: Game) -> Option<WorldMap> {
    if game.worldmap_main == 0 || game.worldmap_symbol == 0 || game.savedat == 0 {
        return None;
    }
    let work = (0..TASK_SLOTS)
        .map(|slot| game.task_manage + slot * TASK_SLOT_SIZE)
        .find(|&base| {
            read_u32(env, base) == TASK_RUNNING
                && read_u32(env, base + 0x10) & !1 == game.worldmap_main
        })
        .map(|base| read_u32(env, base + 0x18))
        .filter(|&work| work >= 0x1000)?;
    let phase = read_u32(env, work);
    if phase != 3 && phase != 4 {
        return None;
    }
    let symbols: Vec<WorldSymbol> = (0..WORLD_SYMBOLS)
        .map(|i| {
            let entry = game.worldmap_symbol + i * WORLD_SYMBOL_SIZE;
            // Type 0 is an unused entry.
            let used = read_u32(env, entry + 4) != 0;
            WorldSymbol {
                x: f32::from(read_i16(env, entry + 8)),
                y: f32::from(read_i16(env, entry + 0xa)),
                open: used && read_u32(env, game.savedat + 0x8a78 + i * 4) == 2,
                // Up to four (short link, short 0) pairs, -1 for none.
                links: (0..4)
                    .map(|k| read_i16(env, entry + 0xc + k * 4))
                    .filter_map(|l| usize::try_from(l).ok())
                    .filter(|&l| l < WORLD_SYMBOLS as usize)
                    .collect(),
            }
        })
        .collect();
    let index = |value: u32| usize::try_from(value as i32).ok().filter(|&i| i < symbols.len());
    let current = index(read_u32(env, game.savedat + 0x87f0)).unwrap_or(0);
    let highlighted = index(read_u32(env, work + 0x38)).unwrap_or(current);
    Some(WorldMap {
        work,
        current,
        highlighted,
        camera: (
            f32::from(read_i16(env, work + 0x28)),
            f32::from(read_i16(env, work + 0x2a)),
        ),
        zoom: read_f32(env, game.savedat + 0x87e8),
        // While +0x2c is set the camera is gliding and Worldmap_Main skips
        // the phase.
        ready: phase == 3 && read_u32(env, work + 0x2c) == 0,
        symbols,
    })
}

/// The battle scene's state, as far as step 1 of the battle plan needs it
/// (song-summoner-re.md §4, "Battle controller plan").
struct Battle {
    /// The Tactics task's work struct.
    work: u32,
    /// `Tactics_Main`'s phase and sub-phase, and `Tactics_CtrlTest`'s
    /// gesture (1 flick, 2 hold, 3 pinch).
    phase: u32,
    sub: u32,
    gesture: u32,
    camera: Camera,
    grid: Grid,
    /// The game's own map cursor (`TacticsMapCursor_Get_Position`).
    cursor: Tile,
    /// The zoom in percent (`TacticsMapCursor_Get_Zoom`), and whether it's
    /// animating (`+0x5e8`; `Tactics_Main` skips the phase meanwhile).
    zoom: i32,
    zooming: bool,
}

/// What [battle_debug] logs, to log only changes.
#[derive(Clone, PartialEq)]
struct BattleLog {
    phase: u32,
    sub: u32,
    gesture: u32,
    cursor: Tile,
    zoom: i32,
    origin: (i32, i32),
    size: (i32, i32),
    highlighted: usize,
    /// A finger's position, if one is down.
    touch: Option<(i32, i32)>,
}

/// The work struct of the running task whose main function is `main`.
fn task_work(env: &Environment, game: Game, main: u32) -> Option<u32> {
    (0..TASK_SLOTS)
        .map(|slot| game.task_manage + slot * TASK_SLOT_SIZE)
        .find(|&base| {
            read_u32(env, base) == TASK_RUNNING && read_u32(env, base + 0x10) & !1 == main
        })
        .map(|base| read_u32(env, base + 0x18))
        .filter(|&work| work >= 0x1000)
}

fn read_battle(env: &Environment, game: Game) -> Option<Battle> {
    if game.tactics_main == 0 || game.tacticsmap_work == 0 || game.prim_scale == 0 {
        return None;
    }
    let work = task_work(env, game, game.tactics_main)?;
    let phase = read_u32(env, work);
    // Phases 0-4 load the map; before then the grid may be the last one's.
    if phase < 5 {
        return None;
    }
    let map = game.tacticsmap_work;
    let (w, h) = (i32::from(read_i16(env, map + 8)), i32::from(read_i16(env, map + 0xa)));
    let (units_ptr, highlight_ptr) = (read_u32(env, map + 0x30), read_u32(env, map + 0x34));
    // Maps are a few dozen tiles across; anything else means a misread.
    let sizes_ok = (1..=64).contains(&w) && (1..=64).contains(&h);
    if !sizes_ok || units_ptr < 0x1000 || highlight_ptr < 0x1000 {
        return None;
    }
    let scale = read_f32(env, game.prim_scale);
    if !(0.25..=4.0).contains(&scale) {
        return None;
    }
    let tiles = (w * h) as u32;
    let read_ints = |ptr: u32| -> Vec<i32> {
        (0..tiles)
            .map(|i| read_u32(env, ptr + i * 4) as i32)
            .collect()
    };
    Some(Battle {
        work,
        phase,
        sub: read_u32(env, work + 4),
        gesture: read_u32(env, work + 0x20),
        camera: Camera {
            origin: (
                f32::from(read_i16(env, work + 0x14)),
                f32::from(read_i16(env, work + 0x16)),
            ),
            scale,
        },
        grid: Grid {
            w,
            h,
            units: read_ints(units_ptr),
            highlight: read_ints(highlight_ptr),
        },
        cursor: (
            i32::from(read_i16(env, map + 0x5d8)),
            i32::from(read_i16(env, map + 0x5da)),
        ),
        zoom: read_u32(env, map + 0x5f0) as i32,
        zooming: read_u32(env, map + 0x5e8) != 0,
    })
}

/// Unit select's own state (`_tac_unit_select + 0x0`) while it takes
/// touches: `TacticsUnitSelect_MainCtrl`'s switch runs `Tactics_CtrlTest`
/// for 1. (0 is its one-off setup; 2 animates MENU; 3 shows another
/// team's unit's status; 7 and 8 select a unit.)
const UNIT_SELECT_TAKING_TOUCHES: u32 = 1;
/// Unit select's state while another team's unit's status is open.
const UNIT_SELECT_STATUS: u32 = 3;

/// A tutorial is showing over the battle; `Tactics_Main` skips the phase
/// while it does, and it takes a tap anywhere.
fn battle_tutorial(env: &Environment, game: Game) -> bool {
    game.tutorial_work != 0 && read_u32(env, game.tutorial_work + 8) != 0
}

/// The selected unit's tile (`_g_select`: `+0x4` → its data, position at
/// `+0x100`).
fn selected_unit_tile(env: &Environment, game: Game) -> Option<Tile> {
    if game.g_select == 0 {
        return None;
    }
    let data = read_u32(env, game.g_select + 4);
    (data >= 0x1000).then(|| {
        (
            i32::from(read_i16(env, data + 0x100)),
            i32::from(read_i16(env, data + 0x102)),
        )
    })
}

/// Unit select (phase 19): the D-pad moves touchHLE's tile cursor along
/// the grid, panning the map to keep it holdable; confirm holds a finger
/// on its tile (which acts on its unit at once, see battle::holdable), the
/// shoulders tap the curved arrows and Start taps MENU. Returns the
/// outline (the cursor's tile).
///
/// Everything waits while the game isn't taking touches (unit select's own
/// state isn't 1, an animation) or the camera is moving, so taps land on
/// what the player saw.
fn unit_select(
    env: &mut Environment,
    game: Game,
    battle: &Battle,
) -> Option<(f32, f32, f32, f32)> {
    use battle::UnitSelectIntent as Intent;

    // State 3: another team's unit's status is open (after tapping it).
    // A still tap at x < 160 (the stats panel) flips its page, one further
    // right closes it; the finger must go down and up on the same side.
    if read_u32(env, game.unit_select) == UNIT_SELECT_STATUS {
        // Here confirm closes it too: an enemy's status is only a glance.
        let state = &mut env.framework_state.song_summoner.game_input;
        if state.taps.is_idle() {
            if let Some(role) = state.commands.pop_front() {
                let role = if role == Role::Confirm { Role::Back } else { role };
                if let Some((x, y)) = battle::status_command(role) {
                    state.taps.tap(x, y);
                }
            }
        }
        return None;
    }
    let select_state = read_u32(env, game.unit_select);
    let taking_touches = select_state == UNIT_SELECT_TAKING_TOUCHES;
    let menu_rect = read_prim_rect(env, game, read_u32(env, game.unit_select + 0x24));
    let selected = selected_unit_tile(env, game).filter(|&t| battle.grid.contains(t));
    let state = &mut env.framework_state.song_summoner.game_input;

    let origin = battle.camera.origin;
    let still = state.battle_origin == Some(origin);
    state.battle_origin = Some(origin);

    // Follow the game's cursor when the game moves it (the arrows, a
    // selection), but not while our own pan or hold moves it (a held
    // finger drags it along; a release puts it back on the selected unit).
    let game_cursor = Some(battle.cursor).filter(|&t| battle.grid.contains(t));
    if state.battle_touching {
        if state.taps.is_idle() {
            state.battle_touching = false;
            state.battle_seen = game_cursor;
        }
    } else if game_cursor.is_some() && game_cursor != state.battle_seen {
        state.battle_seen = game_cursor;
        state.battle_cursor = game_cursor.map(|t| (battle.work, t));
    }
    let mut cursor = match state.battle_cursor {
        Some((work, tile)) if work == battle.work && battle.grid.contains(tile) => tile,
        _ => game_cursor
            .or(selected)
            .unwrap_or((battle.grid.w / 2, battle.grid.h / 2)),
    };

    // Say why presses are waiting, once per reason, so a wrong guess about
    // the game shows up in the log rather than as a dead controller.
    let waiting = (!state.commands.is_empty()).then_some((
        select_state,
        still,
        state.taps.is_idle(),
    ));
    if waiting != state.battle_waiting {
        if let Some((select_state, still, idle)) = waiting {
            if !(taking_touches && still && idle) {
                log!(
                    "input: unit select, presses waiting (state {}, camera still {}, no tap under way {})",
                    select_state,
                    still,
                    idle
                );
            }
        }
        state.battle_waiting = waiting;
    }

    if taking_touches && still && state.taps.is_idle() {
        while let Some(role) = state.commands.pop_front() {
            match battle::unit_select_intent(role) {
                Intent::Move(dir) => {
                    cursor = battle::grid_step(&battle.grid, cursor, dir);
                    state.battle_confirm = false;
                    state.battle_pan = None;
                }
                Intent::Confirm => state.battle_confirm = true,
                Intent::PrevUnit => {
                    let (x, y) = battle::PREV_UNIT_ARROW;
                    state.taps.tap(x, y);
                    break;
                }
                Intent::NextUnit => {
                    let (x, y) = battle::NEXT_UNIT_ARROW;
                    state.taps.tap(x, y);
                    break;
                }
                Intent::Menu => {
                    if let Some(rect) = menu_rect {
                        let (x, y) = rect_center(rect);
                        state.taps.tap(x, y);
                    }
                    break;
                }
                Intent::None => {}
            }
        }
        // Bring the cursor's tile where a held finger reaches it, then
        // hold it if asked to. A hold rather than a tap: the game picks a
        // held finger's unit by tile, a tap's by sprite (battle::holdable).
        let centre = battle.camera.tile_centre(cursor);
        let pan = battle::pan_toward(&battle.camera, cursor, state.battle_pan);
        if state.taps.is_idle() {
            if battle::holdable(centre) {
                state.battle_pan = None;
                // Any tile, as a finger would: the game's cursor goes
                // there and the status panel shows its unit or hides.
                if std::mem::take(&mut state.battle_confirm) {
                    let (x, y) = battle.camera.hold_point(cursor);
                    log!(
                        "input: unit select, holding tile {:?} at {:?} (game cursor on {:?}, confirm still down {})",
                        cursor,
                        (x, y),
                        battle.cursor,
                        state.confirm_down
                    );
                    // The game's cursor follows the finger, and goes back to
                    // the selected unit after a hold on an empty tile; ours
                    // stays put.
                    state.battle_touching = true;
                    // Down while confirm is held (the game's cursor and
                    // status panel show the unit), at least long enough
                    // to be a hold; letting go acts.
                    let frames = battle::SELECT_HOLD_FRAMES;
                    state.taps.push(Gesture::Hold { x, y, frames });
                    state.taps.keep_down(state.confirm_down);
                }
            } else if let Some((dx, dy)) = pan {
                log!("input: unit select, panning by {:?} toward tile {:?}", (dx, dy), cursor);
                state.battle_pan = Some(origin);
                state.battle_touching = true;
                let (x, y) = battle::TAP_CENTRE;
                state.taps.push(Gesture::Pan { x, y, dx, dy });
            } else if battle::tappable(centre)
                && std::mem::take(&mut state.battle_confirm)
                && battle::confirm_picks(&battle.grid, cursor)
            {
                // At the map's bottom edge, too low for a held finger: a
                // tap is the only way left (a unit drawn over this tile
                // may take it).
                log!("input: unit select, tapping tile {:?} at {:?} (too low to hold)", cursor, centre);
                state.taps.tap(centre.0, centre.1);
            } else {
                // It can't be brought any closer (the map's edge).
                state.battle_confirm = false;
            }
        }
    }
    state.battle_cursor = Some((battle.work, cursor));
    Some(battle.camera.tile_rect(cursor))
}

/// Draw every map tile's outline over battle (debug builds), to check
/// `battle.rs`'s projection against the game's tiles. Off since the
/// projection was confirmed in play; the battle log stays.
const DRAW_TILE_GRID: bool = false;

/// Step 1 of the battle plan, in debug builds only: log the battle's state
/// when it changes (with the tile touchHLE thinks a finger is on, next to
/// the game's own cursor, to check the projection), and outline every tile
/// on screen where touchHLE thinks it is.
fn battle_debug(env: &mut Environment, game: Game, main_view: id) {
    if !cfg!(debug_assertions) {
        return;
    }
    let battle = read_battle(env, game);
    let touch = (game.touch_work != 0 && read_u32(env, game.touch_work + 0x10) != 0).then(|| {
        (
            read_f32(env, game.touch_work + 0x24),
            read_f32(env, game.touch_work + 0x28),
        )
    });

    let Some(battle) = battle else {
        let state = &mut env.framework_state.song_summoner.game_input;
        state.logged_battle = None;
        state.logged_targets = None;
        clear_battle_overlay(env);
        return;
    };

    let entry = BattleLog {
        phase: battle.phase,
        sub: battle.sub,
        gesture: battle.gesture,
        cursor: battle.cursor,
        zoom: battle.zoom,
        origin: (battle.camera.origin.0 as i32, battle.camera.origin.1 as i32),
        size: (battle.grid.w, battle.grid.h),
        highlighted: battle.grid.highlighted(),
        touch: touch.map(|(x, y)| (x as i32, y as i32)),
    };
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.logged_battle.as_ref() != Some(&entry) {
        log!(
            "battle: phase {} ({}) sub {:#x}, gesture {}, map {}x{} ({} tiles lit), camera {:?}, zoom {}% (scale {}), game cursor {:?} (unit {:?})",
            battle.phase,
            battle::phase_name(battle.phase),
            battle.sub,
            battle.gesture,
            battle.grid.w,
            battle.grid.h,
            entry.highlighted,
            battle.camera.origin,
            battle.zoom,
            battle.camera.scale,
            battle.cursor,
            battle.grid.unit_at(battle.cursor)
        );
        if let Some(point) = touch {
            // Which tile a tap there would pick, and which the game's
            // held-finger cursor should be on: compare the latter with the
            // game cursor above while holding.
            let tap = battle.camera.tile_at(point);
            let hold = battle.camera.hold_tile_at(point);
            log!(
                "battle: touch at {:?}: tap tile {:?} (unit {:?}), hold tile {:?}",
                point,
                tap,
                battle.grid.unit_at(tap),
                hold
            );
        }
        state.logged_battle = Some(entry);
    }
    log_battle_targets(env, game, &battle);

    if !DRAW_TILE_GRID {
        clear_battle_overlay(env);
        return;
    }
    // The overlay, converted from game points to the portrait screen
    // points the window wants, as show_focus does for the outline. The
    // conversion is affine, so three points give it.
    let lines = battle::grid_lines(&battle.camera, &battle.grid);
    let mut convert = |x: f32, y: f32| -> (f32, f32) {
        let point = CGPoint { x, y };
        let p: CGPoint = msg![env; main_view convertPoint:point toView:nil];
        (p.x, p.y)
    };
    let o = convert(0.0, 0.0);
    let ex = convert(1.0, 0.0);
    let ey = convert(0.0, 1.0);
    let to_screen = |(x, y): (f32, f32)| {
        (
            o.0 + x * (ex.0 - o.0) + y * (ey.0 - o.0),
            o.1 + x * (ex.1 - o.1) + y * (ey.1 - o.1),
        )
    };
    let lines = lines
        .into_iter()
        .map(|(a, b)| (to_screen(a), to_screen(b)))
        .collect();
    if let Some(window) = env.window.as_mut() {
        window.set_debug_lines(lines);
    }
    env.framework_state.song_summoner.game_input.battle_overlay = true;
}

/// `count` `(short x, short y)` tiles at `ptr`, or `None` if either looks
/// wrong (a list that isn't there right now reads as a null pointer).
fn read_tile_list(env: &Environment, ptr: u32, count: u32, max: u32) -> Option<Vec<Tile>> {
    if ptr < 0x1000 || count > max {
        return None;
    }
    Some(
        (0..count)
            .map(|i| {
                let entry = ptr + i * 4;
                (
                    i32::from(read_i16(env, entry)),
                    i32::from(read_i16(env, entry + 2)),
                )
            })
            .collect(),
    )
}

/// The game's own lists of the tiles a tap can pick in this phase, named
/// for the log: move select's reachable tiles, or attack select's unit
/// targets and area tiles (only one is in use; `+0x72c` says which).
fn battle_targets(env: &Environment, game: Game, phase: u32) -> Vec<(String, Vec<Tile>)> {
    let mut lists = Vec::new();
    if phase == 22 && game.move_targets != 0 && game.move_target_count != 0 {
        let ptr = read_u32(env, game.move_targets);
        let count = read_u32(env, game.move_target_count);
        if let Some(tiles) = read_tile_list(env, ptr, count, 4096) {
            lists.push(("move targets".to_string(), tiles));
        }
    }
    if phase == 24 && game.attack_select != 0 {
        let work = game.attack_select;
        let mode = read_u32(env, work + 0x72c);
        // Room for 225 entries each, up to the next field.
        let targets = read_tile_list(env, work + 0x8, read_u32(env, work + 0x38c), 225);
        let area = read_tile_list(env, work + 0x394, read_u32(env, work + 0x718), 225);
        for (name, tiles) in [("attack targets", targets), ("area tiles", area)] {
            if let Some(tiles) = tiles.filter(|t| !t.is_empty()) {
                lists.push((format!("{name} (mode {mode})"), tiles));
            }
        }
    }
    lists
}

/// Log the lit tiles and the game's target lists when any changes, and how
/// each list differs from the lit tiles (the controller's lock will use the
/// lists; in move select they should be the same tiles).
fn log_battle_targets(env: &mut Environment, game: Game, battle: &Battle) {
    let lit = battle.grid.highlighted_tiles();
    let lists = battle_targets(env, game, battle.phase);
    if lit.is_empty() && lists.is_empty() {
        return;
    }
    let entry = (battle.phase, lit, lists);
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.logged_targets.as_ref() == Some(&entry) {
        return;
    }
    let (_, lit, lists) = &entry;
    log!("battle: {} lit tiles (tile, value): {:?}", lit.len(), lit);
    let lit_tiles: Vec<Tile> = lit.iter().map(|&(tile, _)| tile).collect();
    for (name, tiles) in lists {
        let (only_lit, only_listed) = battle::tile_list_diff(&lit_tiles, tiles);
        log!(
            "battle: {} {}: {:?}; same tiles as lit: {} (lit only {:?}, listed only {:?})",
            tiles.len(),
            name,
            tiles,
            only_lit.is_empty() && only_listed.is_empty(),
            only_lit,
            only_listed
        );
    }
    state.logged_targets = Some(entry);
}

fn clear_battle_overlay(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.game_input;
    if std::mem::take(&mut state.battle_overlay) {
        if let Some(window) = env.window.as_mut() {
            window.set_debug_lines(Vec::new());
        }
    }
}

/// Open menus that take touches, in task table order.
fn open_widgets(env: &Environment, game: Game) -> Vec<Widget> {
    let mut widgets = Vec::new();
    for slot in 0..TASK_SLOTS {
        let base = game.task_manage + slot * TASK_SLOT_SIZE;
        if read_u32(env, base) != TASK_RUNNING {
            continue;
        }
        let main = read_u32(env, base + 0x10) & !1;
        let work = read_u32(env, base + 0x18);
        if work < 0x1000 {
            continue;
        }
        // The Options screen covers the pause menu, which stays open (and
        // taking touches) under it, so it's the only thing that counts.
        if game.option_main != 0 && main == game.option_main {
            return vec![Widget::Options(read_options(env, game, work))];
        }
        // The same goes for Help.
        if game.help_main != 0 && main == game.help_main {
            return vec![Widget::Help(read_help(env, game, work))];
        }
        if main == game.drum2_main {
            widgets.extend(read_drum(env, work).map(Widget::Drum));
            continue;
        }
        if main == game.dialog_main {
            widgets.extend(read_dialog(env, work).map(Widget::Dialog));
            continue;
        }
        if main == game.list_menu_main {
            widgets.extend(read_list(env, work).map(Widget::List));
            continue;
        }
        if main != game.button_menu_main || read_u32(env, work) != MENU_ACCEPTING {
            continue;
        }
        let count = read_u32(env, work + 0xc);
        let enabled_ptr = read_u32(env, work + 0x2c);
        // The game's menus have a handful of buttons; anything else means
        // we've misread something.
        if count == 0 || count > 16 || enabled_ptr < 0x1000 {
            continue;
        }
        let enabled = (0..count)
            .map(|i| read_u32(env, enabled_ptr + i * 4) != 0)
            .collect();
        widgets.push(Widget::Buttons(Menu {
            work,
            x: read_i16(env, work + 0x18),
            y: read_i16(env, work + 0x1a),
            w: read_i16(env, work + 0x1c),
            h: read_i16(env, work + 0x1e),
            enabled,
            outrange_cancel: read_u32(env, work + 0x20) == 1,
            cancel_button: read_u32(env, work + 0x28) != u32::MAX,
        }));
    }
    // The card list and a screen's own buttons only count when nothing is
    // open over them: panels over the card list, then the card list, then
    // the rest of the screen's buttons.
    let scene = |over_cards| {
        scene_buttons(env, game, over_cards).map(|(work, set)| Widget::Scene { work, set })
    };
    if widgets.is_empty() {
        widgets.extend(scene(true));
    }
    if widgets.is_empty() {
        let work = game.card_list_work;
        widgets.extend(card_list(env, game).map(|list| Widget::Cards { work, list }));
    }
    if widgets.is_empty() {
        widgets.extend(scene(false));
    }
    if widgets.is_empty() {
        widgets.extend(world_map(env, game).map(Widget::World));
    }
    widgets
}

/// A controller button that the picker didn't take.
pub fn handle_pad_button(env: &mut Environment, button: PadButton, pressed: bool) {
    let Some(role) = pad::role(button, env.options.confirm_button) else {
        return;
    };
    let state = &mut env.framework_state.song_summoner.game_input;
    // Battle's confirm holds a finger down for as long as it's held.
    if role == Role::Confirm {
        state.confirm_down = pressed;
        if !pressed {
            state.taps.keep_down(false);
        }
        if cfg!(debug_assertions) {
            log!(
                "input: confirm {} (frame {})",
                if pressed { "down" } else { "up" },
                state.frame
            );
        }
    }
    if !pressed {
        return;
    }
    state.pad_mode = true;
    if state.commands.len() < MAX_QUEUED_COMMANDS {
        state.commands.push_back(role);
    }
}

/// A finger touched the screen: hide the controller's focus.
pub fn touch_used(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.game_input;
    // A real finger takes over from the held virtual one.
    state.finger.clear();
    if state.pad_mode {
        state.pad_mode = false;
        if let Some(window) = env.window.as_mut() {
            window.set_focus_marker(None);
        }
    }
}

/// A battle phase's sub-phase while it runs (`Tactics_Set_PhaseSub`: 0x66
/// starts a phase, 0x65 runs it).
const PHASE_RUNNING: u32 = 0x65;
/// The command ring's screen (`_tacticsunitmenu_work + 0x0`,
/// `TacticsUnitMenu_Main`'s switch at 0x1de6c): the ring itself, the skill
/// panel, the item window (2, a `SysMenu` list), the unit's status.
const RING_TOP: u32 = 0;
const RING_SKILL: u32 = 1;
const RING_STATUS: u32 = 3;
/// The ring's items (sprite ids at `+0x54 + 4·i`).
const RING_ITEMS: u32 = 7;
/// Idle frames the ring's angle (`+0x14`) must stay put for a turn to
/// have finished (before a confirm, or a turn after one that went astray).
const RING_STILL_FRAMES: u32 = 2;
/// The most items one tap turns the ring by: half of it, beyond which the
/// game turns the other way round.
const RING_MAX_STEPS: usize = 3;
/// The skill panel's sub-state (`_tacticsunitmenu_work + 0x4`,
/// `TacticsUnitMenu_CtrlSkill`'s switch at 0x1d6f8) while it takes
/// touches. 0 sets it up (and clears the touch), 2 is a picked skill, 3
/// closes it.
const SKILL_TAKING_TOUCHES: u32 = 1;
/// Move select's state (`_tacticsmoveselect_work + 0x0`, tested in
/// `TacticsMoveSelect_Main` at 0x21af8) while it takes touches
/// (`Tactics_CtrlTest`); 1 is a tapped tile's 5 frames before the walk.
const MOVE_SELECT_TAKING_TOUCHES: u32 = 0;
/// Attack select's state (`_tac_attack_select + 0x0`, tested in
/// `TacticsAttackSelect_Main` at 0x2fcee) while it takes touches; 1 is a
/// picked target's 5 frames.
const ATTACK_SELECT_TAKING_TOUCHES: u32 = 0;
/// The deploy's states (`_sortieselect_work + 0x0`,
/// `TacticsSortieSelect_Main`'s switch at 0x37158): 3 the card list, 4
/// placing a unit, 5 after placing, 6 the common menu, 7 the sort panel, 8
/// the map view, 10 the victory terms, 13 the party-full dialog.
const SORTIE_PLACING: u32 = 4;
const SORTIE_SORT: u32 = 7;
const SORTIE_MAP_VIEW: u32 = 8;
const SORTIE_TERMS: u32 = 10;
const SORTIE_PARTY_FULL: u32 = 13;
/// The party-full dialog's "No": the focus starts there, so a fast presser
/// isn't locked into the battle.
const PARTY_FULL_NO: usize = 1;
/// The sort panel's sub-state (`+0x4`, tested at 0x37c94) while it takes
/// touches, and the map view's (switch at 0x37fca: 0 sets up, 1
/// `Tactics_CtrlTest`, 2 the back icon's animation, 3 a unit's status).
const SORT_TAKING_TOUCHES: u32 = 0;
const MAP_VIEW_TAKING_TOUCHES: u32 = 1;
const MAP_VIEW_STATUS: u32 = 3;
/// The map view's back icon (`tc_icon` at (456, 296); a tap at x ≥ 432,
/// y ≥ 272, `TacticsSortieMap_Ctrl_Release`).
const MAP_VIEW_BACK: (f32, f32) = (456.0, 296.0);
/// A tap left of the sort panel (x < 60) cancels it, if its back icon
/// can't be found.
const SORT_CANCEL: (f32, f32) = (30.0, 160.0);
/// The victory terms' state (`_tacticsmapterms_work + 0x0`,
/// `TacticsMapTerms_Main`'s switch at 0x1a1a2): 0–3 load, animate and slide
/// in (3 ends by clearing the touch, so earlier taps are lost), 4 any
/// finger-up closes them, 5 and 6 slide out, 7 done.
const TERMS_TAKING_TOUCHES: u32 = 4;
const TERMS_DONE: u32 = 7;
/// Frames a trigger's zoom animates over, and `Set_Zoom`'s mode: the
/// pinch's (0 goes in even steps; 1 would halve the gap each frame).
const ZOOM_FRAMES: i32 = 6;
const ZOOM_MODE: i32 = 0;

/// What [battle_commands] did.
enum Battled {
    /// Handled (or waiting for the game): outline this.
    Done(Option<FocusMarker>),
    /// Not battle's to handle: the menus (and the centre tap) have it.
    Menus,
}

/// Carry out queued controller commands against battle's map or the open
/// menu, if any, and return what to outline (in the game's 480×320 points):
/// a map tile as a diamond, anything else with brackets.
fn run_commands(env: &mut Environment, game: Game) -> Option<FocusMarker> {
    let widgets = open_widgets(env, game);
    env.framework_state.song_summoner.game_input.dialog_default = None;

    match read_battle(env, game) {
        Some(battle) => {
            track_battle_phase(env, game, &battle);
            // The victory terms cover everything, menus included.
            if let Some(marker) = terms_commands(env, game, &battle) {
                return marker;
            }
            // Widgets over the map (dialogs, MENU's buttons, the item
            // list, Options) and tutorials win. The deploy's card list is
            // one of its own screens, not over the map.
            let over_map = widgets.iter().any(|w| !matches!(w, Widget::Cards { .. }));
            if battle_tutorial(env, game) {
                stand_down(env, "a tutorial");
            } else if over_map {
                stand_down(env, "a menu");
            } else if let Battled::Done(marker) =
                battle_commands(env, game, &battle, !widgets.is_empty())
            {
                // No menu is open, so the next one is new.
                let state = &mut env.framework_state.song_summoner.game_input;
                state.known_menus.clear();
                state.focus = None;
                return marker;
            }
            if battle.phase == 8
                && game.sortie_work != 0
                && read_u32(env, game.sortie_work) == SORTIE_PARTY_FULL
            {
                env.framework_state.song_summoner.game_input.dialog_default = Some(PARTY_FULL_NO);
            }
        }
        None => {
            let state = &mut env.framework_state.song_summoner.game_input;
            if state.battle_input.phase.is_some() {
                state.battle_input = BattleInput::default();
                state.finger.clear();
            }
        }
    }
    menu_commands(env, game, widgets).map(|r| (r, FocusShape::Brackets))
}

/// Start the battle handlers afresh when the phase changes: a pending
/// confirm or pan, the held finger and the cursors belong to the last one.
fn track_battle_phase(env: &mut Environment, game: Game, battle: &Battle) {
    let key = (battle.work, battle.phase);
    if env.framework_state.song_summoner.game_input.battle_input.phase == Some(key) {
        return;
    }
    let selected = selected_unit_tile(env, game);
    let state = &mut env.framework_state.song_summoner.game_input;
    log!(
        "input: battle phase {} ({}), controller: {:?}",
        battle.phase,
        battle::phase_name(battle.phase),
        battle::phase_owner(battle.phase)
    );
    if state.finger.is_down() || !state.finger.is_idle() {
        log!("input: battle, dropping the held finger (the phase changed)");
    }
    state.finger.clear();
    state.battle_input = BattleInput {
        phase: Some(key),
        ..BattleInput::default()
    };
    state.battle_confirm = false;
    state.battle_pan = None;
    // Unit select starts on the selected unit: after a turn ends, that's
    // the next one, not where the cursor was. Our own hold (which opened
    // the ring) is over, so the game's cursor is followed again from here.
    state.battle_touching = false;
    state.battle_seen = Some(battle.cursor);
    state.battle_cursor = (battle.phase == 19).then(|| {
        (battle.work, battle::unit_select_start(&battle.grid, selected, battle.cursor))
    });
}

/// Start a handler's own screen afresh (the ring's state, the deploy's)
/// when it changes.
fn enter_screen(state: &mut State, screen: u32) {
    if state.battle_input.screen == Some(screen) {
        return;
    }
    if state.finger.is_down() || !state.finger.is_idle() {
        log!("input: battle, dropping the held finger (screen {} now)", screen);
        state.finger.clear();
    }
    state.battle_input = BattleInput {
        phase: state.battle_input.phase,
        screen: Some(screen),
        ..BattleInput::default()
    };
}

/// Something is over the map: the battle handlers stand down, and a held
/// finger goes (its lift would be what advances a tutorial).
fn stand_down(env: &mut Environment, what: &str) {
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.finger.is_down() || !state.finger.is_idle() {
        log!("input: battle, {} over the map: dropping the held finger", what);
        state.finger.clear();
    }
    state.battle_input.act = false;
}

/// Log, once per change, why queued presses are waiting in a battle
/// handler (`reason`: `None` when they aren't).
fn log_waiting(state: &mut State, what: &str, reason: Option<String>) {
    let reason = reason.filter(|_| !state.commands.is_empty());
    if reason != state.battle_input.waiting {
        if let Some(reason) = &reason {
            log!("input: {}, presses waiting ({})", what, reason);
        }
        state.battle_input.waiting = reason;
    }
}

fn is_zoom(role: Role) -> bool {
    matches!(role, Role::ZoomOut | Role::ZoomIn)
}

/// Take the triggers' presses out of the queue: the zoom they come to,
/// stepping from `pending` if a zoom is waiting already, else from the
/// map's; `pending` if there are none. Ignored while a zoom animates.
fn take_zoom(commands: &mut VecDeque<Role>, battle: &Battle, pending: Option<i32>) -> Option<i32> {
    let mut zoom = pending;
    commands.retain(|&role| {
        if !is_zoom(role) {
            return true;
        }
        if !battle.zooming {
            let from = zoom.unwrap_or(battle.zoom);
            if let Some(to) = battle::zoom_step(from, role == Role::ZoomIn) {
                zoom = Some(to);
            }
        }
        false
    });
    zoom
}

/// Zoom the map with the game's own `TacticsMapCursor_Set_Zoom`, animated
/// (song-summoner-re.md, Zoom). Not saved, unlike a pinch.
fn apply_zoom(env: &mut Environment, game: Game, percent: i32) {
    let Some(set_zoom) = game.set_zoom else {
        log!("input: zoom, TacticsMapCursor_Set_Zoom not found");
        return;
    };
    log!("input: zoom to {}%", percent);
    () = set_zoom.call_from_host(env, (percent, ZOOM_FRAMES, ZOOM_MODE));
}

/// Battle's own screens, by phase (battle::phase_owner). `widgets_open`:
/// something the menus handle is up (only the deploy's card list, since
/// anything over the map was dealt with before).
fn battle_commands(
    env: &mut Environment,
    game: Game,
    battle: &Battle,
    widgets_open: bool,
) -> Battled {
    use battle::Owner;
    let owner = battle::phase_owner(battle.phase);
    let state = &mut env.framework_state.song_summoner.game_input;
    // The triggers only zoom in unit, move and attack select.
    if !matches!(owner, Owner::UnitSelect | Owner::MoveSelect | Owner::AttackSelect) {
        state.commands.retain(|&role| !is_zoom(role));
    }
    match owner {
        Owner::UnitSelect if !widgets_open && game.unit_select != 0 => {
            if battle.sub != PHASE_RUNNING {
                return Battled::Done(None);
            }
            // A zoom goes before unit select sees the queue, once nothing
            // is under way.
            let zoom = take_zoom(&mut state.commands, battle, state.battle_input.zoom);
            state.battle_input.zoom = zoom;
            if battle.zooming {
                // Tactics_Main skips the phase while the zoom animates;
                // the tiles are on the move, so pans and holds wait.
                let outline = state
                    .battle_cursor
                    .map(|(_, tile)| (battle.camera.tile_rect(tile), FocusShape::Diamond));
                return Battled::Done(outline);
            }
            let idle = state.taps.is_idle();
            let taking = read_u32(env, game.unit_select) == UNIT_SELECT_TAKING_TOUCHES;
            if taking && idle {
                let state = &mut env.framework_state.song_summoner.game_input;
                if let Some(percent) = state.battle_input.zoom.take() {
                    apply_zoom(env, game, percent);
                    return Battled::Done(None);
                }
            }
            Battled::Done(unit_select(env, game, battle).map(|r| (r, FocusShape::Diamond)))
        }
        Owner::Ring if !widgets_open && game.unit_menu_work != 0 => {
            Battled::Done(ring_commands(env, game, battle))
        }
        Owner::MoveSelect if !widgets_open && game.move_select_work != 0 => {
            Battled::Done(locked_select(env, game, battle, Locked::Move))
        }
        Owner::AttackSelect if !widgets_open && game.attack_select != 0 => {
            Battled::Done(locked_select(env, game, battle, Locked::Attack))
        }
        Owner::AttackInfo if !widgets_open => Battled::Done(attack_info(env, game, battle)),
        Owner::Sortie if game.sortie_work != 0 => sortie_commands(env, game, battle),
        Owner::Swallow if !widgets_open => {
            if !state.commands.is_empty() {
                log!(
                    "input: battle phase {} ({}) takes no input, dropping presses",
                    battle.phase,
                    battle::phase_name(battle.phase)
                );
                state.commands.clear();
            }
            Battled::Done(None)
        }
        _ => Battled::Menus,
    }
}

/// A unit's full status (the ring's Status, the deploy map view's): north
/// or confirm flip the page, back closes it. One tap at a time.
fn status_commands(state: &mut State) {
    if state.taps.is_idle() {
        if let Some(role) = state.commands.pop_front() {
            if let Some((x, y)) = battle::status_command(role) {
                state.taps.tap(x, y);
            }
        }
    }
}

/// The command ring (phase 20) and the screens it opens.
fn ring_commands(env: &mut Environment, game: Game, battle: &Battle) -> Option<FocusMarker> {
    let screen = read_u32(env, game.unit_menu_work);
    let state = &mut env.framework_state.song_summoner.game_input;
    enter_screen(state, screen);
    match screen {
        RING_TOP => ring_top(env, game, battle),
        RING_SKILL => skill_panel(env, game, battle),
        RING_STATUS => {
            status_commands(state);
            None
        }
        // The item window's list is a widget, handled with the menus;
        // until it shows, presses wait.
        _ => {
            log_waiting(state, "ring", Some(format!("screen {}", screen)));
            None
        }
    }
}

/// The ring itself: left/right tap the neighbouring item (the game turns
/// the ring to it), confirm taps the front item, back taps outside the
/// ring. The game shows the front item, so there's no outline.
///
/// Turning is quick: the next left/right tap goes as soon as the item last
/// tapped has become the front one (the game then turns straight on to
/// the next), and presses queued the same way make one tap that many items
/// round (up to [RING_MAX_STEPS]). Confirm waits for the ring to stop, so
/// it never taps a moving item. The ring doesn't turn while a finger is
/// down, so a tap lands where the item was read.
fn ring_top(env: &mut Environment, game: Game, battle: &Battle) -> Option<FocusMarker> {
    let work = game.unit_menu_work;
    let angle = read_u32(env, work + 0x14) as i32;
    let front = read_u32(env, work + 0x18) as usize;
    // A flick's spin, still winding down.
    let spinning = read_u32(env, work + 0x24) != 0;
    // Indexed by item, as `+0x18` is. A hidden item (if any are) is
    // nowhere, so it's never anyone's neighbour.
    let centres: Vec<(f32, f32)> = (0..RING_ITEMS)
        .map(|i| {
            read_prim_rect(env, game, read_u32(env, work + 0x54 + 4 * i))
                .map_or((f32::NAN, f32::NAN), rect_center)
        })
        .collect();
    let shown = centres.iter().filter(|c| !c.0.is_nan()).count();
    let state = &mut env.framework_state.song_summoner.game_input;
    let idle = state.taps.is_idle();
    // Only idle frames count: the ring turns after a tap's finger-up.
    let still = match state.battle_input.ring_angle {
        Some((a, n)) if a == angle && idle => n + 1,
        _ => 0,
    };
    state.battle_input.ring_angle = Some((angle, still));
    let front_shown = centres.get(front).is_some_and(|c| !c.0.is_nan());
    let settled = still >= RING_STILL_FRAMES;
    // The item last tapped is at the front now, even if still turning.
    let arrived = state.battle_input.ring_target == Some(front);
    let can_turn = battle.sub == PHASE_RUNNING && front_shown && !spinning && idle;
    let turning = matches!(
        state.commands.front(),
        Some(Role::PrevSection | Role::NextSection)
    );
    let ready = can_turn && (settled || (turning && arrived));
    let reason = (!ready).then(|| {
        format!(
            "sub-phase {:#x}, {} items shown (front {} shown {}), still {} frames, last tapped {:?}, spinning {}, tap under way {}",
            battle.sub,
            shown,
            front,
            front_shown,
            still,
            state.battle_input.ring_target,
            spinning,
            !idle
        )
    });
    log_waiting(state, "ring", reason);
    if !ready {
        return None;
    }
    while let Some(role) = state.commands.pop_front() {
        let side = match role {
            Role::PrevSection => Some(battle::Side::Left),
            Role::NextSection => Some(battle::Side::Right),
            _ => None,
        };
        if let Some(side) = side {
            // This press and the same ones queued behind it.
            let mut steps = 1;
            while steps < RING_MAX_STEPS && state.commands.front() == Some(&role) {
                state.commands.pop_front();
                steps += 1;
            }
            let Some(item) = battle::ring_steps(&centres, front, side, steps) else {
                continue;
            };
            let (x, y) = centres[item];
            log!(
                "input: ring, tapping item {} at {:?} (front {}, {} steps, ring settled {})",
                item,
                (x, y),
                front,
                steps,
                settled
            );
            state.taps.tap(x, y);
            state.battle_input.ring_target = Some(item);
            state.battle_input.ring_angle = Some((angle, 0));
            break;
        }
        if !settled {
            // Confirm and back wait for the turn to finish.
            state.commands.push_front(role);
            break;
        }
        let Some((x, y)) = battle::ring_command(&centres, front, role) else {
            continue;
        };
        if role == Role::Confirm {
            log!("input: ring, tapping item {} at {:?} (the front)", front, (x, y));
        } else {
            log!("input: ring, back: tapping {:?}", (x, y));
        }
        state.taps.tap(x, y);
        state.battle_input.ring_target = None;
        state.battle_input.ring_angle = Some((angle, 0));
        break;
    }
    None
}

/// The ring's skill panel: a finger held on the focused row, so the
/// game's cursor and the skill's area show. Up/down slide it between the
/// rows, confirm lets go (a usable skill goes to attack select; otherwise
/// the finger goes down again), back slides off the list and lets go
/// (which does nothing), then taps the map side, which closes the panel.
fn skill_panel(env: &mut Environment, game: Game, battle: &Battle) -> Option<FocusMarker> {
    let sub = read_u32(env, game.unit_menu_work + 4);
    let count = if game.status_work != 0 {
        read_u32(env, game.status_work + 0x24).min(64) as usize
    } else {
        0
    };
    let rows = battle::skill_rows(count);
    let state = &mut env.framework_state.song_summoner.game_input;
    let idle = state.taps.is_idle() && state.finger.is_idle();
    let ready = battle.sub == PHASE_RUNNING && sub == SKILL_TAKING_TOUCHES && idle;
    let reason = (!ready)
        .then(|| format!("panel state {}, finger or tap under way {}", sub, !idle));
    log_waiting(state, "skill panel", reason);
    if !ready {
        return None;
    }
    let (close_x, close_y) = battle::SKILL_CLOSE;
    if state.battle_input.skill_back {
        if state.finger.finger().is_none() {
            state.battle_input.skill_back = false;
            log!("input: skill panel, back: tapping {:?}", battle::SKILL_CLOSE);
            state.taps.tap(close_x, close_y);
        }
        return None;
    }
    let mut row = state.battle_input.skill_row.min(rows.saturating_sub(1));
    if state.finger.finger().is_none() {
        // Back needs no finger: a still tap on the map side closes it.
        if state.commands.front() == Some(&Role::Back) {
            state.commands.pop_front();
            log!("input: skill panel, back: tapping {:?}", battle::SKILL_CLOSE);
            state.taps.tap(close_x, close_y);
        } else if rows > 0 {
            let (x, y) = battle::skill_row_point(row);
            // The game takes the row from the finger's height.
            debug_assert_eq!(battle::skill_row_at(y), Some(row));
            log!("input: skill panel, holding row {} of {} at {:?}", row, rows, (x, y));
            state.finger.press(x, y);
        } else {
            state.commands.retain(|&role| role == Role::Back);
        }
        return None;
    }
    if let Some(role) = state.commands.pop_front() {
        let to = match role {
            Role::Up => Some(row.saturating_sub(1)),
            Role::Down => Some((row + 1).min(rows.saturating_sub(1))),
            _ => None,
        };
        match (role, to) {
            (_, Some(to)) if to != row => {
                row = to;
                let (x, y) = battle::skill_row_point(row);
                log!("input: skill panel, sliding to row {} at {:?}", row, (x, y));
                state.finger.slide_to(x, y);
            }
            (Role::Confirm, _) => {
                log!("input: skill panel, letting go on row {}", row);
                state.finger.lift();
            }
            (Role::Back, _) => {
                // A finger that moved is a flick: letting go does nothing.
                log!("input: skill panel, back: sliding off the list and letting go");
                state.finger.slide_to(close_x, close_y);
                state.finger.lift();
                state.battle_input.skill_back = true;
            }
            _ => {}
        }
    }
    state.battle_input.skill_row = row;
    None
}

/// The screens where a held finger follows a cursor locked to the game's
/// list of tiles that act.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Locked {
    /// Move select (phase 22): the reachable tiles.
    Move,
    /// Attack select (phase 24): the targets, or an area skill's tiles.
    Attack,
    /// The deploy's placing (phase 8, state 4): the deploy tiles.
    Place,
}

impl Locked {
    fn name(self) -> &'static str {
        match self {
            Locked::Move => "move select",
            Locked::Attack => "attack select",
            Locked::Place => "placing",
        }
    }
}

/// Move select's reachable tiles (`*_tactics_move_select_target_list`,
/// including the unit's own tile), empty until they're ready.
fn move_targets(env: &Environment, game: Game) -> Vec<Tile> {
    if game.move_targets == 0 || game.move_target_count == 0 {
        return Vec::new();
    }
    let ptr = read_u32(env, game.move_targets);
    let count = read_u32(env, game.move_target_count);
    read_tile_list(env, ptr, count, 4096).unwrap_or_default()
}

/// Attack select's mode (`+0x72c`) and the list it uses.
fn attack_targets(env: &Environment, game: Game) -> (u32, Vec<Tile>) {
    let work = game.attack_select;
    let mode = read_u32(env, work + 0x72c);
    // Room for 225 entries each, up to the next field.
    let list = match battle::attack_list(mode) {
        Some(battle::AttackList::Targets) => {
            read_tile_list(env, work + 0x8, read_u32(env, work + 0x38c), 225)
        }
        Some(battle::AttackList::Area) => {
            read_tile_list(env, work + 0x394, read_u32(env, work + 0x718), 225)
        }
        None => None,
    };
    (mode, list.unwrap_or_default())
}

/// The deploy tiles (`_sortieselect_work`: `+0x28` count, `+0x2c` → 0x14-
/// byte entries, `short` x, y, the card on it at `+0xc`, −1 when free),
/// and the first free one.
fn deploy_tiles(env: &Environment, game: Game) -> (Vec<Tile>, Option<Tile>) {
    let work = game.sortie_work;
    let count = read_u32(env, work + 0x28);
    let ptr = read_u32(env, work + 0x2c);
    if ptr < 0x1000 || count > 64 {
        return (Vec::new(), None);
    }
    let mut tiles = Vec::new();
    let mut free = None;
    for i in 0..count {
        let entry = ptr + i * 0x14;
        let tile = (
            i32::from(read_i16(env, entry)),
            i32::from(read_i16(env, entry + 2)),
        );
        if free.is_none() && read_u32(env, entry + 0xc) as i32 == -1 {
            free = Some(tile);
        }
        tiles.push(tile);
    }
    (tiles, free)
}

/// Move select, attack select and placing: a finger held on the cursor's
/// tile so the game's own cursor and previews show (battle::follow). The
/// cursor starts on a tile in the game's list of tiles that act, and the
/// D-pad steps it along the grid anywhere on the map (battle::grid_step),
/// sliding the finger along; confirm lets go on it (on a tile that doesn't
/// act, nothing happens, as for a finger).
/// Back: in move select, letting go on the unit's own tile (the move is
/// cancelled); in attack select and placing, letting go where nothing
/// happens, then a quick tap on nothing (back to the ring, or the card
/// list). The shoulders cycle attack select's targets, and the triggers
/// zoom (the finger comes up first, and goes down again after). Returns
/// the cursor's tile to outline.
fn locked_select(
    env: &mut Environment,
    game: Game,
    battle: &Battle,
    kind: Locked,
) -> Option<FocusMarker> {
    // The game's side: whether it takes touches, the tiles where letting
    // go acts, the unit's own tile (where move select's cursor starts,
    // and back returns to), where the cursor starts, and attack select's
    // mode.
    let (taking, list, own, start, mode) = match kind {
        Locked::Move => {
            let own = selected_unit_tile(env, game);
            let taking = read_u32(env, game.move_select_work) == MOVE_SELECT_TAKING_TOUCHES;
            (taking, move_targets(env, game), own, own, None)
        }
        Locked::Attack => {
            let taking = read_u32(env, game.attack_select) == ATTACK_SELECT_TAKING_TOUCHES;
            let (mode, list) = attack_targets(env, game);
            let first = list.first().copied();
            (taking, list, None, first, Some(mode))
        }
        Locked::Place => {
            let (list, free) = deploy_tiles(env, game);
            let start = free.or(list.first().copied());
            (true, list, None, start, None)
        }
    };
    // Placing runs in the deploy phase, whose sub-phase isn't this.
    let taking = taking && (kind == Locked::Place || battle.sub == PHASE_RUNNING);
    let camera = battle.camera;
    let what = kind.name();

    let state = &mut env.framework_state.song_summoner.game_input;
    let still = state.battle_input.origin == Some(camera.origin);
    state.battle_input.origin = Some(camera.origin);
    if kind != Locked::Place {
        let zoom = take_zoom(&mut state.commands, battle, state.battle_input.zoom);
        state.battle_input.zoom = zoom;
    }
    let idle = state.taps.is_idle() && state.finger.is_idle();
    let ready = taking && still && !battle.zooming && !list.is_empty() && idle;
    let reason = (!ready).then(|| {
        format!(
            "taking touches {}, camera still {}, zooming {}, {} tiles, finger or tap under way {}",
            taking,
            still,
            battle.zooming,
            list.len(),
            !idle
        )
    });
    log_waiting(state, what, reason);

    let entering = state.battle_input.cursor.is_none();
    // The cursor starts on a listed tile, then goes anywhere on the map
    // (the user's choice, 2026-09-28).
    let mut cursor = state
        .battle_input
        .cursor
        .filter(|&t| battle.grid.contains(t))
        .or_else(|| start.and_then(|near| battle::locked_start(&list, near)));
    if entering && cursor.is_some() {
        log!(
            "input: {}, {} tiles{}: {:?}; cursor on {:?}",
            what,
            list.len(),
            mode.map_or(String::new(), |m| format!(" (mode {m})")),
            list,
            cursor
        );
    }
    if !ready {
        state.battle_input.cursor = cursor;
        return cursor.map(|c| (camera.tile_rect(c), FocusShape::Diamond));
    }

    while let Some(role) = state.commands.pop_front() {
        let dir = match role {
            Role::Up => Some(battle::Dir::Up),
            Role::Down => Some(battle::Dir::Down),
            Role::PrevSection => Some(battle::Dir::Left),
            Role::NextSection => Some(battle::Dir::Right),
            _ => None,
        };
        match (role, dir, cursor) {
            (_, Some(dir), Some(from)) => {
                cursor = Some(battle::grid_step(&battle.grid, from, dir));
            }
            (Role::PrevTab | Role::NextTab, _, _) if kind == Locked::Attack => {
                let at = cursor.and_then(|c| list.iter().position(|&t| t == c));
                cursor = match at {
                    Some(i) => Some(list[battle::cycle(list.len(), i, role == Role::NextTab)]),
                    None => list.first().copied(),
                };
            }
            (Role::Confirm, _, _) => {
                state.battle_input.act = true;
                break;
            }
            (Role::Back, _, _) => {
                if kind == Locked::Move {
                    // Letting go on the unit's own tile cancels the move.
                    cursor = own;
                    state.battle_input.act = true;
                } else {
                    state.battle_input.back_tap = true;
                }
                break;
            }
            _ => {}
        }
    }

    let input = &mut state.battle_input;
    let finger = state.finger.finger();
    let view = battle::FollowView {
        camera: &camera,
        grid: &battle.grid,
        accepted: &list,
        cursor,
        finger,
        want_down: !input.back_tap && input.zoom.is_none(),
        act: input.act,
        last_pan: input.last_pan,
    };
    let mut zoom_now = None;
    match battle::follow(&view) {
        battle::FollowAction::Wait => {
            // The finger is up as wanted: back's tap, or the zoom.
            if finger.is_none() && input.back_tap {
                input.back_tap = false;
                match battle::harmless_tap(&camera, &battle.grid, &list) {
                    Some((x, y)) => {
                        log!(
                            "input: {}, back: tapping {:?} (tile {:?})",
                            what,
                            (x, y),
                            camera.tile_at((x, y))
                        );
                        state.taps.tap(x, y);
                    }
                    None => {
                        log!("input: {}, back: nowhere to tap that picks nothing", what);
                    }
                }
            } else if finger.is_none() {
                zoom_now = input.zoom.take();
            }
        }
        battle::FollowAction::Press((x, y)) => {
            log!("input: {}, pressing on tile {:?} at {:?}", what, cursor, (x, y));
            input.last_pan = None;
            state.finger.press(x, y);
        }
        battle::FollowAction::Slide((x, y)) => {
            log!("input: {}, sliding to tile {:?} at {:?}", what, cursor, (x, y));
            state.finger.slide_to(x, y);
        }
        battle::FollowAction::Lift => {
            let at = finger.map(|p| camera.hold_tile_at(p));
            log!(
                "input: {}, letting go on tile {:?} (acting {}, cursor {:?})",
                what,
                at,
                input.act,
                cursor
            );
            input.act = false;
            state.finger.lift();
        }
        battle::FollowAction::LiftAt((x, y)) => {
            log!(
                "input: {}, letting go where nothing happens: tile {:?} at {:?}",
                what,
                camera.hold_tile_at((x, y)),
                (x, y)
            );
            state.finger.slide_to(x, y);
            state.finger.lift();
        }
        battle::FollowAction::Pan(dx, dy) => {
            log!("input: {}, panning by {:?} toward tile {:?}", what, (dx, dy), cursor);
            input.last_pan = Some(camera.origin);
            let (x, y) = battle::TAP_CENTRE;
            state.taps.push(Gesture::Pan { x, y, dx, dy });
        }
        battle::FollowAction::Tap((x, y)) => {
            log!(
                "input: {}, tapping tile {:?} at {:?} (too near the edge to hold)",
                what,
                cursor,
                (x, y)
            );
            input.act = false;
            state.taps.tap(x, y);
        }
        battle::FollowAction::Stuck => {
            if std::mem::take(&mut input.act) {
                log!("input: {}, can't reach tile {:?}", what, cursor);
            }
        }
    }
    input.cursor = cursor;
    if let Some(percent) = zoom_now {
        apply_zoom(env, game, percent);
    }
    cursor.map(|c| (camera.tile_rect(c), FocusShape::Diamond))
}

/// Attack info (phase 25): confirm attacks, back returns to attack
/// select, R goes to the next target and L all the way round to the
/// previous one. One tap at a time, once the zoom onto the target is done.
fn attack_info(env: &mut Environment, game: Game, battle: &Battle) -> Option<FocusMarker> {
    let count = if game.attack_info_count != 0 {
        read_u32(env, game.attack_info_count).min(64)
    } else {
        1
    };
    let state = &mut env.framework_state.song_summoner.game_input;
    let idle = state.taps.is_idle();
    let ready = battle.sub == PHASE_RUNNING && !battle.zooming && idle;
    let reason = (!ready).then(|| {
        format!(
            "sub-phase {:#x}, zooming {}, tap under way {}",
            battle.sub, battle.zooming, !idle
        )
    });
    log_waiting(state, "attack info", reason);
    if !ready {
        return None;
    }
    if let Some((x, y)) = state.battle_input.taps.pop_front() {
        state.taps.tap(x, y);
        return None;
    }
    while let Some(role) = state.commands.pop_front() {
        let taps = battle::attack_info_taps(role, count);
        if let Some((&(x, y), rest)) = taps.split_first() {
            log!(
                "input: attack info ({} targets), {:?}: tapping {:?} x{}",
                count,
                role,
                (x, y),
                taps.len()
            );
            state.taps.tap(x, y);
            state.battle_input.taps.extend(rest);
            break;
        }
    }
    None
}

/// The deploy (phase 8): placing a unit, the sort panel and the map view
/// are handled here; the card list, the common menu and dialogs are
/// widgets.
fn sortie_commands(env: &mut Environment, game: Game, battle: &Battle) -> Battled {
    let work = game.sortie_work;
    let screen = read_u32(env, work);
    let sub = read_u32(env, work + 4);
    enter_screen(&mut env.framework_state.song_summoner.game_input, screen);
    match screen {
        SORTIE_PLACING => Battled::Done(locked_select(env, game, battle, Locked::Place)),
        SORTIE_SORT => Battled::Done(sort_panel(env, game, sub)),
        SORTIE_MAP_VIEW => Battled::Done(map_view(env, battle, sub)),
        _ => Battled::Menus,
    }
}

/// The deploy's sort panel: Edit Troopers' six rows, and its back icon.
fn sort_panel(env: &mut Environment, game: Game, sub: u32) -> Option<FocusMarker> {
    let back_icon = read_prim_rect(env, game, read_u32(env, game.sortie_work + 0x68));
    let rows = battle::sort_panel_rows();
    let set = ButtonSet {
        enabled: vec![true; rows.len()],
        rects: rows,
        back: Some(back_icon.map_or(SORT_CANCEL, rect_center)),
        two_tap: false,
        selected: None,
    };
    let state = &mut env.framework_state.song_summoner.game_input;
    let mut index = state.battle_input.sort_row;
    if sub == SORT_TAKING_TOUCHES && state.taps.is_idle() {
        if let Some(role) = state.commands.pop_front() {
            let (new_index, tap) = button_command(&set, index, role);
            index = new_index;
            if let Some((x, y)) = tap {
                log!("input: sort panel, tapping {:?}", (x, y));
                state.taps.tap(x, y);
            }
        }
    }
    state.battle_input.sort_row = index;
    set.rects.get(index).map(|&r| (r, FocusShape::Brackets))
}

/// The deploy's map view: the unit select pattern. The D-pad moves a free
/// cursor (panning to keep it holdable), confirm holds a finger on its
/// tile while pressed (letting go on a unit opens its status), back taps
/// the back icon. In a unit's status: north or confirm flip the page,
/// back closes it.
fn map_view(env: &mut Environment, battle: &Battle, sub: u32) -> Option<FocusMarker> {
    let state = &mut env.framework_state.song_summoner.game_input;
    if sub == MAP_VIEW_STATUS {
        status_commands(state);
        return None;
    }
    let camera = battle.camera;
    let grid = &battle.grid;
    let still = state.battle_input.origin == Some(camera.origin);
    state.battle_input.origin = Some(camera.origin);
    let idle = state.taps.is_idle();
    let ready = sub == MAP_VIEW_TAKING_TOUCHES && still && idle;
    let reason = (!ready).then(|| {
        format!(
            "map view state {}, camera still {}, tap under way {}",
            sub, still, !idle
        )
    });
    log_waiting(state, "deploy map view", reason);
    let mut cursor = state
        .battle_input
        .cursor
        .filter(|&t| grid.contains(t))
        .or(Some(battle.cursor).filter(|&t| grid.contains(t)))
        .unwrap_or((grid.w / 2, grid.h / 2));
    if ready {
        while let Some(role) = state.commands.pop_front() {
            match battle::unit_select_intent(role) {
                battle::UnitSelectIntent::Move(dir) => {
                    cursor = battle::grid_step(grid, cursor, dir);
                    state.battle_input.act = false;
                    state.battle_input.last_pan = None;
                }
                battle::UnitSelectIntent::Confirm => state.battle_input.act = true,
                _ if role == Role::Back => {
                    log!("input: deploy map view, back: tapping {:?}", MAP_VIEW_BACK);
                    state.taps.tap(MAP_VIEW_BACK.0, MAP_VIEW_BACK.1);
                    break;
                }
                _ => {}
            }
        }
        let centre = camera.tile_centre(cursor);
        if !state.taps.is_idle() {
            // Back's tap is under way.
        } else if battle::holdable_in(centre, battle::WHOLE_SCREEN) {
            state.battle_input.last_pan = None;
            if std::mem::take(&mut state.battle_input.act) {
                let (x, y) = camera.hold_point(cursor);
                log!("input: deploy map view, holding tile {:?} at {:?}", cursor, (x, y));
                let frames = battle::SELECT_HOLD_FRAMES;
                state.taps.push(Gesture::Hold { x, y, frames });
                state.taps.keep_down(state.confirm_down);
            }
        } else if let Some((dx, dy)) =
            battle::pan_toward(&camera, cursor, state.battle_input.last_pan)
        {
            log!("input: deploy map view, panning by {:?} toward tile {:?}", (dx, dy), cursor);
            state.battle_input.last_pan = Some(camera.origin);
            let (x, y) = battle::TAP_CENTRE;
            state.taps.push(Gesture::Pan { x, y, dx, dy });
        } else {
            state.battle_input.act = false;
        }
    }
    state.battle_input.cursor = Some(cursor);
    Some((camera.tile_rect(cursor), FocusShape::Diamond))
}

/// The victory terms, over the battle's start, MENU or the deploy menu:
/// confirm or back tap the middle once they take touches (state 4). Presses
/// before then are dropped, since the terms clear the touch as they finish
/// sliding in. `None` if they aren't up.
fn terms_commands(
    env: &mut Environment,
    game: Game,
    battle: &Battle,
) -> Option<Option<FocusMarker>> {
    if game.terms_work == 0 {
        return None;
    }
    let work = game.terms_work;
    let terms_state = read_u32(env, work);
    // Up: their title animation or window exists. `TacticsMapTerms_End`
    // sets both to −1, as `_Init` does.
    let exists =
        read_u32(env, work + 0x10) != u32::MAX || read_u32(env, work + 0x14) != u32::MAX;
    let shown_here = matches!(battle.phase, 5 | 6 | 21)
        || (battle.phase == 8
            && game.sortie_work != 0
            && read_u32(env, game.sortie_work) == SORTIE_TERMS);
    if !(exists && shown_here && terms_state < TERMS_DONE) {
        return None;
    }
    let state = &mut env.framework_state.song_summoner.game_input;
    if terms_state != TERMS_TAKING_TOUCHES {
        if !state.commands.is_empty() {
            log!(
                "input: victory terms not taking touches yet (state {}), dropping presses",
                terms_state
            );
            state.commands.clear();
        }
        return Some(None);
    }
    if state.taps.is_idle() {
        while let Some(role) = state.commands.pop_front() {
            if matches!(role, Role::Confirm | Role::Back) {
                log!("input: victory terms, closing");
                state.taps.tap(CENTER.0, CENTER.1);
                break;
            }
        }
    }
    Some(None)
}

/// Whether the Listening Point scene shows its SKIP icon, and a tap on it
/// would ask to skip: the icon's `SysAnim` (id at `lpwork + 0x8c`) is shown
/// and nothing's asked yet (`+0x80` is 0). While it's hidden, a tap there
/// would count as "tap to continue" instead.
fn listening_point_skip(env: &Environment, game: Game) -> bool {
    if game.listening_point_main == 0 || game.anim_work == 0 {
        return false;
    }
    let Some(work) = task_work(env, game, game.listening_point_main) else {
        return false;
    };
    let id = read_u32(env, work + 0x8c) as i32;
    let slot = |offset: u32| match u32::try_from(id) {
        Ok(id) if id < 512 => read_u32(env, game.anim_work + id * 0x5c + offset),
        _ => 0,
    };
    read_u32(env, work + 0x80) == 0 && battle::anim_visible(id, slot(0), slot(0xc))
}

/// [run_commands] outside battle's map: the open menu, if any, and the
/// rectangle to outline.
fn menu_commands(
    env: &mut Environment,
    game: Game,
    widgets: Vec<Widget>,
) -> Option<(f32, f32, f32, f32)> {
    // A cutscene's SKIP, or the Listening Point scene's.
    let skip_shown = (game.skip_able != 0
        && env.mem.read(ConstPtr::<u8>::from_bits(game.skip_able)) != 0)
        || listening_point_skip(env, game);
    let state = &mut env.framework_state.song_summoner.game_input;
    // Not in unit select: its pending confirm and pan are void.
    state.battle_confirm = false;
    state.battle_pan = None;

    // Start taps the cutscene's SKIP button, whatever else is up (the
    // cutscene's text box isn't a menu). The "Skip?" dialog it opens is an
    // ordinary SysDialog, handled below from the next frame.
    if state.commands.contains(&Role::Skip) {
        state.commands.retain(|&role| role != Role::Skip);
        let tap = skip_shown && state.taps.is_idle();
        log!("input: skip (button shown: {}, tapping: {})", skip_shown, tap);
        if tap {
            state.taps.tap(SKIP_BUTTON.0, SKIP_BUTTON.1);
        }
    }
    let open: Vec<u32> = widgets.iter().map(Widget::work).collect();
    if open != state.known_menus {
        let kinds: Vec<&str> = widgets
            .iter()
            .map(|w| match w {
                Widget::Options(_) => "options",
                Widget::Help(_) => "help",
                Widget::Buttons(_) => "button menu",
                Widget::Dialog(_) => "dialog",
                Widget::Drum(_) => "drum",
                Widget::Scene { .. } => "screen buttons",
                Widget::Cards { .. } => "card list",
                Widget::List(_) => "list",
                Widget::World(_) => "world map",
            })
            .collect();
        log!("input: menus open: {:?}", kinds);
    }
    let focused = state.focus.map(|(work, _)| work);
    let widget = choose_menu(&open, &state.known_menus, focused).map(|i| widgets[i].clone());
    // It wasn't open last frame, even if it's at the same address as one
    // that was focused before.
    let appeared = widget
        .as_ref()
        .is_some_and(|w| !state.known_menus.contains(&w.work()));
    state.known_menus = open;

    let Some(widget) = widget else {
        state.focus = None;
        state.world_target = None;
        // No menu: confirm is a tap anywhere.
        while let Some(role) = state.commands.pop_front() {
            if role == Role::Confirm {
                state.taps.tap(CENTER.0, CENTER.1);
            }
        }
        return None;
    };
    if state.focus.map(|(work, _)| work) != Some(widget.work()) {
        state.world_target = None;
        match &widget {
            Widget::Help(help) => {
                log!(
                    "input: help, page {}, tab {}, {} items, rows {:?}, selected {:?}",
                    help.page,
                    help.tab,
                    help.count,
                    help.rows,
                    help.selected
                );
            }
            Widget::Options(options) => {
                log!(
                    "input: options, knob x {}, switches {:?}, button {:?}, back {:?}",
                    options.knob_x,
                    options.switches,
                    options.button,
                    options.back
                );
            }
            Widget::Buttons(menu) => {
                log!(
                    "input: button menu at ({}, {}), {} buttons",
                    menu.x,
                    menu.y,
                    menu.enabled.len()
                );
            }
            Widget::Dialog(dialog) => {
                log!(
                    "input: dialog, buttons {:?}, focus starts on {:?}",
                    dialog.rects,
                    state.dialog_default
                );
            }
            Widget::Scene { set, .. } => {
                log!("input: screen buttons {:?}, back {:?}", set.rects, set.back);
            }
            Widget::Cards { list, .. } => {
                log!(
                    "input: card list, centre {}, icons {:?}, status panel {}",
                    list.centre_x,
                    list.icons,
                    list.status_panel
                );
            }
            Widget::List(list) => {
                log!(
                    "input: list, {} rows ({} enabled), {} shown from {}, first row {:?}",
                    list.rows.len(),
                    list.enabled.iter().filter(|&&e| e).count(),
                    list.shown,
                    list.first,
                    list.rows.first()
                );
            }
            Widget::World(map) => {
                let open: Vec<usize> = (0..map.symbols.len())
                    .filter(|&i| map.symbols[i].open)
                    .collect();
                log!(
                    "input: world map, at {}, highlighted {}, open {:?}, camera {:?}, zoom {}",
                    map.current,
                    map.highlighted,
                    open,
                    map.camera,
                    map.zoom
                );
            }
            Widget::Drum(drum) => {
                log!(
                    "input: drum at ({}, {}), {}x{}",
                    drum.x,
                    drum.y,
                    drum.w,
                    drum.h
                );
            }
        }
    }

    let work = widget.work();
    let set = match &widget {
        Widget::Buttons(menu) => Some(ButtonSet::from(menu)),
        Widget::Dialog(dialog) => Some(ButtonSet::from(dialog)),
        Widget::Scene { set, .. } => Some(set.clone()),
        Widget::Help(_)
        | Widget::Options(_)
        | Widget::Drum(_)
        | Widget::Cards { .. }
        | Widget::List(_)
        | Widget::World(_) => None,
    };
    if let Some(set) = set {
        // A dialog starts on its default whenever it appears (the deploy's
        // party-full dialog on No); others keep their focus while open.
        let is_dialog = matches!(widget, Widget::Dialog(_));
        let default = match state.dialog_default {
            Some(i) if is_dialog => i,
            _ => first_enabled(&set.enabled),
        };
        let mut index = widget_focus(
            state.focus,
            work,
            is_dialog && appeared,
            default,
            set.rects.len(),
        );
        if set.two_tap {
            // One command at a time, once the last taps have landed, so
            // each is judged against the game's selection (which a finger
            // may also have changed).
            if state.taps.is_idle() {
                index = set.selected.unwrap_or(index);
                if let Some(role) = state.commands.pop_front() {
                    let (new_index, taps) = select_command(&set, index, role);
                    index = new_index;
                    for (x, y) in taps {
                        state.taps.tap(x, y);
                    }
                }
            }
            state.focus = Some((work, index));
            // The game highlights the selected button itself.
            if set.selected == Some(index) || !state.taps.is_idle() {
                return None;
            }
            return set.rects.get(index).copied();
        }
        while let Some(role) = state.commands.pop_front() {
            let (new_index, tap) = button_command(&set, index, role);
            index = new_index;
            if let Some((x, y)) = tap {
                state.taps.tap(x, y);
            }
        }
        state.focus = Some((work, index));
        let rect = set.rects.get(index).copied();
        // Taps use the whole touch area; the outline goes around the button
        // art instead, which the touch area overhangs.
        if matches!(widget, Widget::Dialog(_)) {
            return rect.map(dialog_outline);
        }
        return rect;
    }

    if let Widget::World(map) = &widget {
        state.focus = Some((map.work, 0));
        // The game draws its own highlight, so there's no outline.
        if !map.ready {
            // The player is walking (or the camera gliding): presses now
            // would otherwise play out on arrival, in the location menu.
            state.commands.clear();
            state.world_target = None;
            return None;
        }
        // One gesture at a time, each read against the map it left.
        if !state.taps.is_idle() {
            return None;
        }
        let target = match state.world_target {
            Some((work, t, confirm)) if work == map.work => Some((t, confirm)),
            _ => None,
        };
        let target = target.or_else(|| {
            state.world_pan = None;
            std::iter::from_fn(|| state.commands.pop_front()).find_map(|r| world_target(map, r))
        });
        let Some((t, confirm)) = target else {
            state.world_target = None;
            return None;
        };
        match world_step(map, t, confirm, state.world_pan) {
            WorldStep::Done => {
                state.world_target = None;
                state.world_pan = None;
            }
            WorldStep::Pan(pan) => {
                state.world_target = Some((map.work, t, confirm));
                state.world_pan = Some(map.camera);
                state.taps.push(pan);
            }
            WorldStep::Tap { at, again } => {
                state.taps.tap(at.0, at.1);
                state.world_pan = None;
                state.world_target = again.then_some((map.work, t, confirm));
            }
        }
        return None;
    }

    if let Widget::Help(help) = &widget {
        let id = help.id();
        // A list starts on the item last opened, so closing a page lands
        // back on it.
        let mut focus = match state.focus {
            Some((w, i)) if w == id => i,
            _ => help.selected.unwrap_or(0),
        };
        if !help.page {
            focus = focus.min(help.rows.len().saturating_sub(1));
        }
        if help.ready && state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, gesture) = help_command(help, focus, role);
                focus = new_focus;
                match gesture {
                    Some(Some(gesture)) => {
                        state.taps.push(gesture);
                        break;
                    }
                    Some(None) => {}
                    // Wait for the scroll below to bring the row on screen.
                    None => {
                        state.commands.push_front(role);
                        break;
                    }
                }
            }
            if !help.page && state.taps.is_idle() {
                if let Some(drag) = help.scroll_toward(focus) {
                    state.taps.push(drag);
                }
            }
        }
        state.focus = Some((id, focus));
        // A page has nothing to point at.
        if help.page {
            return None;
        }
        return help.rows.get(focus).copied().filter(|_| help.row_visible(focus));
    }

    if let Widget::Options(options) = &widget {
        let rows = options.rows();
        let mut focus = match state.focus {
            Some((w, i)) if w == options.work && i < rows.len() => i,
            _ => 0,
        };
        // Commands wait while a switch or button animates, and each gesture
        // waits for the last to finish.
        if options.ready && state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, gesture) = options_command(options, focus, role);
                focus = new_focus;
                if let Some(gesture) = gesture {
                    state.taps.push(gesture);
                    break;
                }
            }
        }
        state.focus = Some((options.work, focus));
        return rows.get(focus).copied();
    }

    if let Widget::List(list) = &widget {
        let mut focus = match state.focus {
            Some((w, i)) if w == list.work && i < list.rows.len() => i,
            _ => list.cursor,
        };
        state.since_drag = state.since_drag.saturating_add(1);
        if state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, tap) = list_command(list, focus, role);
                focus = new_focus;
                match tap {
                    Some(Some((x, y))) => {
                        state.taps.tap(x, y);
                        break;
                    }
                    Some(None) => {}
                    // Wait for the scroll below to bring the row on screen.
                    None => {
                        state.commands.push_front(role);
                        break;
                    }
                }
            }
            // Bring the focused row on screen, a row at a time. The frames
            // after a drag let its glide register as the list's speed.
            if state.since_drag > HOLD_FRAMES + 2 {
                if let Some(Gesture::Drag { x, y, dy }) = list.scroll_toward(focus) {
                    state.taps.drag(x, y, dy);
                    state.since_drag = 0;
                }
            }
        }
        state.focus = Some((list.work, focus));
        return list
            .rows
            .get(focus)
            .filter(|_| list.row_visible(focus))
            .map(|&row| list_outline(row, list.span));
    }

    if let Widget::Cards { work, list } = &widget {
        let mut focus = match state.focus {
            Some((w, _)) if w == *work => state.card_focus.unwrap_or(CardFocus::Cards),
            _ => CardFocus::Cards,
        };
        // One tap at a time, so a turn finishes before the next.
        if state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, tap) = card_list_command(list, focus, role);
                focus = new_focus;
                if let Some((x, y)) = tap {
                    state.taps.tap(x, y);
                    break;
                }
            }
        }
        state.focus = Some((*work, 0));
        state.card_focus = Some(focus);
        return Some(list.focus_rect(focus));
    }

    // Otherwise it's a drum, which has its own cursor.
    let Widget::Drum(drum) = widget else {
        unreachable!()
    };
    state.focus = Some((drum.work, 0));
    // A drum only takes a tap once it has stopped turning, and one command
    // at a time: the next waits for this one's turn.
    if drum.settled && state.taps.is_idle() {
        while let Some(role) = state.commands.pop_front() {
            let Some(at) = drum_command(&drum, role) else {
                continue;
            };
            state.taps.tap(at.0, at.1);
            break;
        }
    }
    Some(drum.select_rect())
}

/// Outline `marker` (game points) in the window's (portrait) coordinates.
fn show_focus(env: &mut Environment, main_view: id, marker: Option<FocusMarker>) {
    let show = env.framework_state.song_summoner.game_input.pad_mode;
    // A tile's diamond is drawn inside it, and a tile half off the edge is
    // better cut off than squashed, so only brackets are pulled on screen.
    let marker = marker.map(|(rect, shape)| match shape {
        FocusShape::Brackets => (keep_on_screen(rect), shape),
        FocusShape::Diamond => (rect, shape),
    });
    let rect = match marker {
        Some(((x, y, w, h), shape)) if show => {
            let rect = CGRect {
                origin: CGPoint { x, y },
                size: CGSize {
                    width: w,
                    height: h,
                },
            };
            let on_screen: CGRect = msg![env; main_view convertRect:rect toView:nil];
            let on_screen = (
                on_screen.origin.x,
                on_screen.origin.y,
                on_screen.size.width,
                on_screen.size.height,
            );
            Some(((x, y, w, h), (on_screen, shape)))
        }
        _ => None,
    };
    if let Some(window) = env.window.as_mut() {
        window.set_focus_marker(rect.map(|(_, on_screen)| on_screen));
    }
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.logged_outline != Some(rect) {
        match rect {
            Some((game, screen)) => {
                let drawn = env.window.as_ref().and_then(|w| w.focus_marker_visible_at());
                log!(
                    "input: outline game {:?} -> screen {:?} -> window {:?}",
                    game,
                    screen,
                    drawn
                );
            }
            None => {
                log!("input: no outline (controller in use: {})", show);
            }
        }
        state.logged_outline = Some(rect);
    }
}

/// Run before each of the game's frames.
fn before_frame(env: &mut Environment, main_view: id) {
    let Some(game) = game(env) else {
        return;
    };
    // m_mode 1: the picker is up, and -[MainView mainLoop] skips the
    // scene, so nothing would read or clear a touch.
    let m_mode_offset: u32 = env.mem.read(game.m_mode_offset);
    let m_mode = read_u32(env, main_view.to_bits() + m_mode_offset);
    if m_mode == 1 {
        let state = &mut env.framework_state.song_summoner.game_input;
        state.taps.clear();
        state.finger.clear();
        state.commands.clear();
        show_focus(env, main_view, None);
        clear_battle_overlay(env);
        return;
    }
    env.framework_state.song_summoner.game_input.frame += 1;
    log_tasks(env, game);
    let focus = run_commands(env, game);
    show_focus(env, main_view, focus);
    battle_debug(env, game, main_view);

    // One touch call per frame: the held finger's while it's down (or
    // about to go down with no tap waiting), else the taps'. Handlers never
    // use both at once.
    let state = &mut env.framework_state.song_summoner.game_input;
    let step = if state.finger.is_down() || (state.taps.is_idle() && !state.finger.is_idle()) {
        state.finger.next_frame()
    } else if !state.taps.is_idle() {
        state.taps.next_frame()
    } else {
        None
    };
    let Some(step) = step else {
        return;
    };
    let existing = state.points;
    let points = match existing {
        Some(points) => points,
        None => {
            let points = env.mem.alloc(2 * 8).cast();
            env.framework_state.song_summoner.game_input.points = Some(points);
            points
        }
    };
    match step {
        TouchStep::Began(x, y) => {
            let at = CGPoint { x, y };
            env.mem.write(points, at);
            // The previous position, as `touchesBegan:` passes it.
            env.mem.write(points + 1, at);
            // One finger, tap count 1.
            () = game
                .began
                .call_from_host(env, (points, points + 1, 1i32, 1i32));
        }
        TouchStep::Moved(x, y, prev_x, prev_y) => {
            env.mem.write(points, CGPoint { x, y });
            env.mem.write(points + 1, CGPoint {
                x: prev_x,
                y: prev_y,
            });
            // One finger.
            () = game.moved.call_from_host(env, (points, points + 1, 1i32));
        }
        TouchStep::Ended(x, y) => {
            env.mem.write(points, CGPoint { x, y });
            () = game.ended.call_from_host(env, (points, 1i32));
        }
    }
    log_dbg!("input: {:?}", step);
    // When a synthetic finger goes down and up, to time holds.
    if cfg!(debug_assertions) && !matches!(step, TouchStep::Moved(..)) {
        let frame = env.framework_state.song_summoner.game_input.frame;
        log!("input: finger {:?} (frame {})", step, frame);
    }
}

/// `timer`: the game's frame timer is an `NSTimer`, which sends its
/// selector with itself as the argument (`-mainLoop` ignores it). touchHLE
/// checks host method signatures, so it has to be declared here.
fn main_loop(env: &mut Environment, this: id, cmd: SEL, timer: id) {
    before_frame(env, this);
    match env.objc.app_override_original("MainView", "mainLoop") {
        Some(original) => {
            () = original.call_from_host(env, (this, cmd, timer));
        }
        None => {
            log!("input: the game's -[MainView mainLoop] is missing!");
        }
    }
}

pub const OVERRIDES: &[Override] = &[Override {
    class: "MainView",
    selector: "mainLoop",
    class_method: false,
    imp: &(main_loop as fn(&mut Environment, id, SEL, id)),
}];

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(queue: &mut TapQueue, n: usize) -> Vec<Option<TouchStep>> {
        (0..n).map(|_| queue.next_frame()).collect()
    }

    #[test]
    fn a_tap_is_began_then_ended_a_frame_later() {
        let mut queue = TapQueue::default();
        queue.tap(10.0, 20.0);
        assert!(!queue.is_idle());
        assert_eq!(
            frames(&mut queue, 4),
            vec![
                Some(TouchStep::Began(10.0, 20.0)),
                None, // the finger is down during this frame
                Some(TouchStep::Ended(10.0, 20.0)),
                None,
            ]
        );
        assert!(queue.is_idle());
    }

    #[test]
    fn began_and_ended_are_never_before_the_same_frame() {
        // SysTouch_Main clears "began" at the end of the frame, and the
        // whole struct after an "ended": the scene must see each.
        let mut queue = TapQueue::default();
        queue.tap(1.0, 1.0);
        queue.tap(2.0, 2.0);
        let steps = frames(&mut queue, 8);
        assert_eq!(
            steps,
            vec![
                Some(TouchStep::Began(1.0, 1.0)),
                None,
                Some(TouchStep::Ended(1.0, 1.0)),
                Some(TouchStep::Began(2.0, 2.0)),
                None,
                Some(TouchStep::Ended(2.0, 2.0)),
                None,
                None,
            ]
        );
    }

    #[test]
    fn a_hold_stays_down_still_then_lifts() {
        // No move in between (SysTouch_Moved_F1 would make it a flick):
        // down for `frames` frames, then up where it went down.
        let mut queue = TapQueue::default();
        queue.push(Gesture::Hold {
            x: 10.0,
            y: 20.0,
            frames: 10,
        });
        let steps = frames(&mut queue, 12);
        assert_eq!(steps[0], Some(TouchStep::Began(10.0, 20.0)));
        assert!(steps[1..10].iter().all(Option::is_none), "{steps:?}");
        assert_eq!(steps[10], Some(TouchStep::Ended(10.0, 20.0)));
        assert_eq!(steps[11], None);
        assert!(queue.is_idle());
    }

    #[test]
    fn a_hold_kept_down_lasts_until_let_go() {
        // Confirm held: the finger stays down past the hold's frames, and
        // lifts on the frame after the button comes up.
        let mut queue = TapQueue::default();
        queue.push(Gesture::Hold {
            x: 10.0,
            y: 20.0,
            frames: 3,
        });
        queue.keep_down(true);
        let steps = frames(&mut queue, 20);
        assert_eq!(steps[0], Some(TouchStep::Began(10.0, 20.0)));
        assert!(steps[1..].iter().all(Option::is_none), "{steps:?}");
        assert!(!queue.is_idle());
        queue.keep_down(false);
        assert_eq!(queue.next_frame(), Some(TouchStep::Ended(10.0, 20.0)));
        assert!(queue.is_idle());
    }

    #[test]
    fn a_hold_let_go_early_still_lasts_its_frames() {
        // A quick press: the game only calls it a hold after 6 frames.
        let mut queue = TapQueue::default();
        queue.push(Gesture::Hold {
            x: 1.0,
            y: 2.0,
            frames: 10,
        });
        queue.keep_down(true);
        queue.next_frame(); // Began
        queue.keep_down(false);
        let steps = frames(&mut queue, 10);
        // Down for 10 frames in all, as if it had never been kept.
        assert!(steps[..9].iter().all(Option::is_none), "{steps:?}");
        assert_eq!(steps[9], Some(TouchStep::Ended(1.0, 2.0)));
    }

    #[test]
    fn keeping_down_doesnt_hold_a_tap_or_outlive_a_clear() {
        let mut queue = TapQueue::default();
        queue.keep_down(true);
        queue.tap(1.0, 1.0);
        assert_eq!(
            frames(&mut queue, 3),
            vec![
                Some(TouchStep::Began(1.0, 1.0)),
                None,
                Some(TouchStep::Ended(1.0, 1.0)),
            ]
        );
        queue.clear();
        queue.push(Gesture::Hold {
            x: 1.0,
            y: 1.0,
            frames: 2,
        });
        assert_eq!(
            frames(&mut queue, 3),
            vec![
                Some(TouchStep::Began(1.0, 1.0)),
                None,
                Some(TouchStep::Ended(1.0, 1.0)),
            ]
        );
    }

    // B0.2: a held finger that slides.

    #[test]
    fn a_pressed_finger_stays_down() {
        let mut finger = Finger::default();
        finger.press(10.0, 20.0);
        assert!(!finger.is_idle());
        assert_eq!(finger.next_frame(), Some(TouchStep::Began(10.0, 20.0)));
        // Down and still: nothing more, and idle (ready for the next step).
        assert!(finger.is_idle() && finger.is_down());
        let steps: Vec<_> = (0..30).map(|_| finger.next_frame()).collect();
        assert!(steps.iter().all(Option::is_none), "{steps:?}");
        assert_eq!(finger.finger(), Some((10.0, 20.0)));
    }

    #[test]
    fn a_slide_waits_until_the_finger_is_a_hold() {
        // Tactics_CtrlTest calls a finger a hold after more than 6 still
        // frames, and a move sets the flick flag at once: moving earlier
        // would make it a map scroll.
        let mut finger = Finger::default();
        finger.press(10.0, 20.0);
        finger.slide_to(30.0, 40.0);
        let steps: Vec<_> = (0..MIN_STILL_FRAMES + 2).map(|_| finger.next_frame()).collect();
        assert_eq!(steps[0], Some(TouchStep::Began(10.0, 20.0)));
        let waited = MIN_STILL_FRAMES as usize;
        assert!(steps[1..waited].iter().all(Option::is_none), "{steps:?}");
        // The new point, then the previous one.
        assert_eq!(steps[waited], Some(TouchStep::Moved(30.0, 40.0, 10.0, 20.0)));
        assert_eq!(steps[waited + 1], None);
        assert!(finger.is_idle());
    }

    #[test]
    fn a_slide_after_the_hold_is_at_once_and_one_per_frame() {
        let mut finger = Finger::default();
        finger.press(1.0, 1.0);
        for _ in 0..=MIN_STILL_FRAMES {
            finger.next_frame();
        }
        finger.slide_to(2.0, 2.0);
        finger.slide_to(3.0, 3.0);
        // Where it will be, once the queue has run.
        assert_eq!(finger.finger(), Some((3.0, 3.0)));
        assert_eq!(finger.next_frame(), Some(TouchStep::Moved(2.0, 2.0, 1.0, 1.0)));
        assert_eq!(finger.next_frame(), Some(TouchStep::Moved(3.0, 3.0, 2.0, 2.0)));
        assert_eq!(finger.next_frame(), None);
    }

    #[test]
    fn a_lift_after_a_slide_is_where_it_slid_to() {
        let mut finger = Finger::default();
        finger.press(1.0, 1.0);
        finger.slide_to(5.0, 6.0);
        finger.lift();
        assert_eq!(finger.finger(), None);
        let steps: Vec<_> = (0..MIN_STILL_FRAMES + 3).map(|_| finger.next_frame()).collect();
        let waited = MIN_STILL_FRAMES as usize;
        assert_eq!(steps[waited], Some(TouchStep::Moved(5.0, 6.0, 1.0, 1.0)));
        assert_eq!(steps[waited + 1], Some(TouchStep::Ended(5.0, 6.0)));
        assert!(finger.is_idle() && !finger.is_down());
    }

    #[test]
    fn a_quick_press_and_lift_still_lasts_as_a_hold() {
        let mut finger = Finger::default();
        finger.press(1.0, 2.0);
        finger.lift();
        let steps: Vec<_> = (0..MIN_STILL_FRAMES + 2).map(|_| finger.next_frame()).collect();
        assert_eq!(steps[0], Some(TouchStep::Began(1.0, 2.0)));
        let waited = MIN_STILL_FRAMES as usize;
        assert!(steps[1..waited].iter().all(Option::is_none), "{steps:?}");
        assert_eq!(steps[waited], Some(TouchStep::Ended(1.0, 2.0)));
    }

    #[test]
    fn a_press_after_a_lift_is_on_a_later_frame() {
        // Began and Ended before the same frame would hide the Began.
        let mut finger = Finger::default();
        finger.press(1.0, 1.0);
        finger.lift();
        finger.press(9.0, 9.0);
        assert_eq!(finger.finger(), Some((9.0, 9.0)));
        let steps: Vec<_> = (0..2 * MIN_STILL_FRAMES).map(|_| finger.next_frame()).collect();
        let waited = MIN_STILL_FRAMES as usize;
        assert_eq!(steps[waited], Some(TouchStep::Ended(1.0, 1.0)));
        assert_eq!(steps[waited + 1], Some(TouchStep::Began(9.0, 9.0)));
    }

    #[test]
    fn a_cleared_finger_is_up_without_a_lift() {
        let mut finger = Finger::default();
        finger.press(1.0, 1.0);
        finger.next_frame();
        finger.slide_to(2.0, 2.0);
        finger.clear();
        assert!(finger.is_idle() && !finger.is_down());
        assert_eq!(finger.finger(), None);
        assert_eq!(finger.next_frame(), None);
    }

    #[test]
    fn slides_and_lifts_with_the_finger_up_do_nothing() {
        let mut finger = Finger::default();
        finger.slide_to(2.0, 2.0);
        finger.lift();
        assert_eq!(finger.finger(), None);
        assert_eq!(finger.next_frame(), None);
        assert_eq!(finger.next_frame(), None);
        assert!(finger.is_idle());
    }

    // B6.1: a dialog's focus when it appears.

    #[test]
    fn a_dialog_that_appears_starts_on_its_default() {
        // Reopened at the same address as the one last focused: still a
        // new dialog, so the old index doesn't carry over.
        assert_eq!(widget_focus(Some((0x10, 1)), 0x10, true, 0, 2), 0);
        // The deploy's party-full dialog starts on No (button 1).
        assert_eq!(widget_focus(None, 0x10, true, 1, 2), 1);
        // Open all along: the focus stays where it was.
        assert_eq!(widget_focus(Some((0x10, 1)), 0x10, false, 0, 2), 1);
        // Another widget had the focus: the default.
        assert_eq!(widget_focus(Some((0x20, 1)), 0x10, false, 0, 2), 0);
        // A stale index past the end: the default.
        assert_eq!(widget_focus(Some((0x10, 5)), 0x10, false, 0, 2), 0);
        // A default past the end (a dialog with fewer buttons): the first.
        assert_eq!(widget_focus(None, 0x10, true, 1, 1), 0);
    }

    #[test]
    fn mashing_only_queues_a_couple_of_taps() {
        let mut queue = TapQueue::default();
        for _ in 0..10 {
            queue.tap(5.0, 5.0);
        }
        let began = frames(&mut queue, 40)
            .into_iter()
            .filter(|s| matches!(s, Some(TouchStep::Began(..))))
            .count();
        assert_eq!(began, MAX_QUEUED_TAPS);
    }

    #[test]
    fn clearing_drops_the_finger_and_the_queue() {
        let mut queue = TapQueue::default();
        queue.tap(1.0, 1.0);
        queue.tap(2.0, 2.0);
        queue.next_frame(); // Began
        queue.clear();
        assert!(queue.is_idle());
        assert_eq!(queue.next_frame(), None);
    }

    // SysButtonMenu layout, from SysButtonMenu_Open/Position/Size. The
    // title's start menu is positioned at (240, 212), buttons 160×54.
    fn title_menu(count: usize) -> Menu {
        Menu {
            work: 0x1000,
            x: 240,
            y: 212,
            w: 160,
            h: 54,
            enabled: vec![true; count],
            outrange_cancel: true,
            cancel_button: false,
        }
    }

    #[test]
    fn buttons_stack_down_from_the_centred_position() {
        let menu = title_menu(3);
        assert_eq!(menu.button_center(0), (240.0, 158.0));
        assert_eq!(menu.button_center(1), (240.0, 214.0));
        assert_eq!(menu.button_center(2), (240.0, 270.0));
        let menu = title_menu(2);
        assert_eq!(menu.button_center(0), (240.0, 185.0));
        assert_eq!(menu.button_center(1), (240.0, 241.0));
        assert_eq!(title_menu(1).button_center(0), (240.0, 212.0));
    }

    #[test]
    fn half_heights_round_toward_zero_like_the_game() {
        // (count - 1) * h / 2 is C integer division: 55 / 2 = 27.
        let mut menu = title_menu(2);
        menu.h = 55;
        assert_eq!(menu.button_center(0), (240.0, 185.0));
        assert_eq!(menu.button_center(1), (240.0, 242.0));
    }

    #[test]
    fn button_rect_is_centred_on_the_button() {
        let menu = title_menu(3);
        assert_eq!(menu.button_rect(1), (160.0, 187.0, 160.0, 54.0));
    }

    #[test]
    fn focus_wraps_and_skips_disabled_buttons() {
        let enabled = [true, false, true, true];
        assert_eq!(next_enabled(&enabled, 0, 1), 2);
        assert_eq!(next_enabled(&enabled, 3, 1), 0);
        assert_eq!(next_enabled(&enabled, 0, -1), 3);
        assert_eq!(next_enabled(&enabled, 2, -1), 0);
        // Nothing enabled: stay put.
        assert_eq!(next_enabled(&[false, false], 1, 1), 1);
    }

    #[test]
    fn a_menu_that_just_opened_takes_the_focus() {
        // 0x20 opened over 0x10 (in an earlier table slot, as slots are
        // reused first-free).
        assert_eq!(choose_menu(&[0x20, 0x10], &[0x10], Some(0x10)), Some(0));
    }

    #[test]
    fn the_focused_menu_keeps_the_focus() {
        assert_eq!(choose_menu(&[0x10, 0x20], &[0x10, 0x20], Some(0x10)), Some(0));
        // When it closes, the focus goes to another open one.
        assert_eq!(choose_menu(&[0x20], &[0x10, 0x20], Some(0x10)), Some(0));
        assert_eq!(choose_menu(&[], &[0x10], Some(0x10)), None);
    }

    #[test]
    fn first_focus_is_the_first_enabled_button() {
        assert_eq!(first_enabled(&[true, true]), 0);
        assert_eq!(first_enabled(&[false, true]), 1);
        assert_eq!(first_enabled(&[false, false]), 0);
    }

    #[test]
    fn a_tap_outside_every_button_is_off_the_menu() {
        let menu = title_menu(3);
        let (x, y) = OUTSIDE;
        for i in 0..3 {
            let (bx, by, bw, bh) = menu.button_rect(i);
            assert!(!(x >= bx && x < bx + bw && y >= by && y < by + bh));
        }
        // SysButtonMenu_Check's out-of-range test starts with x < x - w/2.
        assert!(x < (menu.x - menu.w / 2) as f32);
    }

    // SysDrum2 zones, from SysDrum2_Check. The title's drum is at
    // (110, 152), 260×128 (Title_Main).
    fn title_drum() -> Drum {
        Drum {
            work: 0x2000,
            x: 110,
            y: 152,
            w: 260,
            h: 128,
            settled: true,
        }
    }

    #[test]
    fn drum_selects_between_the_centre_line_and_a_third_below() {
        let drum = title_drum();
        // c = y - 8 + h/2 = 208; the select zone is [c, c + h/3) = [208, 250).
        assert_eq!(drum.select_rect(), (110.0, 208.0, 260.0, 42.0));
        let (x, y) = drum.select_point();
        assert_eq!(x, 240.0);
        assert!((208.0..250.0).contains(&y), "{y}");
    }

    #[test]
    fn drum_rotates_from_taps_above_and_below_the_select_zone() {
        let drum = title_drum();
        let (_, up) = drum.up_point();
        let (_, down) = drum.down_point();
        // Above the centre line, and still on the drum.
        assert!(up < 208.0 && up >= 152.0, "{up}");
        // Below the select zone, and still on the drum.
        assert!(down > 250.0 && down < 280.0, "{down}");
        // The old centre tap (240, 160) was above the line: a rotation,
        // which is the scrolling the player saw.
        assert!(CENTER.1 < 208.0);
    }

    // Tested on the title menu: a tap above the select band brings in the
    // item below and one under it the item above, so up taps under the
    // band and down above it.
    #[test]
    fn drum_up_brings_in_the_item_above() {
        let drum = title_drum();
        assert_eq!(drum_command(&drum, Role::Up), Some(drum.down_point()));
        assert_eq!(drum_command(&drum, Role::PrevSection), Some(drum.down_point()));
        assert_eq!(drum_command(&drum, Role::Down), Some(drum.up_point()));
        assert_eq!(drum_command(&drum, Role::NextSection), Some(drum.up_point()));
        assert_eq!(drum_command(&drum, Role::Confirm), Some(drum.select_point()));
        assert_eq!(drum_command(&drum, Role::Back), None);
    }

    // SysDialog, from SysDialog_Check. Rects are top-left based.
    fn yes_no() -> ButtonSet {
        ButtonSet {
            rects: vec![(80.0, 236.0, 116.0, 44.0), (258.0, 236.0, 140.0, 44.0)],
            enabled: vec![true, true],
            back: None,
            two_tap: false,
            selected: None,
        }
    }

    #[test]
    fn dialog_buttons_move_left_and_right() {
        let set = yes_no();
        assert_eq!(button_command(&set, 0, Role::NextSection), (1, None));
        assert_eq!(button_command(&set, 1, Role::PrevSection), (0, None));
        // Up/down move too, for column menus.
        assert_eq!(button_command(&set, 0, Role::Down), (1, None));
    }

    #[test]
    fn confirm_taps_the_middle_of_the_focused_rect() {
        assert_eq!(
            button_command(&yes_no(), 1, Role::Confirm),
            (1, Some((328.0, 258.0)))
        );
    }

    #[test]
    fn back_does_nothing_where_nothing_cancels() {
        assert_eq!(button_command(&yes_no(), 0, Role::Back), (0, None));
    }

    #[test]
    fn back_taps_off_the_buttons_on_dialogs_that_cancel() {
        let dialog = Dialog {
            work: 0x3000,
            rects: yes_no().rects,
            enabled: vec![true, true],
            outside_cancels: true,
        };
        let set = ButtonSet::from(&dialog);
        assert_eq!(button_command(&set, 0, Role::Back), (0, Some(OUTSIDE)));
        for &(x, y, w, h) in &set.rects {
            let (px, py) = OUTSIDE;
            assert!(!(px >= x && px < x + w && py >= y && py < y + h));
        }
    }

    #[test]
    fn back_taps_the_cancel_icon_when_a_menu_has_one() {
        // The Hip-O-Drome menu has the icon (Enable_CancelButton).
        let mut menu = title_menu(5);
        menu.cancel_button = true;
        let set = ButtonSet::from(&menu);
        assert_eq!(button_command(&set, 2, Role::Back), (2, Some(CANCEL_BUTTON)));
        // Without it, a menu that cancels on a tap outside gets that.
        let set = ButtonSet::from(&title_menu(3));
        assert_eq!(button_command(&set, 0, Role::Back), (0, Some(OUTSIDE)));
        menu.cancel_button = false;
        menu.outrange_cancel = false;
        assert_eq!(button_command(&ButtonSet::from(&menu), 0, Role::Back), (0, None));
    }

    #[test]
    fn a_message_without_buttons_advances_on_confirm() {
        let set = ButtonSet {
            rects: Vec::new(),
            enabled: Vec::new(),
            back: None,
            two_tap: false,
            selected: None,
        };
        assert_eq!(button_command(&set, 0, Role::Confirm), (0, Some(CENTER)));
        assert_eq!(button_command(&set, 0, Role::Down), (0, None));
    }

    #[test]
    fn button_menus_become_button_sets() {
        let set = ButtonSet::from(&title_menu(3));
        assert_eq!(set.rects[1], title_menu(3).button_rect(1));
        assert_eq!(set.back, Some(OUTSIDE));
    }

    // Sprite rectangles, from SysPrim_Touch_DrawRect.
    #[test]
    fn centred_sprites_are_tested_around_their_position() {
        assert_eq!(
            prim_rect(350.0, 120.0, 220.0, 44.0, true),
            (240.0, 98.0, 220.0, 44.0)
        );
    }

    #[test]
    fn other_sprites_are_tested_from_their_top_left() {
        assert_eq!(
            prim_rect(350.0, 120.0, 220.0, 44.0, false),
            (350.0, 120.0, 220.0, 44.0)
        );
    }

    // Palace_Main's hit tests (song-summoner-re.md §4).
    #[test]
    fn hip_o_drome_menu_buttons() {
        let menu = SCENE_BUTTONS
            .iter()
            .find(|g| g.main == "__Z11Palace_Mainv" && g.buttons.len() == 5)
            .unwrap();
        assert_eq!(menu.buttons, &[0x454, 0x458, 0x45c, 0x460, 0x464]);
        assert_eq!(menu.back, None);
    }

    #[test]
    fn hip_o_drome_confirmation_panel_comes_before_the_menu() {
        let palace: Vec<_> = SCENE_BUTTONS
            .iter()
            .filter(|g| g.main == "__Z11Palace_Mainv")
            .collect();
        // Create Trooper (+0x47c), No (+0x480), back icon (+0x48c).
        assert_eq!(palace[0].buttons, &[0x47c, 0x480]);
        assert_eq!(palace[0].back, Some(0x48c));
        assert_eq!(palace[1].buttons.len(), 5);
    }

    #[test]
    fn town_icon_row() {
        // TownMenu_Ctrl tests the shown sprites among seven at +0x60.
        let town = SCENE_BUTTONS
            .iter()
            .find(|g| g.main == "__Z9Town_Mainv")
            .unwrap();
        assert_eq!(town.buttons, &[0x60, 0x64, 0x68, 0x6c, 0x70, 0x74, 0x78]);
        assert_eq!(town.back, None);
    }

    // The world map's location menu (WorldmapMenu_Ctrl): three icons, 80
    // apart around (240, 160), and the back icon; a first tap on an icon
    // selects it, a tap on the selected one presses it.
    fn location_menu(selected: Option<usize>) -> ButtonSet {
        ButtonSet {
            rects: vec![
                (128.0, 128.0, 64.0, 64.0),
                (208.0, 128.0, 64.0, 64.0),
                (288.0, 128.0, 64.0, 64.0),
            ],
            enabled: vec![true; 3],
            back: Some((456.0, 296.0)),
            two_tap: true,
            selected,
        }
    }

    #[test]
    fn two_tap_moves_select_with_a_tap() {
        let set = location_menu(None);
        // The game shows the selected icon's description, so moving taps.
        assert_eq!(
            select_command(&set, 0, Role::NextSection),
            (1, vec![(240.0, 160.0)])
        );
        assert_eq!(
            select_command(&set, 0, Role::PrevSection),
            (2, vec![(320.0, 160.0)])
        );
    }

    #[test]
    fn two_tap_never_presses_by_moving() {
        // A single icon: moving stays on it, and it's already selected, so
        // a tap would press it.
        let mut set = location_menu(Some(0));
        set.enabled = vec![true, false, false];
        assert_eq!(select_command(&set, 0, Role::NextSection), (0, vec![]));
    }

    #[test]
    fn two_tap_confirm_presses_the_selected_icon() {
        // Selected: one tap presses it.
        assert_eq!(
            select_command(&location_menu(Some(1)), 1, Role::Confirm),
            (1, vec![(240.0, 160.0)])
        );
        // Not yet selected (or another is): select it, then press it.
        for selected in [None, Some(2)] {
            assert_eq!(
                select_command(&location_menu(selected), 1, Role::Confirm),
                (1, vec![(240.0, 160.0), (240.0, 160.0)])
            );
        }
        // Back is the back icon, which needs one tap.
        assert_eq!(
            select_command(&location_menu(None), 1, Role::Back),
            (1, vec![(456.0, 296.0)])
        );
    }

    #[test]
    fn skip_taps_script_mains_corner() {
        // Script_Main opens "Skip?" on a finger-up at x >= 440, y <= 40;
        // the SKIP art is drawn at (440, 12).
        let (x, y) = SKIP_BUTTON;
        assert!(x >= 440.0 && x <= 480.0);
        assert!(y >= 0.0 && y <= 40.0);
    }

    #[test]
    fn a_selected_slot_is_found_among_the_shown_buttons() {
        // A town showing slots 0, 2 and 5 of its seven: the game's
        // selection is a slot, the focus a position among those shown.
        let shown = [0, 2, 5];
        assert_eq!(selected_position(&shown, 5), Some(2));
        assert_eq!(selected_position(&shown, 0), Some(0));
        // Nothing selected, or a slot that isn't shown.
        assert_eq!(selected_position(&shown, -1), None);
        assert_eq!(selected_position(&shown, 1), None);
    }

    #[test]
    fn town_icons_select_before_they_press() {
        let town = SCENE_BUTTONS
            .iter()
            .find(|g| g.main == "__Z9Town_Mainv")
            .unwrap();
        assert_eq!(town.buttons, &[0x60, 0x64, 0x68, 0x6c, 0x70, 0x74, 0x78]);
        assert_eq!(town.selected, Some(0x14));
    }

    #[test]
    fn location_menu_is_the_world_maps_own_buttons() {
        let menu = SCENE_BUTTONS
            .iter()
            .find(|g| g.main == "__Z13Worldmap_Mainv")
            .unwrap();
        assert_eq!(menu.buttons, &[0x2f0, 0x2f4, 0x2f8]);
        assert_eq!(menu.back, Some(0x2fc));
        assert_eq!(menu.selected, Some(0x14));
    }

    #[test]
    fn a_back_sprite_is_what_back_taps() {
        let set = ButtonSet {
            rects: vec![(268.0, 188.0, 206.0, 44.0), (268.0, 243.0, 206.0, 44.0)],
            enabled: vec![true, true],
            back: Some(rect_center((428.0, 298.0, 48.0, 48.0))),
            two_tap: false,
            selected: None,
        };
        assert_eq!(button_command(&set, 1, Role::Back), (1, Some((452.0, 322.0))));
    }

    // The card list, from CardList_Flow_Select. The delete screen's strip
    // is centred at x = 240, icons along the bottom right.
    fn delete_list() -> CardList {
        CardList {
            status_icon: Some((292.0, 298.0, 52.0, 48.0)),
            exit_icon: Some((430.0, 298.0, 48.0, 48.0)),
            status_panel: false,
            centre_x: 240.0,
            icons: vec![
                (292.0, 298.0, 52.0, 48.0),
                (346.0, 298.0, 52.0, 48.0),
                (400.0, 298.0, 52.0, 48.0),
                (430.0, 298.0, 48.0, 48.0),
            ],
        }
    }

    #[test]
    fn dialog_outline_fits_the_button_art() {
        // Measured on "Welcome this trooper to the ranks?": the Yes
        // button's 157×64 touch area reaches past its 142×44 art on the
        // right and below.
        assert_eq!(
            dialog_button_art((256.0, 139.0, 157.0, 64.0)),
            (258.0, 143.0, 142.0, 44.0)
        );
    }

    #[test]
    fn outlines_stay_on_screen() {
        // Help's rows span the whole screen; the brackets' sides would be
        // drawn off its edges.
        assert_eq!(
            keep_on_screen((0.0, 143.0, 480.0, 56.0)),
            (4.0, 143.0, 472.0, 56.0)
        );
        // Likewise at the top and bottom.
        assert_eq!(
            keep_on_screen((100.0, -10.0, 50.0, 340.0)),
            (100.0, 4.0, 50.0, 312.0)
        );
        // Anything already clear of the edges is left alone.
        let button = (80.0, 131.0, 320.0, 54.0);
        assert_eq!(keep_on_screen(button), button);
    }

    #[test]
    fn dialog_outline_leaves_space_like_the_title_menu() {
        // The title menu's 320×54 button area leaves about 9 points either
        // side of its button art and 5 above and below; a dialog button's
        // outline leaves the same around its 142×44 art.
        let area = (256.0, 139.0, 157.0, 64.0);
        let (ax, ay, aw, ah) = dialog_button_art(area);
        assert_eq!(
            dialog_outline(area),
            (ax - 9.0, ay - 5.0, aw + 18.0, ah + 10.0)
        );
    }

    #[test]
    fn dialog_taps_land_on_the_button_art() {
        // Taps still go to the middle of the touch area; that has to be on
        // the art too.
        let area = (256.0, 139.0, 157.0, 64.0);
        let (x, y) = rect_center(area);
        let (ax, ay, aw, ah) = dialog_button_art(area);
        assert!(x > ax && x < ax + aw && y > ay && y < ay + ah);
    }

    #[test]
    fn card_outline_fits_the_middle_card() {
        // Measured on the Edit Trooper screen: the middle card, from its
        // "Bronze" header to the bottom of its song bar, is x 186-294,
        // y 57-222.
        let list = delete_list();
        assert_eq!(list.card_rect(), (186.0, 57.0, 108.0, 165.0));
        assert_eq!(list.focus_rect(CardFocus::Cards), list.card_rect());
        // Confirm taps inside it.
        let (_, tap) = card_list_command(&list, CardFocus::Cards, Role::Confirm);
        let (x, y) = tap.unwrap();
        let (cx, cy, cw, ch) = list.card_rect();
        assert!(x > cx && x < cx + cw && y > cy && y < cy + ch);
    }

    #[test]
    fn card_list_icons_are_button_sized() {
        for &icon in &delete_list().icons {
            assert!(is_card_icon(icon));
        }
        // Before the list is set up (and after), its icon slots all hold
        // sprite 0, the full-screen backdrop: that's no open card list.
        assert!(!is_card_icon((0.0, 0.0, 480.0, 320.0)));
    }

    #[test]
    fn left_and_right_tap_beside_the_middle_card() {
        let list = delete_list();
        let (_, left) = card_list_command(&list, CardFocus::Cards, Role::PrevSection);
        let (_, right) = card_list_command(&list, CardFocus::Cards, Role::NextSection);
        for (x, y) in [left.unwrap(), right.unwrap()] {
            // In the card band, more than 55 from the centre (one card),
            // not in the two-card edges, and clear of the status panel.
            assert!(y > 40.0 && y < 224.0);
            assert!((x - 240.0).abs() > 55.0);
            assert!(x > 60.0 && x < 420.0 && x > 160.0);
        }
        assert!(left.unwrap().0 < 240.0 && right.unwrap().0 > 240.0);
    }

    #[test]
    fn confirm_on_the_cards_taps_the_middle_card() {
        let (focus, tap) = card_list_command(&delete_list(), CardFocus::Cards, Role::Confirm);
        assert_eq!(focus, CardFocus::Cards);
        let (x, y) = tap.unwrap();
        assert!((x - 240.0).abs() <= 55.0 && y > 40.0 && y < 224.0);
    }

    #[test]
    fn down_goes_to_the_icons_and_up_comes_back() {
        let list = delete_list();
        assert_eq!(
            card_list_command(&list, CardFocus::Cards, Role::Down),
            (CardFocus::Icon(0), None)
        );
        assert_eq!(
            card_list_command(&list, CardFocus::Icon(0), Role::NextSection),
            (CardFocus::Icon(1), None)
        );
        assert_eq!(
            card_list_command(&list, CardFocus::Icon(3), Role::NextSection),
            (CardFocus::Icon(3), None)
        );
        assert_eq!(
            card_list_command(&list, CardFocus::Icon(2), Role::Up),
            (CardFocus::Cards, None)
        );
        assert_eq!(
            card_list_command(&list, CardFocus::Icon(1), Role::Confirm),
            (CardFocus::Icon(1), Some((372.0, 322.0)))
        );
    }

    #[test]
    fn back_taps_the_exit_icon() {
        let (_, tap) = card_list_command(&delete_list(), CardFocus::Cards, Role::Back);
        assert_eq!(tap, Some((454.0, 322.0)));
    }

    #[test]
    fn exit_and_status_icons_by_region() {
        // CardList_Main: region 5 (+0x3c8) toggles the status panel,
        // region 6 (+0x3c4) leaves the list.
        assert_eq!(CARD_LIST_STATUS_ICON, 0x3c8);
        assert_eq!(CARD_LIST_EXIT_ICON, 0x3c4);
        assert!(CARD_LIST_ICONS.contains(&CARD_LIST_STATUS_ICON));
        assert!(CARD_LIST_ICONS.contains(&CARD_LIST_EXIT_ICON));
    }

    #[test]
    fn card_list_icons_in_region_order() {
        // CardList_Flow_Select: regions 5, 6, 7, 8.
        assert_eq!(CARD_LIST_ICONS, [0x3c8, 0x3c4, 0x3cc, 0x3d0]);
    }

    #[test]
    fn a_drag_is_down_move_up_on_three_frames() {
        let mut queue = TapQueue::default();
        queue.drag(100.0, 150.0, -4.8);
        assert_eq!(
            frames(&mut queue, 4),
            vec![
                Some(TouchStep::Began(100.0, 150.0)),
                Some(TouchStep::Moved(100.0, 145.2, 100.0, 150.0)),
                Some(TouchStep::Ended(100.0, 145.2)),
                None,
            ]
        );
    }

    #[test]
    fn a_slide_is_down_move_across_up() {
        let mut queue = TapQueue::default();
        queue.push(Gesture::Slide {
            x: 300.0,
            y: 64.0,
            to_x: 320.0,
        });
        assert_eq!(
            frames(&mut queue, 4),
            vec![
                Some(TouchStep::Began(300.0, 64.0)),
                Some(TouchStep::Moved(320.0, 64.0, 300.0, 64.0)),
                Some(TouchStep::Ended(320.0, 64.0)),
                None,
            ]
        );
    }

    // The Options screen, from Option_Init/Option_Check (§4).
    fn options() -> Options {
        Options {
            work: 0x5000,
            // Volume 0.5.
            knob_x: 354.0,
            switches: [true, true, false],
            button: None,
            back: Some((432.0, 272.0, 48.0, 48.0)),
            ready: true,
        }
    }

    #[test]
    fn options_rows_are_slider_switches_then_back() {
        let rows = options().rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1], (368.0, 84.0, 88.0, 40.0));
        assert_eq!(rows[2], (368.0, 164.0, 88.0, 40.0));
        assert_eq!(rows[3], (368.0, 216.0, 88.0, 40.0));
        assert_eq!(rows[4], (432.0, 272.0, 48.0, 48.0));
    }

    #[test]
    fn options_up_down_move_and_stop_at_the_ends() {
        let o = options();
        assert_eq!(options_command(&o, 0, Role::Up), (0, None));
        assert_eq!(options_command(&o, 0, Role::Down), (1, None));
        assert_eq!(options_command(&o, 4, Role::Down), (4, None));
    }

    #[test]
    fn left_right_drag_the_volume_knob_a_tenth() {
        let o = options();
        let slide = |to_x| Some(Gesture::Slide { x: 354.0, y: 64.0, to_x });
        assert_eq!(options_command(&o, 0, Role::NextSection), (0, slide(374.0)));
        assert_eq!(options_command(&o, 0, Role::PrevSection), (0, slide(334.0)));
        // Nothing past full or silent.
        let mut full = options();
        full.knob_x = 454.0;
        assert_eq!(options_command(&full, 0, Role::NextSection), (0, None));
        let mut silent = options();
        silent.knob_x = 254.0;
        assert_eq!(options_command(&silent, 0, Role::PrevSection), (0, None));
    }

    #[test]
    fn confirm_flips_a_switch_off_its_knob() {
        let o = options();
        // BGM is on: its knob is on the right (412..457), so tap the left.
        let (_, tap) = options_command(&o, 1, Role::Confirm);
        assert_eq!(tap, Some(Gesture::Tap(390.0, 104.0)));
        // Lock orientation is off: its knob is on the left (368..413).
        let (_, tap) = options_command(&o, 3, Role::Confirm);
        assert_eq!(tap, Some(Gesture::Tap(434.0, 236.0)));
    }

    #[test]
    fn left_turns_a_switch_off_and_right_on() {
        let o = options();
        assert_eq!(options_command(&o, 1, Role::NextSection), (1, None));
        assert_eq!(
            options_command(&o, 1, Role::PrevSection),
            (1, Some(Gesture::Tap(390.0, 104.0)))
        );
        assert_eq!(options_command(&o, 3, Role::PrevSection), (3, None));
        assert_eq!(
            options_command(&o, 3, Role::NextSection),
            (3, Some(Gesture::Tap(434.0, 236.0)))
        );
    }

    // The Help screen, from Help2_Main (§4): rows 56 apart from y 32.
    fn help() -> Help {
        Help {
            work: 0x6000,
            page: false,
            tab: 0,
            count: 6,
            rows: (0..6).map(|i| (0.0, 32.0 + 56.0 * i as f32, 480.0, 56.0)).collect(),
            selected: None,
            settled: true,
            ready: true,
        }
    }

    #[test]
    fn help_rows_show_between_the_title_and_the_tabs() {
        let h = help();
        assert!(h.row_visible(0));
        assert!(h.row_visible(3)); // 200..256
        assert!(!h.row_visible(4)); // under the tabs
        let mut gliding = help();
        gliding.settled = false;
        assert!(!gliding.row_visible(0));
    }

    #[test]
    fn help_confirm_taps_a_shown_row_and_waits_for_a_hidden_one() {
        let h = help();
        assert_eq!(
            help_command(&h, 1, Role::Confirm),
            (1, Some(Some(Gesture::Tap(240.0, 116.0))))
        );
        assert_eq!(help_command(&h, 4, Role::Confirm), (4, None));
        assert_eq!(help_command(&h, 3, Role::Down), (4, Some(None)));
        assert_eq!(help_command(&h, 5, Role::Down), (5, Some(None)));
        assert_eq!(help_command(&h, 0, Role::Up), (0, Some(None)));
    }

    #[test]
    fn help_scrolls_a_row_into_view_with_a_short_drag() {
        let h = help();
        assert_eq!(h.scroll_toward(2), None);
        // Row 4 ends at 312: the rows must glide up 56 + 3, a tenth of that
        // as a finger-up move.
        let Some(Gesture::Drag { dy, .. }) = h.scroll_toward(4) else {
            panic!()
        };
        assert!((dy - -5.9).abs() < 1e-4, "{dy}");
        // And down for a row above the list.
        let mut scrolled = help();
        scrolled.rows[0].1 = -24.0;
        let Some(Gesture::Drag { dy, .. }) = scrolled.scroll_toward(0) else {
            panic!()
        };
        assert!(dy > 0.0);
    }

    #[test]
    fn help_tabs_on_left_right_and_shoulders_back_exits() {
        let mut h = help();
        let tab = |x: f32| (0, Some(Some(Gesture::Tap(x, 288.0))));
        assert_eq!(help_command(&h, 0, Role::NextSection), tab(120.0));
        assert_eq!(help_command(&h, 0, Role::NextTab), tab(120.0));
        // No tab left of the first, and the last before Exit stops right.
        assert_eq!(help_command(&h, 0, Role::PrevTab), (0, Some(None)));
        h.tab = 4;
        assert_eq!(help_command(&h, 0, Role::NextTab), (0, Some(None)));
        assert_eq!(help_command(&h, 0, Role::PrevSection), tab(280.0));
        // Exit is the sixth tab.
        assert_eq!(help_command(&h, 0, Role::Back), tab(440.0));
    }

    #[test]
    fn help_page_scrolls_turns_and_closes() {
        let mut h = help();
        h.page = true;
        h.rows.clear();
        h.selected = Some(0);
        let tap = |x, y| (0, Some(Some(Gesture::Tap(x, y))));
        // First item: no previous.
        assert_eq!(help_command(&h, 0, Role::PrevSection), (0, Some(None)));
        assert_eq!(help_command(&h, 0, Role::NextSection), tap(456.0, 160.0));
        h.selected = Some(5);
        assert_eq!(help_command(&h, 0, Role::PrevTab), tap(24.0, 160.0));
        assert_eq!(help_command(&h, 0, Role::NextTab), (0, Some(None)));
        assert_eq!(help_command(&h, 0, Role::Back), tap(440.0, 36.0));
        // Down: finger up 13.6, which glides the text on 136.
        let Some(Some(Gesture::Drag { dy, .. })) = help_command(&h, 0, Role::Down).1 else {
            panic!()
        };
        assert!((dy - -13.6).abs() < 1e-4, "{dy}");
    }

    #[test]
    fn help_ids_differ_by_tab_and_page() {
        let mut h = help();
        let list = h.id();
        h.tab = 1;
        assert_ne!(h.id(), list);
        h.page = true;
        assert_ne!(h.id(), list);
    }

    #[test]
    fn options_back_taps_the_back_icon() {
        let o = options();
        let back = Some(Gesture::Tap(456.0, 296.0));
        assert_eq!(options_command(&o, 2, Role::Back), (2, back));
        // So does confirm on it.
        assert_eq!(options_command(&o, 4, Role::Confirm), (4, back));
    }

    // The status panel: its icon (region 5) toggles it, a tap on the open
    // panel (region 4, x <= 160) flips its page.
    #[test]
    fn north_opens_the_status_panel_then_flips_it() {
        let mut list = delete_list();
        let (focus, tap) = card_list_command(&list, CardFocus::Cards, Role::Info);
        assert_eq!(focus, CardFocus::Cards);
        assert_eq!(tap, Some((318.0, 322.0)));
        list.status_panel = true;
        let (_, tap) = card_list_command(&list, CardFocus::Icon(2), Role::Info);
        assert!(tap.unwrap().0 <= 160.0);
    }

    #[test]
    fn back_closes_an_open_status_panel_first() {
        let mut list = delete_list();
        list.status_panel = true;
        let (_, tap) = card_list_command(&list, CardFocus::Cards, Role::Back);
        assert_eq!(tap, Some((318.0, 322.0)));
    }

    #[test]
    fn up_no_longer_goes_to_the_panel() {
        let mut list = delete_list();
        list.status_panel = true;
        assert_eq!(
            card_list_command(&list, CardFocus::Cards, Role::Up),
            (CardFocus::Cards, None)
        );
    }

    // SysMenu lists, from SysMenu_Check2 and menumain. Eight items, four
    // shown, rows 40 high.
    #[test]
    fn a_list_outline_stays_inside_the_panel() {
        // Battle's item window (seen 2026-09-28): the menu is at x 120,
        // 240 wide, but its rows start at 130 and are as wide as the menu,
        // so they overhang the panel on the right. The outline stops short
        // of the panel's edge, and short of the next row (the marker is
        // drawn a few points outside it).
        let row = (130.0, 88.0, 240.0, 40.0);
        assert_eq!(list_outline(row, Some((120.0, 240.0))), (130.0, 91.0, 226.0, 34.0));
        // No box read: just pulled in.
        let row = (245.0, 90.0, 230.0, 40.0);
        assert_eq!(list_outline(row, None), (245.0, 93.0, 226.0, 34.0));
        // A box that doesn't overlap the row (a misread) is ignored.
        let row = (130.0, 88.0, 240.0, 40.0);
        assert_eq!(list_outline(row, Some((500.0, 10.0))), (130.0, 91.0, 236.0, 34.0));
    }

    fn items(first: f32) -> ListMenu {
        let top = 90.0 - first * LIST_ROW_H;
        ListMenu {
            work: 0x4000,
            rows: (0..8)
                .map(|i| (245.0, top + i as f32 * LIST_ROW_H, 230.0, LIST_ROW_H))
                .collect(),
            enabled: vec![true; 8],
            first,
            shown: 4,
            settled: true,
            cursor: 0,
            span: None,
        }
    }

    #[test]
    fn list_rows_on_screen() {
        let list = items(2.0);
        assert!(!list.row_visible(1));
        assert!(list.row_visible(2));
        assert!(list.row_visible(5));
        assert!(!list.row_visible(6));
        // While it's still gliding, nothing counts as on screen.
        let mut moving = items(2.0);
        moving.settled = false;
        assert!(!moving.row_visible(3));
    }

    #[test]
    fn list_focus_moves_by_row_and_page() {
        let list = items(0.0);
        assert_eq!(list_command(&list, 0, Role::Down), (1, Some(None)));
        assert_eq!(list_command(&list, 0, Role::Up), (0, Some(None)));
        assert_eq!(list_command(&list, 7, Role::Down), (7, Some(None)));
        assert_eq!(list_command(&list, 1, Role::NextSection), (5, Some(None)));
        assert_eq!(list_command(&list, 6, Role::NextSection), (7, Some(None)));
        assert_eq!(list_command(&list, 5, Role::PrevSection), (1, Some(None)));
    }

    #[test]
    fn confirm_taps_a_row_only_once_its_on_screen() {
        let list = items(0.0);
        assert_eq!(
            list_command(&list, 2, Role::Confirm),
            (2, Some(Some((360.0, 190.0))))
        );
        // Row 6 is below the list: wait for the scroll.
        assert_eq!(list_command(&list, 6, Role::Confirm), (6, None));
    }

    #[test]
    fn back_taps_off_the_list() {
        assert_eq!(list_command(&items(0.0), 3, Role::Back), (3, Some(Some(OUTSIDE))));
    }

    #[test]
    fn scrolling_drags_toward_the_focused_row() {
        // Row 6 is below: the finger goes up, which moves the list on.
        match items(0.0).scroll_toward(6) {
            Some(Gesture::Drag { dy, .. }) => assert!(dy < 0.0),
            other => panic!("{other:?}"),
        }
        // Row 0 is above: the finger goes down.
        match items(3.0).scroll_toward(0) {
            Some(Gesture::Drag { dy, .. }) => assert!(dy > 0.0),
            other => panic!("{other:?}"),
        }
        // On screen, or still gliding: no drag.
        assert_eq!(items(0.0).scroll_toward(2), None);
        let mut moving = items(0.0);
        moving.settled = false;
        assert_eq!(moving.scroll_toward(6), None);
    }

    #[test]
    fn a_drag_step_scrolls_about_one_row() {
        // menumain: speed (drag / 24) slowed 10% a frame, snapped to a
        // whole row once below 0.08.
        let mut speed = LIST_DRAG / 24.0;
        let mut position = 0.0;
        while speed >= 0.08 {
            position += speed;
            speed *= 0.9;
        }
        assert_eq!((position as f32).round(), 1.0);
    }

    #[test]
    fn sort_panel_is_over_the_card_list() {
        let sort = SCENE_BUTTONS
            .iter()
            .find(|g| g.main == "__Z13Teammake_Mainv")
            .unwrap();
        assert_eq!(sort.buttons, &[0x4c, 0x50, 0x54, 0x58, 0x5c, 0x60]);
        assert_eq!(sort.back, Some(0x3c));
        assert!(sort.over_cards);
    }

    // The world map (song-summoner-re.md, World map): entries 0-6 of
    // `_worldmap_symbol`, with their road links (those to entries outside
    // this set left out), all open, standing on 1, camera at the bottom
    // edge at full zoom.
    fn world() -> WorldMap {
        let table: [(f32, f32, &[usize]); 7] = [
            (262.0, 797.0, &[]),
            (347.0, 748.0, &[0, 2, 4]),
            (402.0, 697.0, &[3, 6]),
            (472.0, 764.0, &[2]),
            (234.0, 675.0, &[5]),
            (153.0, 630.0, &[4]),
            (354.0, 555.0, &[2]),
        ];
        WorldMap {
            work: 0x2000,
            symbols: table
                .iter()
                .map(|&(x, y, links)| WorldSymbol {
                    x,
                    y,
                    open: true,
                    links: links.to_vec(),
                })
                .collect(),
            current: 1,
            highlighted: 1,
            camera: (400.0, 672.0),
            zoom: 1.0,
            ready: true,
        }
    }

    #[test]
    fn world_screen_points_follow_the_games_camera() {
        let mut map = world();
        // Full zoom: one map point per screen point, camera at the centre.
        assert_eq!(map.to_screen(1), (187.0, 236.0));
        // Zoomed out to 0.5, Check_SymbolTouch's scale is 3 - 2 * 0.5 = 2.
        map.zoom = 0.5;
        assert_eq!(map.to_screen(1), (213.5, 198.0));
    }

    #[test]
    fn world_screen_points_hit_their_location() {
        for zoom in [1.0, 0.75, 0.5] {
            let mut map = world();
            map.zoom = zoom;
            for i in 0..map.symbols.len() {
                if map.on_screen(i) {
                    assert_eq!(map.symbol_at(map.to_screen(i)), Some(i), "{i} at {zoom}");
                }
            }
        }
    }

    #[test]
    fn world_hit_test_is_the_games() {
        let map = world();
        // Check_SymbolTouch's rect: 36 either side, 24 above and below,
        // edges included.
        let (x, y) = map.to_screen(1);
        assert_eq!(map.symbol_at((x + 36.0, y + 24.0)), Some(1));
        assert_eq!(map.symbol_at((x + 37.0, y + 24.0)), None);
        // Only open locations count.
        let mut closed = world();
        closed.symbols[1].open = false;
        assert_eq!(closed.symbol_at((x, y)), None);
    }

    #[test]
    fn world_dpad_follows_roads_to_the_next_open_location() {
        let map = world();
        assert_eq!(map.neighbour(1, (0.0, -1.0)), Some(2));
        assert_eq!(map.neighbour(1, (1.0, 0.0)), Some(2));
        assert_eq!(map.neighbour(1, (-1.0, 0.0)), Some(0));
        // Roads work both ways: 0 lists none, but 1 lists 0.
        assert_eq!(map.neighbour(0, (1.0, 0.0)), Some(1));
    }

    #[test]
    fn world_dpad_passes_through_closed_locations() {
        let mut map = world();
        map.symbols[2].open = false;
        // Up from 1 goes on past 2 to 6.
        assert_eq!(map.neighbour(1, (0.0, -1.0)), Some(6));
        for dir in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
            assert_ne!(map.neighbour(1, dir), Some(2));
        }
    }

    #[test]
    fn world_dpad_off_the_roads_takes_the_nearest_that_way() {
        let map = world();
        // 0's only road leads to 1, which is too far right of up; the
        // nearest open location above is 4.
        assert_eq!(map.neighbour(0, (0.0, -1.0)), Some(4));
        // Nothing left of 5.
        assert_eq!(map.neighbour(5, (-1.0, 0.0)), None);
    }

    #[test]
    fn world_shoulders_cycle_through_open_locations() {
        let mut map = world();
        map.symbols[2].open = false;
        assert_eq!(map.cycle(1, true), Some(3));
        assert_eq!(map.cycle(6, true), Some(0));
        assert_eq!(map.cycle(0, false), Some(6));
        // Just the one open: nowhere to go.
        for s in &mut map.symbols {
            s.open = false;
        }
        map.symbols[1].open = true;
        assert_eq!(map.cycle(1, true), None);
    }

    #[test]
    fn world_commands_pick_a_target() {
        let map = world();
        assert_eq!(world_target(&map, Role::Up), Some((2, false)));
        assert_eq!(world_target(&map, Role::NextTab), Some((2, false)));
        // Confirm taps the highlighted location again.
        assert_eq!(world_target(&map, Role::Confirm), Some((1, true)));
        assert_eq!(world_target(&map, Role::Back), None);
    }

    #[test]
    fn world_step_taps_a_location_on_screen() {
        let map = world();
        assert_eq!(
            world_step(&map, 2, false, None),
            WorldStep::Tap {
                at: map.to_screen(2),
                again: false
            }
        );
        // Confirming the highlighted one: one tap.
        assert_eq!(
            world_step(&map, 1, true, None),
            WorldStep::Tap {
                at: map.to_screen(1),
                again: false
            }
        );
        // Confirming one a pan un-highlighted: the first tap highlights it,
        // so it takes another.
        assert_eq!(
            world_step(&map, 2, true, None),
            WorldStep::Tap {
                at: map.to_screen(2),
                again: true
            }
        );
    }

    #[test]
    fn world_step_never_taps_by_accident() {
        let mut map = world();
        // Tapping the highlighted location again would confirm it.
        assert_eq!(world_step(&map, 1, false, None), WorldStep::Done);
        map.symbols[2].open = false;
        assert_eq!(world_step(&map, 2, false, None), WorldStep::Done);
        // An earlier open location over the target would take the tap.
        let mut stacked = world();
        stacked.symbols[0].x = stacked.symbols[2].x;
        stacked.symbols[0].y = stacked.symbols[2].y;
        assert_eq!(stacked.symbol_at(stacked.to_screen(2)), Some(0));
        assert_eq!(world_step(&stacked, 2, false, None), WorldStep::Done);
    }

    #[test]
    fn world_step_pans_to_a_location_off_screen() {
        let map = world();
        // 5 is at x = 153, 247 left of the camera: its rect is off the left
        // edge. A flick moves the camera by twice the finger's movement,
        // the other way, in whole points.
        assert!(!map.on_screen(5));
        let WorldStep::Pan(pan) = world_step(&map, 5, false, None) else {
            panic!("no pan");
        };
        assert_eq!(pan, Gesture::Pan { x: 240.0, y: 160.0, dx: 123.0, dy: 21.0 });
        let mut panned = world();
        panned.camera = (400.0 - 2.0 * 123.0, 672.0 - 2.0 * 21.0);
        assert!(panned.on_screen(5));
        // If the last pan didn't move the camera (the map's edge), give up.
        assert_eq!(
            world_step(&map, 5, false, Some(map.camera)),
            WorldStep::Done
        );
    }

    #[test]
    fn world_pans_keep_the_finger_on_screen() {
        let mut map = world();
        map.symbols[6].y = -2000.0;
        let WorldStep::Pan(Gesture::Pan { x, y, dx, dy }) = world_step(&map, 6, false, None)
        else {
            panic!("no pan");
        };
        let (to_x, to_y) = (x + dx, y + dy);
        assert!((0.0..=480.0).contains(&to_x) && (0.0..=320.0).contains(&to_y));
    }

    #[test]
    fn a_pan_is_one_move_in_two_directions() {
        let mut queue = TapQueue::default();
        queue.push(Gesture::Pan { x: 240.0, y: 160.0, dx: 123.0, dy: 21.0 });
        assert_eq!(
            frames(&mut queue, 4),
            vec![
                Some(TouchStep::Began(240.0, 160.0)),
                Some(TouchStep::Moved(363.0, 181.0, 240.0, 160.0)),
                Some(TouchStep::Ended(363.0, 181.0)),
                None,
            ]
        );
    }
}
