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
//! - Battle's unit select (the map with the status panel): a virtual finger
//!   rests on the cursor's tile, so the game draws its own cursor there
//!   (under the units) and its status panel shows the unit under it. The
//!   D-pad slides it along the grid's axes, panning the map with a flick
//!   when a tile is out of reach; confirm lets go, which acts on the tile
//!   (the game picks a held finger's unit by tile, a tap's by sprite); the
//!   shoulders tap the curved arrows (previous/next unit) and Start taps
//!   MENU, after letting go where nothing happens. Where the finger can't
//!   reach (the bottom rows), touchHLE draws a gold diamond instead. An
//!   enemy's status (after picking one): confirm or back close it, north
//!   flips its page.
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
use super::pad::Role;
use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::gles::present::{FocusMarker, FocusShape};
use crate::mem::{ConstPtr, MutPtr, Ptr};
use crate::objc::{id, msg, nil, Override, SEL};
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
/// Frames the play look hides a menu's or dialog's highlight after the
/// controller presses one of its buttons: the game animates the pressed
/// button (5 frames), then usually closes it, and a menu under a dialog
/// just answered showed its highlight again, on its first button, as it
/// closed (2026-09-28). A menu that stays gets it back after this.
const PRESS_HIDE_FRAMES: u64 = 20;
/// Frames presses may wait with none of them used before they're dropped.
/// A screen that ignores the D-pad (an attack playing out, a message) left
/// them queued, and they moved the cursor once the player backed out
/// (2026-09-28). Real waits (a camera pan, a scroll, a hold) are shorter.
const STALE_COMMAND_FRAMES: u32 = 30;

/// Counts frames the queued presses sit unused (see
/// [STALE_COMMAND_FRAMES]).
#[derive(Default)]
pub struct CommandAge {
    frames: u32,
}

impl CommandAge {
    /// After a frame's handling, with the queue's length before and after:
    /// whether the queue has waited too long and should be dropped.
    pub fn stale(&mut self, before: usize, after: usize) -> bool {
        if after == 0 || after < before {
            self.frames = 0;
            return false;
        }
        self.frames += 1;
        if self.frames > STALE_COMMAND_FRAMES {
            self.frames = 0;
            return true;
        }
        false
    }
}

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
/// row: `SysMenu_Check`/`Check2` make it a speed of dy / 24 rows a frame,
/// which `menumain` slows by 10% a frame, then below 0.08 "snaps" by
/// pushing toward the nearest row. That push keeps adding up, so a glide
/// that stops far from a row overshoots: 4.8 ended a row the *wrong* way
/// after 47 frames (the shop, 2026-09-28 log). 2.7 to 3.8 all land one row
/// on; 3.5 does it in 10 frames (see `tests::menumain_step`).
const LIST_DRAG: f32 = 3.5;
/// The same for a list with a cursor (Edit Troopers' item list,
/// 2026-09-29 log), which never snaps: it just glides, covering 10x its
/// first frame's dy / 24 rows, so this many points a row lands it on one.
const LIST_COAST_DRAG: f32 = 2.4;
/// Below this speed (rows a frame) a list that doesn't snap counts as
/// still. It only gets near 0 (1e-10 after 200 frames), so waiting for
/// exactly 0 stalled the controller. By here under 0.12 of a row (5
/// points) is left, which it finishes by itself; a one-row scroll takes
/// about 20 frames rather than the 30 it took at 0.004.
const LIST_STILL: f32 = 0.012;
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

    /// Push a gesture if there is one.
    pub fn extend(&mut self, gesture: Option<Gesture>) {
        if let Some(gesture) = gesture {
            self.push(gesture);
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

    /// Down long enough for `Tactics_CtrlTest` to call it a hold, when the
    /// game shows its own cursor under it.
    pub fn held(&self) -> bool {
        matches!(self.down, Some((_, frames)) if frames >= MIN_STILL_FRAMES)
    }

    /// Drop the finger without an Ended (as [TapQueue::clear]), e.g. when
    /// the phase it was held in has gone. Returns whether the game had it
    /// down: then its touch state must be cleared too (see [drop_finger]).
    #[must_use]
    pub fn clear(&mut self) -> bool {
        self.steps.clear();
        self.down.take().is_some()
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

/// The enabled button a direction moves to from `from`, by where the
/// buttons are on screen, not their order (the game lists a dialog's
/// buttons right to left). Left and right go to the nearest button that
/// way and stop at the last, so a dialog's left is always its left button
/// (the user's rule, 2026-09-28: wrapping made the focus look like it was
/// on the other button). Up and down do the same in a column but wrap
/// round at its ends. "That way" is within 45° of the direction, so a row
/// has nothing above it and a column nothing beside it. Stays put when
/// nothing is that way.
pub fn step_toward(rects: &[(f32, f32, f32, f32)], enabled: &[bool], from: usize, role: Role) -> usize {
    let (dx, dy) = match role {
        Role::Up => (0.0, -1.0),
        Role::Down => (0.0, 1.0),
        Role::PrevSection => (-1.0, 0.0),
        Role::NextSection => (1.0, 0.0),
        _ => return from,
    };
    let Some(&here) = rects.get(from) else {
        return from;
    };
    let (hx, hy) = rect_center(here);
    // Each other enabled button: how far along the direction it is, and
    // how far off to the side.
    let others: Vec<(usize, f32, f32)> = rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from && enabled.get(i).copied().unwrap_or(false))
        .map(|(i, &rect)| {
            let (x, y) = rect_center(rect);
            let along = (x - hx) * dx + (y - hy) * dy;
            let side = ((x - hx) * dy - (y - hy) * dx).abs();
            (i, along, side)
        })
        .collect();
    // The nearest, keeping to the line; for the wrap, the furthest the
    // other way (the most negative `along`).
    let best = |keep: &dyn Fn(f32, f32) -> bool| {
        others
            .iter()
            .filter(|&&(_, along, side)| keep(along, side))
            .min_by(|a, b| (a.1 + 2.0 * a.2).total_cmp(&(b.1 + 2.0 * b.2)))
            .map(|&(i, _, _)| i)
    };
    if let Some(i) = best(&|along, side| along > 0.5 && side <= along) {
        return i;
    }
    if dy == 0.0 {
        return from;
    }
    best(&|along, side| along < -0.5 && side <= -along).unwrap_or(from)
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

/// Drop dialogs with no buttons when something with buttons is open too:
/// those are captions over a menu (the world map's "Select a map for
/// battle." over its location list), and taking the focus they'd make
/// confirm tap the middle of the screen instead of the menu. On their own
/// they're messages, which confirm taps through.
pub fn without_captions(widgets: Vec<Widget>) -> Vec<Widget> {
    let caption = |w: &Widget| matches!(w, Widget::Dialog(d) if d.rects.is_empty());
    if widgets.iter().all(caption) {
        return widgets;
    }
    widgets.into_iter().filter(|w| !caption(w)).collect()
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

/// Where a stretched `button_001.png` frame (menu buttons, the sort
/// panel's rows) is drawn in a button's rectangle. The 144×56 frame is
/// opaque from 4 to 140 across and 4 to 49 down, with a soft shadow below
/// (measured from the texture, 2026-09-28), so the play look's copy of the
/// highlight covers the button, not its transparent edges.
pub fn button_art((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (left, top) = (w * 4.0 / 144.0, h * 4.0 / 56.0);
    let (right, bottom) = (w * 4.0 / 144.0, h * 7.0 / 56.0);
    (x + left, y + top, w - left - right, h - top - bottom)
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
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection => {
            (step_toward(&set.rects, &set.enabled, index, role), None)
        }
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
    match role {
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection => {}
        // Selected, it's pressed by one tap; else the first only selects it.
        Role::Confirm if set.selected == Some(index) => return (index, vec![rect_center(rect)]),
        Role::Confirm => return (index, vec![rect_center(rect); 2]),
        Role::Back => return (index, set.back.into_iter().collect()),
        _ => return (index, Vec::new()),
    }
    let next = step_toward(&set.rects, &set.enabled, index, role);
    // Never tap the selected button by moving: that would press it. Not
    // moving (the end of a row) taps nothing either.
    if set.selected == Some(next) || next == index {
        return (next, Vec::new());
    }
    (next, vec![rect_center(set.rects[next])])
}

/// The Results screen (`Result_Main`, work through the task table): its flow
/// (`+0x0`, `Result_Main`'s switch at 0x47fc4) while the pearls are split
/// (`ResultFlow_DivideSelect`, 0x46ba4), and that flow's sub-state (`+0x4`,
/// switch at 0x46bbc) while it takes touches: 0 sets up, 2 a message, 3 the
/// rank-up dialog, 4 EXIT's animation.
const RESULT_DIVIDE_FLOW: u32 = 13;
const RESULT_DIVIDE_TAKING_TOUCHES: u32 = 1;
/// The EXIT icon: slot 4 (x 412-460, y 228-300), which leaves on one tap.
const RESULT_EXIT: (f32, f32) = (436.0, 264.0);

/// The pearl split's troopers as a two-tap button set: `count` (`+0x1c`, at
/// most four) slots along the bottom, a finger-up at x 156 + 64i to
/// 204 + 64i, y 228 to 300 (`DivideSelect`'s hit test), the selected one
/// `selected` (`+0xc`). A tap on another selects it (SE 2, its status); on
/// the selected one, it asks to rank it up (or says why it can't). EXIT is
/// back, not a slot the D-pad reaches, since one tap on it leaves.
pub fn result_divide_set(count: usize, selected: i32) -> ButtonSet {
    let count = count.min(4);
    let rects = (0..count)
        .map(|i| (156.0 + 64.0 * i as f32, 228.0, 48.0, 72.0))
        .collect();
    ButtonSet {
        rects,
        enabled: vec![true; count],
        back: Some(RESULT_EXIT),
        two_tap: true,
        selected: usize::try_from(selected).ok().filter(|&i| i < count),
    }
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
    /// `menumain` ends a glide on a whole row, which it only does with
    /// the cursor at -1 (the shop's lists). With a cursor (Edit
    /// Troopers' item list) it coasts to wherever the glide runs out.
    pub snaps: bool,
    /// The menu's x (`+0xd08`), where its highlight starts (its rows
    /// start a little further right).
    pub menu_x: Option<f32>,
    /// The game's own highlight sprite.
    pub highlight: ListHighlight,
    /// Where back taps: off the rows, the list's cancel (`SysMenu_Check2`
    /// returns −2); or a screen's own back icon where a tap off the rows
    /// does nothing (the shop's older `SysMenu_Check`).
    pub back: (f32, f32),
}

/// A `SysMenu`'s row highlight (the sprite at `+0xd28`, opened by
/// `SysMenu_Open` untextured in [LIST_HIGHLIGHT_COLOR]), which the game
/// shows under a finger held on a row (`SysMenu_Check2` →
/// `SysMenu_Disp_Cursor`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListHighlight {
    /// Its size, once it has one.
    pub size: Option<(f32, f32)>,
    /// Placed by its centre (`_prim_work + 0x38`).
    pub centred: bool,
    /// The game is showing it.
    pub shown: bool,
}

/// Where the game would put its highlight for `row`: at the menu's x and
/// one point above the row (`SysMenu_Disp_Cursor`), at its own size (the
/// row's until it has one).
pub fn list_highlight(
    (rx, ry, rw, rh): (f32, f32, f32, f32),
    menu_x: Option<f32>,
    highlight: &ListHighlight,
) -> (f32, f32, f32, f32) {
    let (w, h) = highlight
        .size
        .filter(|&(w, h)| w > 0.0 && h > 0.0)
        .unwrap_or((rw, rh));
    prim_rect(menu_x.unwrap_or(rx), ry - 1.0, w, h, highlight.centred)
}

/// The controller's marker on a list row: a copy of the game's highlight,
/// or nothing while the game's own shows (a finger on a row), so the two
/// never add up.
/// What the play look does with a screen's outline. The debug look
/// (`--controller-debug`) always shows the outline as the screen made it.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Clean {
    /// Show it as it is (a copy of the game's own highlight already, or
    /// a tile the game can't show its cursor on).
    #[default]
    Same,
    /// Nothing: the game shows the selection itself (the title drum's
    /// band, the centred card, its own tile cursor).
    Hide,
    /// A copy of the game's own highlight on the same rectangle, for
    /// screens with no resting selected look of their own.
    Highlight,
    /// The same, on another rectangle: a dialog button's art, where the
    /// outline leaves space around it.
    HighlightAt((f32, f32, f32, f32)),
}

/// The outline to draw, in the play look or the debug look.
pub fn play_look(marker: Option<FocusMarker>, clean: Clean, debug: bool) -> Option<FocusMarker> {
    if debug {
        return marker;
    }
    match clean {
        Clean::Same => marker,
        Clean::Hide => None,
        Clean::Highlight => marker.map(|(rect, _)| (rect, FocusShape::Highlight)),
        Clean::HighlightAt(rect) => marker.map(|_| (rect, FocusShape::Highlight)),
    }
}

pub fn list_marker(
    row: (f32, f32, f32, f32),
    menu_x: Option<f32>,
    highlight: &ListHighlight,
) -> Option<FocusMarker> {
    (!highlight.shown).then(|| (list_highlight(row, menu_x, highlight), FocusShape::Highlight))
}

impl ListMenu {
    /// Row `i` is on screen, and the list is still.
    pub fn row_visible(&self, i: usize) -> bool {
        let first = self.first.round().max(0.0) as usize;
        self.settled && i >= first && i < first + self.shown
    }

    /// The nearest row to `i` that's on screen, for a list that won't
    /// scroll (see [State::list_scroll]).
    pub fn clamp_to_shown(&self, i: usize) -> usize {
        let Some(last_row) = self.rows.len().checked_sub(1) else {
            return i;
        };
        let first = (self.first.round().max(0.0) as usize).min(last_row);
        let last = (first + self.shown.max(1) - 1).min(last_row);
        i.clamp(first, last)
    }

    /// A drag scrolling the list a row toward row `i`, if it's off screen
    /// and the list is still.
    pub fn scroll_toward(&self, i: usize) -> Option<Gesture> {
        if !self.settled || self.row_visible(i) || self.rows.is_empty() {
            return None;
        }
        let first = (self.first.round().max(0.0) as usize).min(self.rows.len() - 1);
        let (_, y) = rect_center(self.rows[first]);
        // Down just left of the rows: `SysMenu_Check` (the shop's list)
        // makes a row under a finger going down the cursor and clears the
        // touch state, which may be what kept its drags from scrolling.
        let x = (self.rows[first].0 / 2.0).max(1.0);
        // Finger up moves the list on to later rows (the speed is
        // -dy / 24).
        let dy = if self.snaps {
            if i >= first {
                -LIST_DRAG
            } else {
                LIST_DRAG
            }
        } else {
            // The nearest whole-row view with row i in it, reached in one
            // coast from wherever the list is, even between rows.
            let target = if i >= first { i + 1 - self.shown.max(1) } else { i };
            -(target as f32 - self.first) * LIST_COAST_DRAG
        };
        Some(Gesture::Drag { x, y, dy })
    }
}

/// A list's scroll speed is slow enough to count as still. A snapping
/// list does stop at 0 (and passes through slow speeds when its snap
/// turns back), so for it only 0 counts.
pub fn list_still(speed: f32, snaps: bool) -> bool {
    if snaps {
        speed == 0.0
    } else {
        speed.abs() < LIST_STILL
    }
}

/// What to do with a list's focus while the list is still: `follow` (a
/// command moved it) scrolls toward it; otherwise an off-screen focus was
/// left there by a touch scroll, so it moves onto the rows shown rather
/// than dragging the list back (seen 2026-09-29).
pub fn list_keep_focus(list: &ListMenu, focus: usize, follow: bool) -> (usize, Option<Gesture>) {
    if !list.settled || list.row_visible(focus) {
        (focus, None)
    } else if follow {
        (focus, list.scroll_toward(focus))
    } else {
        (list.clamp_to_shown(focus), None)
    }
}

/// What a scroll drag did to a list, once it's still again.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollOutcome {
    /// It moved the way it was meant to.
    Moved,
    /// It moved the other way.
    Reversed,
    /// It didn't move.
    Still,
}

/// Judge a scroll drag from the list's position before and after it, and
/// whether it was meant to bring on later rows.
pub fn judge_scroll(before: f32, after: f32, wanted_later: bool) -> ScrollOutcome {
    if after == before {
        ScrollOutcome::Still
    } else if (after > before) == wanted_later {
        ScrollOutcome::Moved
    } else {
        ScrollOutcome::Reversed
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
        // A tap off every row is the list's cancel (SysMenu_Check2: -2), or
        // the screen's back icon.
        Role::Back => (focus, Some(Some(list.back))),
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

/// How far a flick moves the finger to turn a shop quantity drum one digit.
/// `SysDrum_Check` makes it a speed of -dy / 24 digits a frame, which
/// `drummenumain` slows by 20% a frame and, below 0.1, nudges to the
/// nearest digit (song-summoner-re.md, The shop). Worked through, 5 to 7.5
/// turn it one digit either way; 6 takes 8 frames. The controller checks
/// the digit after each flick anyway.
const DIAL_DRAG: f32 = 6.0;

/// One of the drums of the shop's quantity dial (the older `SysDrum`,
/// task main `drummenumain`).
#[derive(Clone, Debug, PartialEq)]
pub struct DialDrum {
    pub work: u32,
    /// Where it takes touches (`+0x1610` position, `+0x1614` size).
    pub rect: (f32, f32, f32, f32),
    /// The digit it's on (the item at the cursor `+2`: its `+0x38`).
    pub digit: i32,
    /// It has stopped (`SysDrum_CheckCursordisp`: no speed, no nudge).
    pub settled: bool,
    /// Where the game draws its own band on it (the cursor sprite, id at
    /// `+0x1624`, a `button_001.png` frame), if it's there.
    pub band: Option<(f32, f32, f32, f32)>,
}

/// The shop's quantity dial: its drums, most significant (leftmost) first.
/// The amount is tens × 10 + ones (`ShopFlow_BuyMenu`, 0x13ac4).
#[derive(Clone, Debug, PartialEq)]
pub struct Dial {
    pub drums: Vec<DialDrum>,
}

impl Dial {
    pub fn digits(&self) -> Vec<i32> {
        self.drums.iter().map(|d| d.digit).collect()
    }

    pub fn settled(&self) -> bool {
        self.drums.iter().all(|d| d.settled)
    }

    /// The outline for drum `i`: its middle band, where the chosen digit
    /// shows.
    pub fn band(&self, i: usize) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.drums[i].rect;
        (x, y + h / 2.0 - 18.0, w, 36.0)
    }

    /// Where the play look's copy of the highlight goes on drum `i`: over
    /// the game's own band (drawn on every drum), so the picked one shows
    /// brighter; the middle band if the band sprite isn't there.
    pub fn highlight(&self, i: usize) -> (f32, f32, f32, f32) {
        self.drums[i].band.map_or_else(|| self.band(i), button_art)
    }
}

/// The dialog button a dial command presses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DialPress {
    /// Confirm (the right button).
    Confirm,
    /// Forget it (the left button).
    Forget,
}

/// What a command does on the dial (the user's layout, 2026-09-28):
/// left/right pick a drum, up/down turn the picked one's target digit
/// the way the drum shows it: up brings in the digit above (one less),
/// down the one below (one more), wrapping as the drum does (the user's
/// choice, 2026-09-28). Confirm and back press the dialog's
/// Confirm and Forget it. Returns the new focus and the press, if any.
pub fn dial_command(
    focus: usize,
    target: &mut [i32],
    role: Role,
) -> (usize, Option<DialPress>) {
    let last = target.len().saturating_sub(1);
    match role {
        Role::PrevSection => (focus.saturating_sub(1), None),
        Role::NextSection => ((focus + 1).min(last), None),
        Role::Up | Role::Down => {
            if let Some(digit) = target.get_mut(focus) {
                let step = if role == Role::Up { -1 } else { 1 };
                *digit = (*digit + step).rem_euclid(10);
            }
            (focus, None)
        }
        Role::Confirm => (focus, Some(DialPress::Confirm)),
        Role::Back => (focus, Some(DialPress::Forget)),
        _ => (focus, None),
    }
}

/// The next flick to bring the dial to `target`, the leftmost drum that's
/// off first, the short way round; `None` while it's turning or once it's
/// there. A finger going up turns a drum to the next digit.
pub fn dial_step(dial: &Dial, target: &[i32]) -> Option<Gesture> {
    if !dial.settled() {
        return None;
    }
    let (drum, &want) = dial
        .drums
        .iter()
        .zip(target)
        .find(|&(drum, &want)| drum.digit != want)?;
    let forward = (want - drum.digit).rem_euclid(10) <= 5;
    let (x, y) = rect_center(drum.rect);
    let dy = if forward { -DIAL_DRAG } else { DIAL_DRAG };
    Some(Gesture::Drag { x, y, dy })
}

/// Where a dial press taps: the dialog's rightmost button for Confirm,
/// its leftmost for Forget it.
pub fn dial_press_point(dialog: &Dialog, press: DialPress) -> Option<(f32, f32)> {
    let by_x = |a: &&(f32, f32, f32, f32), b: &&(f32, f32, f32, f32)| a.0.total_cmp(&b.0);
    let rect = match press {
        DialPress::Confirm => dialog.rects.iter().max_by(by_x),
        DialPress::Forget => dialog.rects.iter().min_by(by_x),
    }?;
    Some(rect_center(*rect))
}

/// A button was pressed recently enough that the play look still hides
/// its menu's highlight (see [PRESS_HIDE_FRAMES]).
pub fn just_pressed(pressed_at: Option<u64>, frame: u64) -> bool {
    pressed_at.is_some_and(|at| frame < at + PRESS_HIDE_FRAMES)
}

/// The play look for touchHLE's diamond: only on a tile a finger can't be
/// held on (`holdable` false) once the camera can't pan any further to
/// bring it in (`stuck`, a pan that didn't move it). Until then the finger
/// is on its way and the game's own cursor follows it; showing it in
/// between flashed an orange diamond as a unit was picked (2026-09-28).
pub fn diamond_look(holdable: bool, stuck: bool) -> Clean {
    if !holdable && stuck {
        Clean::Same
    } else {
        Clean::Hide
    }
}

/// Where to tap to select a select-then-press screen's first item when it
/// opens, if that's due: nothing selected yet, the controller in use (a
/// finger chooses for itself), not done already this opening, and the
/// item enabled.
pub fn preselect_tap(
    set: &ButtonSet,
    index: usize,
    pad_mode: bool,
    done: bool,
) -> Option<(f32, f32)> {
    if !set.two_tap || set.selected.is_some() || !pad_mode || done {
        return None;
    }
    if set.enabled.get(index) != Some(&true) {
        return None;
    }
    set.rects.get(index).copied().map(rect_center)
}

/// The controller's own state on the dial.
#[derive(Clone, Debug, PartialEq)]
pub struct DialState {
    /// The dial (its first drum's work), so a new one starts afresh.
    pub work: u32,
    /// The drum picked.
    pub focus: usize,
    /// The digits the drums are being turned to.
    pub target: Vec<i32>,
    /// Each drum's band as last seen: the game hides it while the drum
    /// turns, and falling back to the middle band then made the copy grow
    /// on every step (2026-09-28).
    pub bands: Vec<Option<(f32, f32, f32, f32)>>,
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
/// How far a scroll drag's move takes Help's list or page before
/// [help_stop] stops it. `Help2_Main` sets the speed to the move, adds the
/// speed to the position every frame (0x60e36), 10% less each time, and
/// zeroes it when a finger goes down: the move's frame and the next (the
/// finger-up's) add 1.9 times the move before the stop's finger-down.
/// (Left to glide, it would be ten times, over some 45 frames.)
const HELP_DRAG_GAIN: f32 = 1.9;

/// The touch that stops a Help scroll drag's glide: down where the drag
/// ended (which zeroes the speed), a 1-point slide sideways (so its
/// finger-up is a flick, which opens nothing; a swipe takes 24), up.
pub fn help_stop(drag: Gesture) -> Option<Gesture> {
    match drag {
        Gesture::Drag { x, y, dy } => Some(Gesture::Slide {
            x,
            y: y + dy,
            to_x: x + 1.0,
        }),
        _ => None,
    }
}
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
            dy: -need / HELP_DRAG_GAIN,
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
            Role::Up => (focus, scroll(HELP_PAGE_STEP / HELP_DRAG_GAIN)),
            Role::Down => (focus, scroll(-HELP_PAGE_STEP / HELP_DRAG_GAIN)),
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

/// The shop's password keyboard (`Keyboard_Main`, song-summoner-re.md, The
/// shop's password keyboard): `getkeybord`'s table (`keyrect`, 0x6ea48) of
/// x, y, width, height and the key, in screen points (the table's y is
/// 160 higher up). A finger-up types the key the finger was last on; the
/// game shows I and O as 1 and 0.
pub const KEYBOARD_KEYS: [(f32, f32, f32, f32, u8); 39] = [
    // Digits.
    (9.0, 164.0, 39.0, 32.0, b'1'),
    (56.0, 164.0, 39.0, 32.0, b'2'),
    (103.0, 164.0, 39.0, 32.0, b'3'),
    (150.0, 164.0, 39.0, 32.0, b'4'),
    (197.0, 164.0, 39.0, 32.0, b'5'),
    (244.0, 164.0, 39.0, 32.0, b'6'),
    (291.0, 164.0, 39.0, 32.0, b'7'),
    (338.0, 164.0, 39.0, 32.0, b'8'),
    (385.0, 164.0, 39.0, 32.0, b'9'),
    (432.0, 164.0, 39.0, 32.0, b'0'),
    // Q to P (I and O show as 1 and 0).
    (9.0, 204.0, 39.0, 32.0, b'Q'),
    (56.0, 204.0, 39.0, 32.0, b'W'),
    (103.0, 204.0, 39.0, 32.0, b'E'),
    (150.0, 204.0, 39.0, 32.0, b'R'),
    (197.0, 204.0, 39.0, 32.0, b'T'),
    (244.0, 204.0, 39.0, 32.0, b'Y'),
    (291.0, 204.0, 39.0, 32.0, b'U'),
    (338.0, 204.0, 39.0, 32.0, b'1'),
    (385.0, 204.0, 39.0, 32.0, b'0'),
    (432.0, 204.0, 39.0, 32.0, b'P'),
    // A to L, then Backspace.
    (9.0, 244.0, 39.0, 32.0, b'A'),
    (56.0, 244.0, 39.0, 32.0, b'S'),
    (103.0, 244.0, 39.0, 32.0, b'D'),
    (150.0, 244.0, 39.0, 32.0, b'F'),
    (197.0, 244.0, 39.0, 32.0, b'G'),
    (244.0, 244.0, 39.0, 32.0, b'H'),
    (291.0, 244.0, 39.0, 32.0, b'J'),
    (338.0, 244.0, 39.0, 32.0, b'K'),
    (385.0, 244.0, 39.0, 32.0, b'L'),
    (432.0, 244.0, 39.0, 32.0, KEY_BACKSPACE),
    // The bottom row is set in, between the wider quit and Enter keys.
    (9.0, 285.0, 51.0, 30.0, KEY_QUIT),
    (80.0, 284.0, 39.0, 32.0, b'Z'),
    (127.0, 284.0, 39.0, 32.0, b'X'),
    (174.0, 284.0, 39.0, 32.0, b'C'),
    (221.0, 284.0, 39.0, 32.0, b'V'),
    (268.0, 284.0, 39.0, 32.0, b'B'),
    (315.0, 284.0, 39.0, 32.0, b'N'),
    (362.0, 284.0, 39.0, 32.0, b'M'),
    (419.0, 285.0, 51.0, 30.0, KEY_ENTER),
];
/// Deletes the last character.
pub const KEY_BACKSPACE: u8 = b'b';
/// Checks the password (`Keyboard_Main` state 2).
pub const KEY_ENTER: u8 = b'e';
/// Asks whether to stop entering the password (a `SysDialog`).
pub const KEY_QUIT: u8 = b'r';

fn key_rect(i: usize) -> (f32, f32, f32, f32) {
    let (x, y, w, h, _) = KEYBOARD_KEYS[i];
    (x, y, w, h)
}

fn key_index(key: u8) -> Option<usize> {
    KEYBOARD_KEYS.iter().position(|k| k.4 == key)
}

fn key_point(key: u8) -> Option<(f32, f32)> {
    Some(rect_center(key_rect(key_index(key)?)))
}

/// The password keyboard key a real key types (desktop; SDL scancode
/// names): letters and digits, keypad digits, Backspace, and Return or
/// Keypad Enter for Enter. The game has no I or O (its keys there are 1
/// and 0), so those type 1 and 0. Other keys keep their mapping.
pub fn typed_key(name: &str) -> Option<u8> {
    match name {
        "Backspace" => return Some(KEY_BACKSPACE),
        "Return" | "Keypad Enter" => return Some(KEY_ENTER),
        _ => {}
    }
    let name = name.strip_prefix("Keypad ").unwrap_or(name);
    let &[c] = name.as_bytes() else {
        return None;
    };
    match c {
        b'I' => Some(b'1'),
        b'O' => Some(b'0'),
        b'A'..=b'Z' | b'0'..=b'9' => Some(c),
        _ => None,
    }
}

/// One command on the password keyboard with key `focus` focused and
/// `typed` characters entered: the new focus, and where to tap. The D-pad
/// moves as on any buttons ([step_toward]), confirm types the key, and
/// back is Backspace, or with nothing typed the key that asks to leave.
pub fn keyboard_command(focus: usize, typed: usize, role: Role) -> (usize, Option<(f32, f32)>) {
    let focus = focus.min(KEYBOARD_KEYS.len() - 1);
    match role {
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection => {
            let rects: Vec<_> = (0..KEYBOARD_KEYS.len()).map(key_rect).collect();
            let enabled = [true; KEYBOARD_KEYS.len()];
            (step_toward(&rects, &enabled, focus, role), None)
        }
        Role::Confirm => (focus, Some(rect_center(key_rect(focus)))),
        Role::Back if typed > 0 => (focus, key_point(KEY_BACKSPACE)),
        Role::Back => (focus, key_point(KEY_QUIT)),
        _ => (focus, None),
    }
}

/// Where the shop's top menu (`ShopFlow_MenuSelect`) also takes a tap: a
/// finger-up at x 40-192, y 108 or more (over the shopkeeper) opens the
/// password entry. There's no button for it: the Info button taps it,
/// and the Setup menu's HUD shows a pill saying so (the user's choice,
/// 2026-09-28).
const SHOP_PASSWORD_TAP: (f32, f32) = (116.0, 204.0);

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
    /// The shop's password keyboard (its work), with how many characters
    /// are typed.
    Keyboard { work: u32, typed: usize },
}

impl Widget {
    pub fn work(&self) -> u32 {
        match self {
            Widget::World(map) => map.work,
            Widget::Help(help) => help.id(),
            Widget::Options(options) => options.work,
            Widget::Buttons(menu) => menu.work,
            Widget::Dialog(dialog) => dialog.work,
            Widget::Scene { work, .. } | Widget::Cards { work, .. } | Widget::Keyboard { work, .. } => *work,
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
    /// `drummenumain`, the older `SysDrum` (the shop's quantity dial); 0
    /// if missing.
    drum_main: u32,
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
    /// `_tactics_script_flag` (set while a battle script runs, in any
    /// phase) and `_mesmanage` (`+0x0` the "tap to continue" mark's
    /// `SysAnim`, `+0x4` it's enabled); 0 if missing.
    script_flag: u32,
    mesmanage: u32,
    /// The Results screen's and the shop's task mains, and the password
    /// keyboard's; 0 if missing.
    result_main: u32,
    shop_main: u32,
    keyboard_main: u32,
    /// `SysTouch_Clear`: forgets every finger (count, down, flick) without
    /// a finger-up, for a held virtual finger that has to go.
    touch_clear: GuestFunction,
    task_manage: u32,
    prim_work: u32,
    card_list_work: u32,
    /// Each [SCENE_BUTTONS] group's main function, resolved (0 if this
    /// copy of the game lacks it).
    scene_mains: [u32; SCENE_GROUPS],
    /// Holds the offset of `MainView`'s `m_mode` ivar.
    m_mode_offset: ConstPtr<u32>,
}

/// What the battle handlers keep between frames
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
    /// Unit select: the game's cursor last seen (to follow it when the
    /// game moves it: the arrows, a selection, a new turn), whether our own
    /// lift or pan is what moved it last (a release puts it back on the
    /// selected unit, which ours ignores), and a tap to make once the
    /// finger is up (the arrows, MENU).
    seen: Option<Tile>,
    ours: bool,
    pending_tap: Option<(f32, f32)>,
}

#[derive(Default)]
pub struct State {
    taps: TapQueue,
    /// Controller commands waiting for the next frame, where the menus
    /// they act on are read.
    commands: VecDeque<Role>,
    /// The password keyboard was up last frame, and keys typed on a real
    /// keyboard waiting to be tapped on it ([typed_key]).
    password_keyboard: bool,
    typed_keys: VecDeque<u8>,
    /// Android's on-screen keyboard is up for it.
    soft_keyboard: bool,
    /// The shop's top menu is up (for the Password pill).
    shop_menu: bool,
    /// How long they've waited unused.
    command_age: CommandAge,
    /// On the shop's quantity dial: the drum picked and the target digits.
    dial: Option<DialState>,
    /// What the play look does with this frame's outline, set by the
    /// screen that made it.
    clean_look: Clean,
    /// A select-then-press screen (its work) whose first item the
    /// controller has selected, as a first tap would, so it's done once.
    preselected: Option<u32>,
    /// The frame the controller last pressed a menu's or dialog's button
    /// (see [PRESS_HIDE_FRAMES]); cleared by any other command.
    pressed_at: Option<u64>,
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
    /// A `SysMenu` list (its work), its scroll position when the last
    /// scroll drag was made, and whether that drag was meant to bring on
    /// later rows. Once the list is still again it's judged
    /// ([judge_scroll]); a list that didn't move gets no more drags, and
    /// the focus stays on the rows shown.
    list_scroll: Option<(u32, f32, bool)>,
    /// A command moved a list's focus (or is waiting to tap it), so the
    /// list scrolls to it; until then an off-screen focus was left there
    /// by a touch scroll, and goes to the rows shown ([list_keep_focus]).
    list_follow: bool,
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
    /// Whether the confirm button is down now (battle's confirm holds a
    /// finger for as long as it is).
    confirm_down: bool,
    /// The held finger of the battle screens.
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
        drum_main: optional("__ZL12drummenumainv") & !1,
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
        touch_clear: thumb(get("__Z14SysTouch_Clearv")?),
        script_flag: optional("_tactics_script_flag"),
        result_main: optional("__Z11Result_Mainv") & !1,
        shop_main: optional("__Z9Shop_Mainv") & !1,
        keyboard_main: optional("__Z13Keyboard_Mainv") & !1,
        mesmanage: optional("_mesmanage"),
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

/// While a list's scroll drag is being judged, log each frame what the
/// list and the game's touch state (`_touch_work`) do: the shop's list
/// didn't scroll for one (2026-09-28).
fn log_list_drag(env: &Environment, game: Game, widgets: &[Widget]) {
    let state = &env.framework_state.song_summoner.game_input;
    if !cfg!(debug_assertions) || state.list_scroll.is_none() || game.touch_work == 0 {
        return;
    }
    let Some(list) = widgets.iter().find_map(|w| match w {
        Widget::List(list) => Some(list),
        _ => None,
    }) else {
        return;
    };
    let t = game.touch_work;
    log!(
        "input: list after drag (frame {}): at {}, speed {}, cursor {}; touch down {} flick {} began {} ended {} count {}",
        state.frame,
        read_f32(env, list.work + 0xd18),
        read_f32(env, list.work + 0xd1c),
        read_i8(env, list.work + 2),
        read_u32(env, t + 0x10),
        read_u32(env, t + 0x14),
        read_u32(env, t + 0x54),
        read_u32(env, t + 0x58),
        read_u32(env, t + 0xc)
    );
}

/// The shop's quantity dial, if one is open: every running `drummenumain`
/// task's drum, left to right.
fn read_dial(env: &Environment, game: Game) -> Option<Dial> {
    if game.drum_main == 0 {
        return None;
    }
    let mut drums: Vec<DialDrum> = (0..TASK_SLOTS)
        .map(|slot| game.task_manage + slot * TASK_SLOT_SIZE)
        .filter(|&base| {
            read_u32(env, base) == TASK_RUNNING
                && read_u32(env, base + 0x10) & !1 == game.drum_main
        })
        .map(|base| read_u32(env, base + 0x18))
        .filter(|&work| work >= 0x1000)
        .filter_map(|work| {
            let count = read_i8(env, work + 4);
            let cursor = read_i8(env, work + 2);
            if count <= 0 || cursor < 0 || cursor >= count {
                return None;
            }
            let digit = read_u32(env, work + 0x38 + cursor as u32 * 0x2c) as i32;
            Some(DialDrum {
                work,
                rect: (
                    f32::from(read_i16(env, work + 0x1610)),
                    f32::from(read_i16(env, work + 0x1612)),
                    f32::from(read_i16(env, work + 0x1614)),
                    f32::from(read_i16(env, work + 0x1616)),
                ),
                digit,
                settled: read_f32(env, work + 0x1620) == 0.0
                    && read_f32(env, work + 0x1628).abs() < 1.0e-5,
                band: read_prim_rect(env, game, read_u32(env, work + 0x1624)),
            })
        })
        .collect();
    if drums.is_empty() {
        return None;
    }
    drums.sort_by(|a, b| a.rect.0.total_cmp(&b.rect.0));
    Some(Dial { drums })
}

fn read_i8(env: &Environment, addr: u32) -> i8 {
    env.mem.read(ConstPtr::<i8>::from_bits(addr))
}

fn read_list(env: &Environment, prim_work: u32, work: u32) -> Option<ListMenu> {
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
    let cursor = read_i8(env, work + 2);
    let snaps = cursor < 0;
    Some(ListMenu {
        work,
        rows,
        enabled,
        first: read_f32(env, work + 0xd18),
        shown: read_i8(env, work + 5).max(1) as usize,
        settled: list_still(read_f32(env, work + 0xd1c), snaps),
        cursor: (cursor.max(0) as usize).min(count as usize - 1),
        snaps,
        menu_x: Some(f32::from(read_i16(env, work + 0xd08))),
        highlight: read_list_highlight(env, prim_work, read_u32(env, work + 0xd28)),
        back: OUTSIDE,
    })
}

/// The game's list highlight sprite `id` (see [ListHighlight]), read from
/// the sprite table whether it's shown or not.
fn read_list_highlight(env: &Environment, prim_work: u32, id: u32) -> ListHighlight {
    // Ids are small table indices; anything else means a misread.
    if prim_work == 0 || id >= 0x1000 {
        return ListHighlight {
            size: None,
            centred: false,
            shown: false,
        };
    }
    let entry = prim_work + id * PRIM_SIZE;
    let in_use = read_u32(env, entry) != 0;
    ListHighlight {
        size: in_use.then(|| (read_f32(env, entry + 0x14), read_f32(env, entry + 0x18))),
        centred: read_u32(env, entry + 0x38) != 0,
        shown: in_use && read_u32(env, entry + 8) != 0,
    }
}

/// The shop (`Shop_Main`, work through the task table): its flow (`+0x0`,
/// `Shop_Main`'s switch at 0x14274) while buying or selling
/// (`ShopFlow_BuyMenu`, `_SellMenu`), whose lists are the older
/// `SysMenu_Check` (a tap off the rows does nothing) and whose back icon
/// is the sprite at `+0x20` (both hit-test it with `SysPrim_Touch_DrawRect`).
const SHOP_BUY: u32 = 6;
const SHOP_SELL: u32 = 7;

/// The shop's top menu (`ShopFlow_MenuSelect`) is up: flow 3, sub-state
/// (`+4`) 1 once its `SysButtonMenu` is open.
const SHOP_MENU: u32 = 3;
const SHOP_MENU_TAKING_TOUCHES: u32 = 1;

fn shop_top_menu(env: &Environment, game: Game) -> bool {
    if game.shop_main == 0 {
        return false;
    }
    task_work(env, game, game.shop_main)
        .is_some_and(|work| read_u32(env, work) == SHOP_MENU && read_u32(env, work + 4) == SHOP_MENU_TAKING_TOUCHES)
}

/// The password keyboard's work (`Keyboard_Main`): `+0` its state (1
/// takes touches; others check the password or show a dialog), and the
/// byte count of characters typed at `+0x30c` (35 at most).
const KEYBOARD_TAKING_TOUCHES: u32 = 1;
const KEYBOARD_TYPED: u32 = 0x30c;

/// Where back taps on the shop's buy or sell list: its back icon.
fn shop_list_back(env: &Environment, game: Game) -> Option<(f32, f32)> {
    if game.shop_main == 0 {
        return None;
    }
    let work = task_work(env, game, game.shop_main)?;
    if !matches!(read_u32(env, work), SHOP_BUY | SHOP_SELL) {
        return None;
    }
    read_prim_rect(env, game, read_u32(env, work + 0x20)).map(rect_center)
}

/// The Results screen's pearl split, while it takes touches.
fn result_divide(env: &Environment, game: Game) -> Option<Widget> {
    if game.result_main == 0 {
        return None;
    }
    let work = task_work(env, game, game.result_main)?;
    if read_u32(env, work) != RESULT_DIVIDE_FLOW
        || read_u32(env, work + 4) != RESULT_DIVIDE_TAKING_TOUCHES
    {
        return None;
    }
    let count = read_u32(env, work + 0x1c) as usize;
    if count == 0 {
        return None;
    }
    let selected = read_u32(env, work + 0xc) as i32;
    Some(Widget::Scene {
        work: work + RESULT_DIVIDE_FLOW,
        set: result_divide_set(count, selected),
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

/// A battle script is running (`_tactics_script_flag`): `Tactics_Main`
/// runs the script loop instead of the phase.
fn battle_script(env: &Environment, game: Game) -> bool {
    game.script_flag != 0 && read_u32(env, game.script_flag) != 0
}

/// A message's "tap to continue" mark shows (`SysMessageKeyMark`: its
/// `SysAnim` id at `_mesmanage + 0x0`, enabled at `+0x4`).
fn message_waiting(env: &Environment, game: Game) -> bool {
    if game.mesmanage == 0 || game.anim_work == 0 {
        return false;
    }
    let id = read_u32(env, game.mesmanage) as i32;
    let enabled = read_u32(env, game.mesmanage + 4) == 1;
    let slot = |offset: u32| match u32::try_from(id) {
        Ok(id) if id < 512 => read_u32(env, game.anim_work + id * 0x5c + offset),
        _ => 0,
    };
    enabled && battle::anim_visible(id, slot(0), slot(0xc))
}

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

/// Unit select (phase 19), on a held finger like move select (the user's
/// choice, 2026-09-28): a finger rests on the cursor's tile, so the game
/// draws its own cursor there (under the units) and its status panel shows
/// the unit under it. The D-pad slides it along the grid (panning when a
/// tile is out of the finger's reach), confirm lets go (acting on the tile
/// as a tap would: the game picks a held finger's unit by tile, a tap's by
/// sprite, see battle::holdable), and the shoulders (the curved arrows),
/// Start (MENU) and the triggers (zoom) first let go on a tile with no unit,
/// which does nothing. Too low to hold (the map's bottom rows), the cursor
/// is touchHLE's diamond and confirm taps a unit there.
///
/// Everything waits while the game isn't taking touches (unit select's own
/// state isn't 1, an animation), the camera is moving or a zoom animates,
/// so touches land on what the player saw.
fn unit_select(env: &mut Environment, game: Game, battle: &Battle) -> Option<FocusMarker> {
    use battle::UnitSelectIntent as Intent;

    // The finger rests only while the controller is in use: a real touch
    // takes over (touch_used), and the next button press brings it back.
    if !env.framework_state.song_summoner.game_input.pad_mode {
        return None;
    }

    let select_state = read_u32(env, game.unit_select);
    // State 3: another team's unit's status is open (after letting go on
    // one). A still tap at x < 160 (the stats panel) flips its page, one
    // further right closes it; the finger must go down and up on the same
    // side. Here confirm closes it too: an enemy's status is only a glance.
    if select_state == UNIT_SELECT_STATUS {
        let state = &mut env.framework_state.song_summoner.game_input;
        if state.taps.is_idle() && state.finger.finger().is_none() {
            if let Some(role) = state.commands.pop_front() {
                let role = if role == Role::Confirm { Role::Back } else { role };
                if let Some((x, y)) = battle::status_command(role) {
                    state.taps.tap(x, y);
                }
            }
        }
        return None;
    }
    let taking = select_state == UNIT_SELECT_TAKING_TOUCHES;
    let menu = read_prim_rect(env, game, read_u32(env, game.unit_select + 0x24)).map(rect_center);
    let camera = battle.camera;
    let grid = &battle.grid;
    // Letting go on a unit acts (selects it, opens its ring or status);
    // anywhere else the game's cursor just goes back to the selected unit.
    let units: Vec<Tile> = grid.tiles().filter(|&t| grid.unit_at(t).is_some()).collect();
    let game_cursor = Some(battle.cursor).filter(|&t| grid.contains(t));

    let state = &mut env.framework_state.song_summoner.game_input;
    let still = state.battle_input.origin == Some(camera.origin);
    state.battle_input.origin = Some(camera.origin);
    let zoom = take_zoom(&mut state.commands, battle, state.battle_input.zoom);
    state.battle_input.zoom = zoom;
    let idle = state.taps.is_idle() && state.finger.is_idle();
    let finger = state.finger.finger();
    let input = &mut state.battle_input;

    // Follow the game's cursor when the game moves it, but not while our
    // finger is down (the game's cursor follows the finger) or just after
    // our own lift or pan.
    if finger.is_some() || !idle {
        input.seen = game_cursor;
    } else if std::mem::take(&mut input.ours) {
        input.seen = game_cursor;
    } else if game_cursor.is_some() && game_cursor != input.seen {
        input.seen = game_cursor;
        input.cursor = game_cursor;
    }
    let mut cursor = input
        .cursor
        .filter(|&t| grid.contains(t))
        .or(game_cursor)
        .unwrap_or((grid.w / 2, grid.h / 2));

    let ready = taking && still && !battle.zooming && idle;
    let reason = (!ready).then(|| {
        format!(
            "state {}, camera still {}, zooming {}, finger or tap under way {}",
            select_state, still, battle.zooming, !idle
        )
    });
    log_waiting(state, "unit select", reason);
    let input = &mut state.battle_input;
    if !ready {
        input.cursor = Some(cursor);
        let stuck = state.battle_input.last_pan == Some(camera.origin);
        return diamond(
            &camera,
            &state.finger,
            cursor,
            battle::UNIT_SELECT_HOLD,
            stuck,
            &mut state.clean_look,
        );
    }

    while let Some(role) = state.commands.pop_front() {
        match battle::unit_select_intent(role) {
            Intent::Move(dir) => {
                cursor = battle::grid_step(grid, cursor, dir);
                input.act = false;
            }
            Intent::Confirm => {
                input.act = true;
                break;
            }
            Intent::PrevUnit => {
                input.pending_tap = Some(battle::PREV_UNIT_ARROW);
                break;
            }
            Intent::NextUnit => {
                input.pending_tap = Some(battle::NEXT_UNIT_ARROW);
                break;
            }
            Intent::Menu => {
                input.pending_tap = menu;
                break;
            }
            Intent::None => {}
        }
    }

    let view = battle::FollowView {
        camera: &camera,
        grid,
        accepted: &units,
        cursor: Some(cursor),
        finger,
        want_down: input.pending_tap.is_none() && input.zoom.is_none(),
        act: input.act,
        last_pan: input.last_pan,
        hold_area: battle::UNIT_SELECT_HOLD,
        harmless_area: battle::TAP_AREA,
    };
    let mut zoom_now = None;
    match battle::follow(&view) {
        battle::FollowAction::Wait => {
            // The finger is up as wanted: the arrows' or MENU's tap, or the
            // zoom.
            if finger.is_none() {
                if let Some((x, y)) = input.pending_tap.take() {
                    log!("input: unit select, tapping {:?}", (x, y));
                    state.taps.tap(x, y);
                } else {
                    zoom_now = input.zoom.take();
                }
            }
        }
        battle::FollowAction::Press((x, y)) => {
            log!("input: unit select, pressing on tile {:?} at {:?}", cursor, (x, y));
            input.last_pan = None;
            state.finger.press(x, y);
        }
        battle::FollowAction::Slide((x, y)) => {
            log!("input: unit select, sliding to tile {:?} at {:?}", cursor, (x, y));
            state.finger.slide_to(x, y);
        }
        battle::FollowAction::Lift => {
            log!(
                "input: unit select, letting go on tile {:?} (unit {:?}, acting {})",
                cursor,
                grid.unit_at(cursor),
                input.act
            );
            input.act = false;
            input.ours = true;
            state.finger.lift();
        }
        battle::FollowAction::LiftAt((x, y)) => {
            log!(
                "input: unit select, letting go where nothing happens: tile {:?} at {:?}",
                camera.hold_tile_at((x, y)),
                (x, y)
            );
            input.ours = true;
            state.finger.slide_to(x, y);
            state.finger.lift();
        }
        battle::FollowAction::Pan(dx, dy) => {
            log!("input: unit select, panning by {:?} toward tile {:?}", (dx, dy), cursor);
            input.ours = true;
            input.last_pan = Some(camera.origin);
            let (x, y) = battle::TAP_CENTRE;
            state.taps.push(Gesture::Pan { x, y, dx, dy });
        }
        battle::FollowAction::Tap((x, y)) => {
            input.act = false;
            // Only a unit can be picked by a tap; on empty ground it shows
            // nothing.
            if battle::confirm_picks(grid, cursor) {
                log!(
                    "input: unit select, tapping tile {:?} at {:?} (too low to hold)",
                    cursor,
                    (x, y)
                );
                state.taps.tap(x, y);
            }
        }
        battle::FollowAction::Stuck => {
            input.act = false;
        }
    }
    input.cursor = Some(cursor);
    let stuck = state.battle_input.last_pan == Some(camera.origin);
    let outline = diamond(
        &camera,
        &state.finger,
        cursor,
        battle::UNIT_SELECT_HOLD,
        stuck,
        &mut state.clean_look,
    );
    if let Some(percent) = zoom_now {
        apply_zoom(env, game, percent);
    }
    outline
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

    if !(DRAW_TILE_GRID || env.options.controller_debug) {
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
            widgets.extend(read_list(env, game.prim_work, work).map(Widget::List));
            continue;
        }
        if game.keyboard_main != 0 && main == game.keyboard_main {
            if read_u32(env, work) == KEYBOARD_TAKING_TOUCHES {
                let typed = env.mem.read(ConstPtr::<u8>::from_bits(work + KEYBOARD_TYPED));
                widgets.push(Widget::Keyboard {
                    work,
                    typed: usize::from(typed),
                });
            }
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
    // The shop's buy and sell lists go back by its back icon.
    if let Some(back) = shop_list_back(env, game) {
        for widget in &mut widgets {
            if let Widget::List(list) = widget {
                list.back = back;
            }
        }
    }
    if widgets.is_empty() {
        widgets.extend(result_divide(env, game));
    }
    if widgets.is_empty() {
        widgets.extend(world_map(env, game).map(Widget::World));
    }
    widgets
}

/// A key on a real keyboard (desktop), before its mapping: while the
/// password keyboard is up, letters, digits, Backspace and Return type on
/// it ([typed_key]). Returns true if it took the key (press and release).
pub fn handle_typing(env: &mut Environment, key: &str, pressed: bool) -> bool {
    if !super::setup::device_keyboard(env) {
        return false;
    }
    let state = &mut env.framework_state.song_summoner.game_input;
    if !state.password_keyboard {
        return false;
    }
    // The on-screen keyboard's text input is on (Android): its text
    // events type, so the key itself mustn't too.
    if state.soft_keyboard {
        return false;
    }
    let Some(typed) = typed_key(key) else {
        return false;
    };
    // A few ahead at most, like the pad's commands.
    if pressed && state.typed_keys.len() < 8 {
        state.typed_keys.push_back(typed);
    }
    true
}

/// Text from Android's on-screen keyboard (SDL text input), while it's up
/// for the password keyboard: each character it types, Backspace and
/// Return, the same as [typed_key]. Returns true if it took the event.
pub fn handle_text(env: &mut Environment, event: &crate::window::TextInputEvent) -> bool {
    use crate::window::TextInputEvent;
    if super::setup::is_open(env) {
        return false;
    }
    let state = &mut env.framework_state.song_summoner.game_input;
    if !state.soft_keyboard {
        return false;
    }
    let keys: Vec<u8> = match event {
        TextInputEvent::Backspace => vec![KEY_BACKSPACE],
        TextInputEvent::Return => vec![KEY_ENTER],
        TextInputEvent::Text(text) => text
            .chars()
            .filter_map(|c| typed_key(&c.to_ascii_uppercase().to_string()))
            .collect(),
    };
    for key in keys {
        if state.typed_keys.len() < 8 {
            state.typed_keys.push_back(key);
        }
    }
    true
}

/// The shop's top menu is up and the controller or keyboard is in use:
/// show the Password pill.
pub fn password_pill(env: &Environment) -> bool {
    let state = &env.framework_state.song_summoner.game_input;
    state.shop_menu && state.pad_mode
}

/// Android: show the on-screen keyboard while the password keyboard is up
/// and the player chose to type on it; hide it after. SDL's text input,
/// as for `UITextField`, which keeps the app focused.
/// The Setup menu is opening: put away the on-screen keyboard, which would
/// cover it and keep its keys from the menu. The game's next frame brings
/// it back if the password keyboard is still up.
pub fn hide_soft_keyboard(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.game_input;
    if !std::mem::take(&mut state.soft_keyboard) {
        return;
    }
    log!("input: hiding the on-screen keyboard for the Setup menu");
    env.on_parent_stack_in_coroutine(|window, _| window.stop_text_input());
}

fn update_soft_keyboard(env: &mut Environment) {
    if !cfg!(target_os = "android") {
        return;
    }
    let want = env.framework_state.song_summoner.game_input.password_keyboard
        && super::setup::device_keyboard(env);
    let state = &mut env.framework_state.song_summoner.game_input;
    if want == state.soft_keyboard {
        return;
    }
    state.soft_keyboard = want;
    log!("input: {} the on-screen keyboard", if want { "showing" } else { "hiding" });
    env.on_parent_stack_in_coroutine(move |window, _| {
        if want {
            window.start_text_input();
        } else {
            window.stop_text_input();
        }
    });
}

/// The Setup menu is opening and takes every release until it closes: let
/// go of a held Confirm now, or battle's finger would stay down.
pub fn release_held(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.game_input;
    state.confirm_down = false;
    state.taps.keep_down(false);
}

/// A controller button or key (already mapped to its command) that the
/// picker didn't take.
pub fn handle_role(env: &mut Environment, role: Role, pressed: bool) {
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
    // A real finger takes over from the held virtual one. This runs before
    // the game hears of the touch, so its touch state is clear by then.
    if let Some(game) = game(env) {
        drop_finger(env, game, "a real touch");
    }
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.pad_mode {
        state.pad_mode = false;
        if let Some(window) = env.window.as_mut() {
            window.set_focus_marker(None);
        }
    }
}

/// Drop the held virtual finger. If the game has it down, clear the game's
/// touch state too (`SysTouch_Clear`): `SysTouch_Began_F1` adds to a finger
/// count that only a finger-up or a clear resets, so the next finger, a real
/// one included, would be a second one (a pinch). A finger-up instead would
/// act (letting go on a unit opens its ring).
fn drop_finger(env: &mut Environment, game: Game, why: &str) {
    if env.framework_state.song_summoner.game_input.finger.clear() {
        log!("input: battle, dropping the held finger ({})", why);
        () = game.touch_clear.call_from_host(env, ());
    }
}

/// A battle phase's sub-phase while it runs (`Tactics_Main` calls a phase's
/// Start in 0x64, its Main in 0x65 and its End in 0x66; seen at 0x4684).
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
            let script = battle_script(env, game);
            let message = message_waiting(env, game);
            let owner = battle::owner_now(battle.phase, script, message);
            if battle_tutorial(env, game) {
                stand_down(env, game, "a tutorial");
            } else if script || message {
                // The script's text takes a tap anywhere (menu_commands).
                stand_down(env, game, "a script's message");
            } else if over_map {
                stand_down(env, game, "a menu");
            } else if let Battled::Done(marker) =
                battle_commands(env, game, &battle, owner, !widgets.is_empty())
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
                drop_finger(env, game, "left battle");
            }
        }
    }
    menu_commands(env, game, widgets)
}

/// Start the battle handlers afresh when the phase changes: the held
/// finger, the cursor and anything pending belong to the last one.
fn track_battle_phase(env: &mut Environment, game: Game, battle: &Battle) {
    let key = (battle.work, battle.phase);
    if env.framework_state.song_summoner.game_input.battle_input.phase == Some(key) {
        return;
    }
    let selected = selected_unit_tile(env, game);
    drop_finger(env, game, "the phase changed");
    let state = &mut env.framework_state.song_summoner.game_input;
    let after_turn_script = state.battle_input.phase == Some((battle.work, 18));
    log!(
        "input: battle phase {} ({}), controller: {:?}",
        battle.phase,
        battle::phase_name(battle.phase),
        battle::phase_owner(battle.phase)
    );
    // Unit select starts on the selected unit: after a unit's turn ends,
    // that's the next one, not where the cursor was. But when a team's
    // turn starts (after the turn script), it's the game's cursor: the
    // selected unit is still the other team's last. The game's cursor is
    // followed from where it is now.
    state.battle_input = BattleInput {
        phase: Some(key),
        cursor: (battle.phase == 19)
            .then(|| battle::unit_select_start(&battle.grid, selected, battle.cursor, after_turn_script)),
        seen: Some(battle.cursor),
        ..BattleInput::default()
    };
}

/// Start a handler's own screen afresh (the ring's state, the deploy's)
/// when it changes.
fn enter_screen(env: &mut Environment, game: Game, screen: u32) {
    if env.framework_state.song_summoner.game_input.battle_input.screen == Some(screen) {
        return;
    }
    drop_finger(env, game, &format!("screen {screen} now"));
    let state = &mut env.framework_state.song_summoner.game_input;
    state.battle_input = BattleInput {
        phase: state.battle_input.phase,
        screen: Some(screen),
        ..BattleInput::default()
    };
}

/// Something is over the map: the battle handlers stand down, and a held
/// finger goes (its lift would be what advances a tutorial).
fn stand_down(env: &mut Environment, game: Game, what: &str) {
    drop_finger(env, game, &format!("{what} over the map"));
    env.framework_state.song_summoner.game_input.battle_input.act = false;
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

/// Battle's own screens, by `owner` (battle::owner_now). `widgets_open`:
/// something the menus handle is up (only the deploy's card list, since
/// anything over the map was dealt with before).
fn battle_commands(
    env: &mut Environment,
    game: Game,
    battle: &Battle,
    owner: battle::Owner,
    widgets_open: bool,
) -> Battled {
    use battle::Owner;
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
            Battled::Done(unit_select(env, game, battle))
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
    enter_screen(env, game, screen);
    let state = &mut env.framework_state.song_summoner.game_input;
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
    // Only while the controller is in use, as in unit select.
    if !env.framework_state.song_summoner.game_input.pad_mode {
        return None;
    }
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
    // Only while the controller is in use, as in unit select.
    if !env.framework_state.song_summoner.game_input.pad_mode {
        return None;
    }
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
        let stuck = state.battle_input.last_pan == Some(camera.origin);
        let clean = &mut state.clean_look;
        return cursor.and_then(|c| diamond(&camera, &state.finger, c, battle::WHOLE_SCREEN, stuck, clean));
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
        hold_area: battle::WHOLE_SCREEN,
        harmless_area: battle::HARMLESS_AREA,
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
    let stuck = state.battle_input.last_pan == Some(camera.origin);
    let clean = &mut state.clean_look;
    let outline = cursor.and_then(|c| diamond(&camera, &state.finger, c, battle::WHOLE_SCREEN, stuck, clean));
    if let Some(percent) = zoom_now {
        apply_zoom(env, game, percent);
    }
    outline
}

/// touchHLE's diamond on the cursor's tile, unless the finger is held
/// there: then the game draws its own cursor, under the units. The play
/// look shows it only where the game can't show its own: see
/// [diamond_look].
fn diamond(
    camera: &Camera,
    finger: &Finger,
    cursor: Tile,
    hold_area: battle::Rect,
    stuck: bool,
    clean: &mut Clean,
) -> Option<FocusMarker> {
    let holdable = battle::holdable_in(camera.tile_centre(cursor), hold_area);
    *clean = diamond_look(holdable, stuck);
    let under_finger = finger.finger().map(|p| camera.hold_tile_at(p)) == Some(cursor);
    if finger.held() && under_finger {
        None
    } else {
        Some((camera.tile_rect(cursor), FocusShape::Diamond))
    }
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
    enter_screen(env, game, screen);
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
    if let Some(&row) = set.rects.get(index) {
        env.framework_state.song_summoner.game_input.clean_look = Clean::HighlightAt(button_art(row));
    }
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
) -> Option<FocusMarker> {
    let brackets = |rect: (f32, f32, f32, f32)| (rect, FocusShape::Brackets);
    // A cutscene's SKIP, or the Listening Point scene's.
    let skip_shown = (game.skip_able != 0
        && env.mem.read(ConstPtr::<u8>::from_bits(game.skip_able)) != 0)
        || listening_point_skip(env, game);
    log_list_drag(env, game, &widgets);
    let widgets = without_captions(widgets);
    let dial = read_dial(env, game);
    let shop_menu = shop_top_menu(env, game);
    let state = &mut env.framework_state.song_summoner.game_input;
    // Set again below while the password keyboard is up; typing waits
    // for nothing else.
    let keyboard_was_up = std::mem::take(&mut state.password_keyboard);
    // The shop's top menu: Info taps the password spot.
    state.shop_menu = shop_menu;
    if shop_menu && state.commands.contains(&Role::Info) {
        state.commands.retain(|&role| role != Role::Info);
        if state.taps.is_idle() {
            log!("input: shop, opening the password entry");
            state.taps.tap(SHOP_PASSWORD_TAP.0, SHOP_PASSWORD_TAP.1);
        }
    }
    if dial.is_none() {
        state.dial = None;
    }

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
                Widget::Keyboard { .. } => "keyboard",
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

    // The shop's quantity dial takes the focus from its Forget it/Confirm
    // dialog, which confirm and back press (the user's choice,
    // 2026-09-28).
    if let Some(dial) = &dial {
        let dialog = widgets.iter().find_map(|w| match w {
            Widget::Dialog(dialog) => Some(dialog),
            _ => None,
        });
        if let Some(dialog) = dialog {
            return Some(dial_commands(state, dial, dialog));
        }
    }

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
            Widget::Keyboard { typed, .. } => {
                log!("input: password keyboard, {} typed", typed);
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
        | Widget::World(_)
        | Widget::Keyboard { .. } => None,
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
            if appeared && state.preselected == Some(work) {
                state.preselected = None;
            }
            if state.taps.is_idle() {
                index = set.selected.unwrap_or(index);
                if let Some(role) = state.commands.pop_front() {
                    let (new_index, taps) = select_command(&set, index, role);
                    index = new_index;
                    for (x, y) in taps {
                        state.taps.tap(x, y);
                    }
                } else if let Some(at) = preselect_tap(&set, index, state.pad_mode, state.preselected == Some(work)) {
                    // Select the first item as a first tap would, so the
                    // game shows its own highlight and description (the
                    // user's choice, 2026-09-28). Once per opening, in
                    // case the game won't select it.
                    log!("input: selecting item {} to start, at {:?}", index, at);
                    state.taps.tap(at.0, at.1);
                    state.preselected = Some(work);
                }
            }
            state.focus = Some((work, index));
            // The game highlights the selected button itself.
            if set.selected == Some(index) || !state.taps.is_idle() {
                return None;
            }
            state.clean_look = Clean::Highlight;
            return set.rects.get(index).copied().map(brackets);
        }
        while let Some(role) = state.commands.pop_front() {
            let (new_index, tap) = button_command(&set, index, role);
            index = new_index;
            state.pressed_at = (role == Role::Confirm && tap.is_some()).then_some(state.frame);
            if let Some((x, y)) = tap {
                state.taps.tap(x, y);
            }
        }
        state.focus = Some((work, index));
        let rect = set.rects.get(index).copied();
        // Taps use the whole touch area; the outline goes around the button
        // art instead, which the touch area overhangs.
        // No resting selected look of their own: a copy of the game's
        // highlight, on the button's art.
        state.clean_look = match rect {
            Some(area) => Clean::HighlightAt(button_art(area)),
            None => Clean::Highlight,
        };
        if matches!(widget, Widget::Dialog(_)) {
            // The copy goes on the button art only; the touch area reaches
            // past it (2026-09-28 screenshot: Yes's copy overhung it).
            if let Some(area) = rect {
                state.clean_look = Clean::HighlightAt(dialog_button_art(area));
            }
        }
        if just_pressed(state.pressed_at, state.frame) {
            state.clean_look = Clean::Hide;
        }
        if matches!(widget, Widget::Dialog(_)) {
            return rect.map(dialog_outline).map(brackets);
        }
        return rect.map(brackets);
    }

    if let &Widget::Keyboard { work, typed } = &widget {
        let mut focus = match state.focus {
            Some((w, i)) if w == work && i < KEYBOARD_KEYS.len() => i,
            _ => 0,
        };
        state.password_keyboard = true;
        if !keyboard_was_up {
            state.typed_keys.clear();
        }
        // One key at a time: the game takes the key under the finger when
        // it lifts, so each tap finishes before the next. Keys typed on a
        // real keyboard go first, and move the focus to where they are.
        if state.taps.is_idle() {
            if let Some(i) = state.typed_keys.pop_front().and_then(key_index) {
                focus = i;
                let (x, y) = rect_center(key_rect(i));
                state.taps.tap(x, y);
            }
        }
        if state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, tap) = keyboard_command(focus, typed, role);
                focus = new_focus;
                if let Some((x, y)) = tap {
                    state.taps.tap(x, y);
                    break;
                }
            }
        }
        state.focus = Some((work, focus));
        // The keys have no selected look of their own.
        state.clean_look = Clean::Highlight;
        return Some(brackets(key_rect(focus)));
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
                        // A scroll is stopped once it has gone its way.
                        state.taps.extend(help_stop(gesture));
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
                    state.taps.extend(help_stop(drag));
                }
            }
        }
        state.focus = Some((id, focus));
        // A page has nothing to point at.
        if help.page {
            return None;
        }
        state.clean_look = Clean::Highlight;
        return help
            .rows
            .get(focus)
            .copied()
            .filter(|_| help.row_visible(focus))
            .map(brackets);
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
        state.clean_look = Clean::Highlight;
        return rows.get(focus).copied().map(brackets);
    }

    if let Widget::List(list) = &widget {
        let mut focus = match state.focus {
            Some((w, i)) if w == list.work && i < list.rows.len() => i,
            _ => list.cursor,
        };
        state.since_drag = state.since_drag.saturating_add(1);
        // Judge the last scroll drag once it's done and the list is still:
        // moved, fine; not moved, this list won't scroll by a drag.
        let judging = state.taps.is_idle() && state.since_drag > HOLD_FRAMES + 2 && list.settled;
        // (pending: still waiting to judge it; stuck: it didn't move.)
        let (pending, stuck) = match state.list_scroll {
            Some((work, first, wanted_later)) if work == list.work => {
                if !judging {
                    (true, false)
                } else {
                    match judge_scroll(first, list.first, wanted_later) {
                        ScrollOutcome::Moved => {
                            state.list_scroll = None;
                            (false, false)
                        }
                        ScrollOutcome::Still => (false, true),
                        // Not expected with LIST_DRAG; the next drag tries
                        // again from wherever it went.
                        ScrollOutcome::Reversed => {
                            log!(
                                "input: list scrolled the wrong way for a drag (from {} to {})",
                                first,
                                list.first
                            );
                            state.list_scroll = None;
                            (false, false)
                        }
                    }
                }
            }
            _ => (false, false),
        };
        if state.taps.is_idle() {
            while let Some(role) = state.commands.pop_front() {
                let (new_focus, tap) = list_command(list, focus, role);
                if new_focus != focus || tap.is_none() {
                    state.list_follow = true;
                }
                focus = if stuck {
                    list.clamp_to_shown(new_focus)
                } else {
                    new_focus
                };
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
            // Bring the focused row on screen. The frames after a drag let
            // its glide register as the list's speed.
            if stuck {
                let shown = list.clamp_to_shown(focus);
                if shown != focus {
                    log!(
                        "input: list didn't scroll for a drag (at {}, speed now 0), keeping the focus on row {} instead of {}",
                        list.first,
                        shown,
                        focus
                    );
                    focus = shown;
                }
            } else if !pending && state.since_drag > HOLD_FRAMES + 2 {
                let (kept, drag) = list_keep_focus(list, focus, state.list_follow);
                if kept != focus {
                    log!(
                        "input: list moved by touch (at {}), focus from row {} to {}",
                        list.first,
                        focus,
                        kept
                    );
                    focus = kept;
                }
                if let Some(Gesture::Drag { x, y, dy }) = drag {
                    log!(
                        "input: list, dragging at {:?} by {} to scroll toward row {} (at {}, {} shown, cursor {})",
                        (x, y),
                        dy,
                        focus,
                        list.first,
                        list.shown,
                        list.cursor
                    );
                    state.taps.drag(x, y, dy);
                    state.since_drag = 0;
                    // Meant to bring on later rows (the position going up).
                    let later = focus >= list.first.round().max(0.0) as usize;
                    state.list_scroll = Some((list.work, list.first, later));
                }
            }
        }
        if list.row_visible(focus) {
            state.list_follow = false;
        }
        state.focus = Some((list.work, focus));
        // A copy of the game's own highlight (the user's choice,
        // 2026-09-28).
        return list
            .rows
            .get(focus)
            .filter(|_| list.row_visible(focus))
            .and_then(|&row| list_marker(row, list.menu_x, &list.highlight));
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
        // The list centres the selected card itself; the icons below it
        // have no selected look.
        state.clean_look = if focus == CardFocus::Cards {
            Clean::Hide
        } else {
            Clean::Highlight
        };
        return Some(brackets(list.focus_rect(focus)));
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
    // Its middle band shows the choice.
    state.clean_look = Clean::Hide;
    Some(brackets(drum.select_rect()))
}

/// The shop's quantity dial (see [dial_command]): commands set the target
/// digits, and the drums are flicked to them one digit at a time, each
/// flick once they've stopped. Confirm waits until they're there.
fn dial_commands(state: &mut State, dial: &Dial, dialog: &Dialog) -> FocusMarker {
    let work = dial.drums[0].work;
    if state.dial.as_ref().map(|d| d.work) != Some(work) {
        log!(
            "input: quantity dial, drums {:?}, digits {:?}",
            dial.drums.iter().map(|d| d.rect).collect::<Vec<_>>(),
            dial.digits()
        );
        state.dial = Some(DialState {
            work,
            focus: dial.drums.len() - 1,
            target: dial.digits(),
            bands: vec![None; dial.drums.len()],
        });
    }
    let State {
        dial: dial_state,
        commands,
        taps,
        command_age,
        ..
    } = state;
    let ds = dial_state.as_mut().unwrap();
    ds.bands.resize(dial.drums.len(), None);
    for (seen, drum) in ds.bands.iter_mut().zip(&dial.drums) {
        if drum.band.is_some() {
            *seen = drum.band;
        }
    }
    let there = dial.settled() && dial.digits() == ds.target;
    if taps.is_idle() {
        while let Some(role) = commands.pop_front() {
            let (focus, press) = dial_command(ds.focus, &mut ds.target, role);
            ds.focus = focus.min(dial.drums.len() - 1);
            let Some(press) = press else {
                continue;
            };
            // Buy what's shown only once the drums show what was asked.
            if press == DialPress::Confirm && !there {
                commands.push_front(role);
                break;
            }
            if let Some((x, y)) = dial_press_point(dialog, press) {
                log!("input: quantity dial, {:?} at ({}, {})", press, x, y);
                taps.tap(x, y);
            }
            break;
        }
    }
    if taps.is_idle() {
        if let Some(Gesture::Drag { x, y, dy }) = dial_step(dial, &ds.target) {
            log!(
                "input: quantity dial, digits {:?} toward {:?}, flicking at ({}, {}) by {}",
                dial.digits(),
                ds.target,
                x,
                y,
                dy
            );
            taps.drag(x, y, dy);
            // The dial is getting there: a confirm waiting on it isn't
            // stale.
            *command_age = CommandAge::default();
        }
    }
    let focus = ds.focus;
    state.focus = Some((work, focus));
    // The band as last seen, if the game has it hidden while turning.
    let mut shown = dial.clone();
    if let Some(band) = state.dial.as_ref().and_then(|d| d.bands.get(focus).copied().flatten()) {
        shown.drums[focus].band = Some(band);
    }
    state.clean_look = Clean::HighlightAt(shown.highlight(focus));
    (dial.band(focus), FocusShape::Brackets)
}

/// Outline `marker` (game points) in the window's (portrait) coordinates.
fn show_focus(env: &mut Environment, main_view: id, marker: Option<FocusMarker>) {
    let show = env.framework_state.song_summoner.game_input.pad_mode;
    let marker = play_look(
        marker,
        env.framework_state.song_summoner.game_input.clean_look,
        env.options.controller_debug,
    );
    // A tile's diamond is drawn inside it, and a tile half off the edge is
    // better cut off than squashed, so only brackets are pulled on screen.
    let marker = marker.map(|(rect, shape)| match shape {
        FocusShape::Brackets => (keep_on_screen(rect), shape),
        FocusShape::Diamond | FocusShape::Highlight => (rect, shape),
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
        state.commands.clear();
        drop_finger(env, game, "the picker is up");
        show_focus(env, main_view, None);
        clear_battle_overlay(env);
        return;
    }
    env.framework_state.song_summoner.game_input.frame += 1;
    log_tasks(env, game);
    let before = env.framework_state.song_summoner.game_input.commands.len();
    env.framework_state.song_summoner.game_input.clean_look = Clean::Same;
    let focus = run_commands(env, game);
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.command_age.stale(before, state.commands.len()) {
        log!(
            "input: {:?} waited {} frames unused, dropping them",
            state.commands,
            STALE_COMMAND_FRAMES
        );
        state.commands.clear();
    }
    show_focus(env, main_view, focus);
    update_soft_keyboard(env);
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
    // The Setup menu is open: the game is paused, so its frame is skipped
    // (the menu shows its last one again).
    if super::setup::before_frame(env) {
        return;
    }
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
    fn a_finger_is_held_once_the_game_calls_it_a_hold() {
        // The game shows its cursor only once a finger is a hold, so
        // touchHLE's cursor stays until then.
        let mut finger = Finger::default();
        assert!(!finger.held());
        finger.press(1.0, 1.0);
        finger.next_frame(); // Began
        for _ in 0..MIN_STILL_FRAMES {
            assert!(!finger.held());
            finger.next_frame();
        }
        assert!(finger.held());
        // Sliding keeps it a hold; letting go ends it.
        finger.slide_to(2.0, 2.0);
        finger.next_frame();
        assert!(finger.held());
        finger.lift();
        finger.next_frame();
        assert!(!finger.held());
    }

    #[test]
    fn a_cleared_finger_is_up_without_a_lift() {
        let mut finger = Finger::default();
        finger.press(1.0, 1.0);
        finger.next_frame();
        finger.slide_to(2.0, 2.0);
        // The game had it down: it must be told (SysTouch_Clear), or it
        // counts the next real finger as a second one (a pinch).
        assert!(finger.clear());
        assert!(finger.is_idle() && !finger.is_down());
        assert_eq!(finger.finger(), None);
        assert_eq!(finger.next_frame(), None);
        // Only queued, never down: nothing to tell.
        finger.press(1.0, 1.0);
        assert!(!finger.clear());
        assert!(!Finger::default().clear());
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
    fn a_column_wraps_up_and_down_and_skips_disabled_buttons() {
        let menu = title_menu(4);
        let rects: Vec<_> = (0..4).map(|i| menu.button_rect(i)).collect();
        let enabled = [true, false, true, true];
        assert_eq!(step_toward(&rects, &enabled, 0, Role::Down), 2);
        assert_eq!(step_toward(&rects, &enabled, 3, Role::Down), 0);
        assert_eq!(step_toward(&rects, &enabled, 0, Role::Up), 3);
        assert_eq!(step_toward(&rects, &enabled, 2, Role::Up), 0);
        // Nothing to the side of a column.
        assert_eq!(step_toward(&rects, &enabled, 2, Role::PrevSection), 2);
        assert_eq!(step_toward(&rects, &enabled, 2, Role::NextSection), 2);
        // Nothing enabled: stay put.
        assert_eq!(step_toward(&rects[..2], &[false, false], 1, Role::Down), 1);
    }

    #[test]
    fn left_and_right_go_by_where_the_buttons_are_and_never_wrap() {
        // The user's rule (2026-09-28): in a Yes/No dialog left is always
        // the left button (No) and right the right one (Yes); left on No
        // and right on Yes do nothing, and up/down nothing at all. The
        // game lists a dialog's buttons right to left (a 2026-09-28 log:
        // Yes at x 255 first, No at x 80).
        let rects = [(255.0, 200.0, 157.0, 64.0), (80.0, 200.0, 157.0, 64.0)];
        let on = [true, true];
        assert_eq!(step_toward(&rects, &on, 0, Role::PrevSection), 1);
        assert_eq!(step_toward(&rects, &on, 1, Role::PrevSection), 1);
        assert_eq!(step_toward(&rects, &on, 1, Role::NextSection), 0);
        assert_eq!(step_toward(&rects, &on, 0, Role::NextSection), 0);
        for role in [Role::Up, Role::Down] {
            assert_eq!(step_toward(&rects, &on, 0, role), 0);
            assert_eq!(step_toward(&rects, &on, 1, role), 1);
        }
        // A row of three stops at both ends.
        let row = location_menu(None).rects;
        let on = [true; 3];
        assert_eq!(step_toward(&row, &on, 0, Role::NextSection), 1);
        assert_eq!(step_toward(&row, &on, 2, Role::NextSection), 2);
        assert_eq!(step_toward(&row, &on, 0, Role::PrevSection), 0);
    }

    // The shop's password keyboard: getkeybord's table (`keyrect`,
    // 0x6ea48), song-summoner-re.md, The shop's password keyboard.
    fn key_at(key: u8) -> usize {
        KEYBOARD_KEYS.iter().position(|k| k.4 == key).unwrap()
    }

    #[test]
    fn the_keyboard_is_the_games_key_table() {
        assert_eq!(KEYBOARD_KEYS.len(), 39);
        assert_eq!(KEYBOARD_KEYS[0], (9.0, 164.0, 39.0, 32.0, b'1'));
        assert_eq!(KEYBOARD_KEYS[key_at(b'P')], (432.0, 204.0, 39.0, 32.0, b'P'));
        assert_eq!(KEYBOARD_KEYS[key_at(KEY_BACKSPACE)], (432.0, 244.0, 39.0, 32.0, b'b'));
        assert_eq!(KEYBOARD_KEYS[key_at(KEY_QUIT)], (9.0, 285.0, 51.0, 30.0, b'r'));
        assert_eq!(KEYBOARD_KEYS[key_at(KEY_ENTER)], (419.0, 285.0, 51.0, 30.0, b'e'));
        // Every key's middle is inside its own hit test and no other.
        for (i, &(x, y, w, h, _)) in KEYBOARD_KEYS.iter().enumerate() {
            let (cx, cy) = rect_center((x, y, w, h));
            let hits: Vec<usize> = KEYBOARD_KEYS
                .iter()
                .enumerate()
                .filter(|(_, k)| cx >= k.0 && cx < k.0 + k.2 && cy >= k.1 && cy < k.1 + k.3)
                .map(|(j, _)| j)
                .collect();
            assert_eq!(hits, vec![i]);
        }
    }

    #[test]
    fn the_d_pad_moves_over_the_keys() {
        let one = key_at(b'1');
        assert_eq!(keyboard_command(one, 0, Role::NextSection), (key_at(b'2'), None));
        assert_eq!(keyboard_command(one, 0, Role::Down), (key_at(b'Q'), None));
        // The rows stop at their ends; up and down wrap round.
        assert_eq!(keyboard_command(key_at(b'0'), 0, Role::NextSection), (key_at(b'0'), None));
        assert_eq!(keyboard_command(one, 0, Role::PrevSection), (one, None));
        assert_eq!(keyboard_command(one, 0, Role::Up), (key_at(KEY_QUIT), None));
        // The bottom row is set in: Enter is under Backspace.
        assert_eq!(keyboard_command(key_at(KEY_BACKSPACE), 0, Role::Down), (key_at(KEY_ENTER), None));
    }

    #[test]
    fn a_real_keyboard_types_on_the_password_keyboard() {
        // SDL scancode names, as window.rs sends them.
        assert_eq!(typed_key("Q"), Some(b'Q'));
        assert_eq!(typed_key("7"), Some(b'7'));
        assert_eq!(typed_key("Keypad 7"), Some(b'7'));
        // The game has no I or O: its keys there are 1 and 0.
        assert_eq!(typed_key("I"), Some(b'1'));
        assert_eq!(typed_key("O"), Some(b'0'));
        assert_eq!(typed_key("Backspace"), Some(KEY_BACKSPACE));
        assert_eq!(typed_key("Return"), Some(KEY_ENTER));
        assert_eq!(typed_key("Keypad Enter"), Some(KEY_ENTER));
        // Everything else keeps its mapping (arrows move, Escape backs
        // out).
        for key in ["Up", "Escape", "Space", "F2", "Left Shift", "-"] {
            assert_eq!(typed_key(key), None, "{key}");
        }
        // Every key it types is on the game's keyboard.
        for c in ('A'..='Z').chain('0'..='9') {
            let key = typed_key(&c.to_string()).unwrap();
            assert!(key_index(key).is_some(), "{c}");
        }
    }

    #[test]
    fn confirm_types_the_key_and_back_deletes() {
        let q = key_at(b'Q');
        assert_eq!(keyboard_command(q, 0, Role::Confirm), (q, Some((28.5, 220.0))));
        // Back is Backspace while there's something typed...
        let (x, y, w, h, _) = KEYBOARD_KEYS[key_at(KEY_BACKSPACE)];
        let backspace = rect_center((x, y, w, h));
        assert_eq!(keyboard_command(q, 3, Role::Back), (q, Some(backspace)));
        // ...and with nothing typed, the key that asks to leave.
        let (x, y, w, h, _) = KEYBOARD_KEYS[key_at(KEY_QUIT)];
        assert_eq!(keyboard_command(q, 0, Role::Back), (q, Some(rect_center((x, y, w, h)))));
    }

    #[test]
    fn the_password_button_taps_the_shopkeeper() {
        // ShopFlow_MenuSelect opens the password entry on a still
        // finger-up at x 40-192, y 108 or more, off the menu's buttons
        // (a column at x 320, 196 wide).
        let (x, y) = SHOP_PASSWORD_TAP;
        assert!((40.0..=192.0).contains(&x) && (108.0..320.0).contains(&y));
        assert!(x < 320.0 - 98.0);
    }

    #[test]
    fn a_caption_never_takes_the_focus_from_a_menu() {
        // The world map's "Select a map for battle." (2026-09-28, Odin): a
        // SysDialog with no buttons opens with a one-button menu under it.
        // It opened last, took the focus, and confirm tapped the middle of
        // the screen instead of the button.
        let caption = Widget::Dialog(Dialog {
            work: 0x3000,
            rects: Vec::new(),
            enabled: Vec::new(),
            outside_cancels: false,
        });
        let menu = Widget::Buttons(title_menu(1));
        let kept = without_captions(vec![caption.clone(), menu.clone()]);
        assert_eq!(kept, vec![menu]);
        // Alone, a message is still what confirm taps through.
        assert_eq!(without_captions(vec![caption.clone()]), vec![caption]);
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
        // No wrapping, and up/down do nothing in a row.
        assert_eq!(button_command(&set, 1, Role::NextSection), (1, None));
        assert_eq!(button_command(&set, 0, Role::Down), (0, None));
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
        // The row doesn't wrap: left on the first icon does nothing.
        assert_eq!(select_command(&set, 0, Role::PrevSection), (0, vec![]));
        assert_eq!(
            select_command(&set, 2, Role::PrevSection),
            (1, vec![(240.0, 160.0)])
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
        // Row 4 ends at 312: the rows must go up 56 + 3. The move counts on
        // its frame and 90% of it on the next, before a stop: 59 / 1.9.
        let Some(Gesture::Drag { dy, .. }) = h.scroll_toward(4) else {
            panic!()
        };
        assert!((dy - -59.0 / 1.9).abs() < 1e-3, "{dy}");
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
        // Down: half the view, 136, as a move of 136 / 1.9 and a stop.
        let Some(Some(Gesture::Drag { dy, .. })) = help_command(&h, 0, Role::Down).1 else {
            panic!()
        };
        assert!((dy - -136.0 / 1.9).abs() < 1e-3, "{dy}");
    }

    #[test]
    fn a_help_scroll_is_stopped_by_a_sideways_touch() {
        // Help glides a move on, 10% less a frame, until a finger goes
        // down. So a scroll drag is followed by a touch where it ended that
        // slides 1 point sideways: its finger-down stops the glide, and a
        // finger-up after a move opens nothing (nor swipes, under 24).
        let drag = Gesture::Drag {
            x: 240.0,
            y: 144.0,
            dy: -31.0,
        };
        assert_eq!(
            help_stop(drag),
            Some(Gesture::Slide {
                x: 240.0,
                y: 113.0,
                to_x: 241.0
            })
        );
        assert_eq!(help_stop(Gesture::Tap(1.0, 1.0)), None);
    }

    #[test]
    fn a_lists_back_can_be_the_screens_back_icon() {
        // The shop's lists (the older SysMenu_Check) don't cancel on a tap
        // off the rows; its back icon does.
        let mut list = items(0.0);
        assert_eq!(list_command(&list, 0, Role::Back), (0, Some(Some(OUTSIDE))));
        list.back = (456.0, 296.0);
        assert_eq!(list_command(&list, 0, Role::Back), (0, Some(Some((456.0, 296.0)))));
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
    // The Results screen's pearl split (ResultFlow_DivideSelect).

    #[test]
    fn result_troopers_are_the_slots_along_the_bottom() {
        // A finger-up at y 228-300, x 156 + 64i to 204 + 64i, is slot i;
        // slots below the trooper count are troopers, slot 4 is EXIT.
        let set = result_divide_set(3, 1);
        assert_eq!(
            set.rects,
            vec![
                (156.0, 228.0, 48.0, 72.0),
                (220.0, 228.0, 48.0, 72.0),
                (284.0, 228.0, 48.0, 72.0),
            ]
        );
        assert!(set.two_tap);
        assert_eq!(set.selected, Some(1));
        // Back taps EXIT, which leaves on one tap.
        assert_eq!(set.back, Some(RESULT_EXIT));
        let (x, y) = RESULT_EXIT;
        assert!((412.0..=460.0).contains(&x) && (228.0..=300.0).contains(&y));
        // Never more than four troopers; a bad selection is none.
        assert_eq!(result_divide_set(9, -1).rects.len(), 4);
        assert_eq!(result_divide_set(9, -1).selected, None);
    }

    #[test]
    fn result_troopers_select_then_rank_up() {
        // A tap on another trooper selects it (its status shows); a tap on
        // the selected one asks to rank it up.
        let set = result_divide_set(3, 0);
        assert_eq!(select_command(&set, 0, Role::NextSection), (1, vec![(244.0, 264.0)]));
        assert_eq!(select_command(&set, 0, Role::Confirm), (0, vec![(180.0, 264.0)]));
        assert_eq!(select_command(&set, 0, Role::Back), (0, vec![RESULT_EXIT]));
        // The last trooper is the end: EXIT isn't in the row, so moving
        // never leaves the screen, and the row doesn't wrap.
        let set = result_divide_set(3, 2);
        assert_eq!(select_command(&set, 2, Role::NextSection), (2, vec![]));
    }

    #[test]
    fn a_list_highlight_goes_where_the_games_would() {
        // SysMenu_Disp_Cursor puts the highlight sprite (+0xd28) at the
        // menu's x (+0xd08) and one point above the row, at its own size.
        // Battle's item window: menu at x 120, rows from x 130.
        let row = (130.0, 88.0, 240.0, 40.0);
        let highlight = ListHighlight {
            size: Some((240.0, 42.0)),
            centred: false,
            shown: false,
        };
        assert_eq!(list_highlight(row, Some(120.0), &highlight), (120.0, 87.0, 240.0, 42.0));
        // A sprite placed by its centre.
        let centred = ListHighlight {
            centred: true,
            ..highlight
        };
        assert_eq!(list_highlight(row, Some(120.0), &centred), (0.0, 66.0, 240.0, 42.0));
        // No size read (never shown yet): the row's; no menu x: the row's.
        let no_size = ListHighlight {
            size: None,
            ..highlight
        };
        assert_eq!(list_highlight(row, None, &no_size), (130.0, 87.0, 240.0, 40.0));
    }

    #[test]
    fn the_copy_hides_while_the_games_highlight_shows() {
        // A finger on a row (a touch, or the controller's own tap) shows the
        // game's highlight; drawing the copy too would double it.
        let row = (130.0, 88.0, 240.0, 40.0);
        let shown = ListHighlight {
            size: Some((240.0, 42.0)),
            centred: false,
            shown: true,
        };
        assert_eq!(list_marker(row, Some(120.0), &shown), None);
        let hidden = ListHighlight {
            shown: false,
            ..shown
        };
        assert_eq!(
            list_marker(row, Some(120.0), &hidden),
            Some(((120.0, 87.0, 240.0, 42.0), FocusShape::Highlight))
        );
    }

    #[test]
    fn a_list_that_wont_scroll_keeps_the_focus_on_screen() {
        // The shop's list didn't move for a drag (seen 2026-09-28): the
        // focus goes to the nearest row shown rather than waiting forever.
        assert_eq!(items(0.0).clamp_to_shown(6), 3);
        assert_eq!(items(0.0).clamp_to_shown(2), 2);
        assert_eq!(items(3.0).clamp_to_shown(0), 3);
        assert_eq!(items(3.0).clamp_to_shown(7), 6);
        // Past the end (8 rows, 4 shown from 6): only rows 6 and 7 exist.
        assert_eq!(items(6.0).clamp_to_shown(0), 6);
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
            snaps: true,
            menu_x: None,
            highlight: ListHighlight {
                size: None,
                centred: false,
                shown: false,
            },
            back: OUTSIDE,
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

    // The shop's quantity dial: tens at (306, 40), ones at (375, 40), each
    // 80x170 (ShopFlow_BuyMenu), and its Forget it / Confirm dialog.
    fn dial(tens: i32, ones: i32) -> Dial {
        let drum = |x: f32, digit: i32| DialDrum {
            work: 0x5000 + x as u32,
            rect: (x, 40.0, 80.0, 170.0),
            digit,
            settled: true,
            band: None,
        };
        Dial {
            drums: vec![drum(306.0, tens), drum(375.0, ones)],
        }
    }

    fn quantity_dialog() -> Dialog {
        Dialog {
            work: 0x6000,
            rects: vec![(256.0, 285.0, 104.0, 44.0), (370.0, 285.0, 104.0, 44.0)],
            enabled: vec![true, true],
            outside_cancels: false,
        }
    }

    #[test]
    fn dial_commands_pick_a_drum_and_turn_it() {
        let mut target = vec![0, 0];
        // Down brings in the digit below the middle band, one more.
        assert_eq!(dial_command(1, &mut target, Role::Down), (1, None));
        assert_eq!(target, [0, 1]);
        assert_eq!(dial_command(1, &mut target, Role::PrevSection), (0, None));
        assert_eq!(dial_command(0, &mut target, Role::PrevSection), (0, None));
        // Up brings in the digit above: from 0 that's 9, as the drum shows.
        dial_command(0, &mut target, Role::Up);
        assert_eq!(target, [9, 1]);
        assert_eq!(dial_command(0, &mut target, Role::NextSection), (1, None));
        assert_eq!(dial_command(1, &mut target, Role::NextSection), (1, None));
        assert_eq!(
            dial_command(1, &mut target, Role::Confirm),
            (1, Some(DialPress::Confirm))
        );
        assert_eq!(
            dial_command(1, &mut target, Role::Back),
            (1, Some(DialPress::Forget))
        );
    }

    #[test]
    fn the_dial_is_flicked_a_digit_at_a_time_the_short_way() {
        // Ones 3 -> 4: a finger going up on the ones drum.
        match dial_step(&dial(0, 3), &[0, 4]) {
            Some(Gesture::Drag { x, y, dy }) => {
                assert_eq!((x, y), (415.0, 125.0));
                assert_eq!(dy, -DIAL_DRAG);
            }
            other => panic!("{other:?}"),
        }
        // 0 -> 9 is one step back, not nine on.
        match dial_step(&dial(0, 0), &[0, 9]) {
            Some(Gesture::Drag { dy, .. }) => assert_eq!(dy, DIAL_DRAG),
            other => panic!("{other:?}"),
        }
        // The tens first.
        match dial_step(&dial(0, 0), &[1, 1]) {
            Some(Gesture::Drag { x, .. }) => assert_eq!(x, 346.0),
            other => panic!("{other:?}"),
        }
        // There, or still turning: nothing.
        assert_eq!(dial_step(&dial(1, 2), &[1, 2]), None);
        let mut turning = dial(0, 0);
        turning.drums[1].settled = false;
        assert_eq!(dial_step(&turning, &[0, 5]), None);
    }

    #[test]
    fn dial_confirm_and_back_press_the_dialogs_buttons() {
        let dialog = quantity_dialog();
        assert_eq!(dial_press_point(&dialog, DialPress::Confirm), Some((422.0, 307.0)));
        assert_eq!(dial_press_point(&dialog, DialPress::Forget), Some((308.0, 307.0)));
        // The outline is the picked drum's middle band.
        assert_eq!(dial(0, 0).band(1), (375.0, 107.0, 80.0, 36.0));
    }

    #[test]
    fn the_play_look_leans_on_the_games_own_highlights() {
        let marker = Some(((10.0, 20.0, 30.0, 40.0), FocusShape::Brackets));
        // Debug: the outline as made, whatever the screen asked.
        assert_eq!(play_look(marker, Clean::Hide, true), marker);
        assert_eq!(play_look(marker, Clean::Highlight, true), marker);
        // Play: hidden, or a copy of the game's highlight on the same rect.
        assert_eq!(play_look(marker, Clean::Hide, false), None);
        assert_eq!(
            play_look(marker, Clean::Highlight, false),
            Some(((10.0, 20.0, 30.0, 40.0), FocusShape::Highlight))
        );
        assert_eq!(play_look(marker, Clean::Same, false), marker);
        assert_eq!(play_look(None, Clean::Highlight, false), None);
        // A dialog's copy sits on its button art, not the outline's space.
        let art = (258.0, 143.0, 142.0, 44.0);
        assert_eq!(
            play_look(marker, Clean::HighlightAt(art), false),
            Some((art, FocusShape::Highlight))
        );
        assert_eq!(play_look(marker, Clean::HighlightAt(art), true), marker);
        assert_eq!(play_look(None, Clean::HighlightAt(art), false), None);
    }

    #[test]
    fn the_diamond_shows_only_where_the_games_cursor_cant() {
        // A tile the finger can rest on: the game's cursor.
        assert_eq!(diamond_look(true, false), Clean::Hide);
        assert_eq!(diamond_look(true, true), Clean::Hide);
        // Out of reach, but a pan may still bring it in.
        assert_eq!(diamond_look(false, false), Clean::Hide);
        // Out of reach for good: touchHLE's diamond is all there is.
        assert_eq!(diamond_look(false, true), Clean::Same);
    }

    #[test]
    fn a_pressed_menus_highlight_stays_hidden_a_moment() {
        assert!(!just_pressed(None, 100));
        assert!(just_pressed(Some(100), 100));
        assert!(just_pressed(Some(100), 100 + PRESS_HIDE_FRAMES - 1));
        assert!(!just_pressed(Some(100), 100 + PRESS_HIDE_FRAMES));
    }

    #[test]
    fn the_button_highlight_covers_the_button_art() {
        // Soul Master's Place's 208×54 menu buttons (2026-09-28
        // screenshot: the art is about 5 in from each side, 4 from the
        // top and 6 from the bottom).
        let (x, y, w, h) = button_art((216.0, 119.0, 208.0, 54.0));
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(x, 216.0 + 208.0 * 4.0 / 144.0), "{x}");
        assert!(close(y, 119.0 + 54.0 * 4.0 / 56.0), "{y}");
        assert!(close(w, 208.0 * 136.0 / 144.0), "{w}");
        assert!(close(h, 54.0 * 45.0 / 56.0), "{h}");
        assert!((5.0..7.0).contains(&(x - 216.0)));
        assert!((5.0..8.0).contains(&(119.0 + 54.0 - (y + h))));
    }

    #[test]
    fn the_dial_highlight_sits_on_the_games_band() {
        let mut d = dial(0, 0);
        // No band sprite read: the middle band.
        assert_eq!(d.highlight(1), d.band(1));
        // With it: that frame's art.
        let sprite = (375.0, 97.0, 80.0, 32.0);
        d.drums[1].band = Some(sprite);
        assert_eq!(d.highlight(1), button_art(sprite));
    }

    #[test]
    fn a_select_then_press_screen_starts_with_its_first_item_selected() {
        let set = ButtonSet {
            rects: vec![(98.0, 138.0, 44.0, 44.0), (178.0, 138.0, 44.0, 44.0)],
            enabled: vec![true, true],
            back: None,
            two_tap: true,
            selected: None,
        };
        assert_eq!(preselect_tap(&set, 0, true, false), Some((120.0, 160.0)));
        // Once only, with the controller in use, and never over a selection.
        assert_eq!(preselect_tap(&set, 0, true, true), None);
        assert_eq!(preselect_tap(&set, 0, false, false), None);
        let selected = ButtonSet {
            selected: Some(1),
            ..set.clone()
        };
        assert_eq!(preselect_tap(&selected, 1, true, false), None);
        // Not a disabled item, and not a screen that presses on one tap.
        let disabled = ButtonSet {
            enabled: vec![false, true],
            ..set.clone()
        };
        assert_eq!(preselect_tap(&disabled, 0, true, false), None);
        let one_tap = ButtonSet {
            two_tap: false,
            ..set
        };
        assert_eq!(preselect_tap(&one_tap, 0, true, false), None);
    }

    #[test]
    fn presses_nothing_uses_are_dropped() {
        let mut age = CommandAge::default();
        // Two presses sit unused on a screen that ignores them.
        for _ in 0..STALE_COMMAND_FRAMES {
            assert!(!age.stale(2, 2));
        }
        assert!(age.stale(2, 2));
        // Presses being used, or none waiting, never age.
        let mut age = CommandAge::default();
        for _ in 0..100 {
            assert!(!age.stale(3, 2));
            assert!(!age.stale(0, 0));
        }
        // Using one starts the wait over.
        let mut age = CommandAge::default();
        for _ in 0..STALE_COMMAND_FRAMES {
            assert!(!age.stale(2, 2));
        }
        assert!(!age.stale(2, 1));
        assert!(!age.stale(1, 1));
    }

    #[test]
    fn a_scroll_drag_starts_off_the_rows() {
        // Left of the rows (x 245 on), level with the first one shown, so
        // the finger going down doesn't pick a row.
        match items(2.0).scroll_toward(7) {
            Some(Gesture::Drag { x, y, .. }) => {
                assert!(x < 245.0);
                assert_eq!(y, 110.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_scroll_drag_is_judged_by_where_the_list_went() {
        // Wanted later rows (the position going up).
        assert_eq!(judge_scroll(0.0, 1.0, true), ScrollOutcome::Moved);
        assert_eq!(judge_scroll(5.0, 6.0, false), ScrollOutcome::Reversed);
        assert_eq!(judge_scroll(5.0, 4.0, false), ScrollOutcome::Moved);
        assert_eq!(judge_scroll(6.0, 5.0, true), ScrollOutcome::Reversed);
        // At the top, a drag the wrong way can't move it at all.
        assert_eq!(judge_scroll(0.0, 0.0, true), ScrollOutcome::Still);
    }

    /// One frame of `menumain`'s scroll: at 0.08 rows a frame or more, or
    /// always if the list has a cursor (`snaps` false), glide (position
    /// += speed, speed x 0.9). With the cursor at -1 (0x51224), below
    /// 0.08 push toward the nearest row by min(0.005 / distance,
    /// distance), added to the speed, and stop at a whole row as soon as
    /// the position crosses one. Checked against the 2026-09-28 log (the
    /// shop) and the 2026-09-29 one (Edit Troopers).
    fn menumain_step(position: f32, speed: f32, last: f32, snaps: bool) -> (f32, f32) {
        let (mut position, mut speed) = (position, speed);
        if snaps && speed.abs() < 0.08 {
            let frac = position - position.floor();
            let push = if frac > 0.5 {
                let d = 1.0 - frac;
                (0.005 / f64::from(d)).min(f64::from(d)) as f32
            } else if frac > 0.0 {
                -(0.005 / f64::from(frac)).min(f64::from(frac)) as f32
            } else {
                0.0
            };
            speed += push;
            let new = position + speed;
            if new.floor() != position.floor() {
                speed = 0.0;
                position = (position + 0.5).floor();
            } else {
                position = new;
            }
        } else {
            position += speed;
            // In doubles, as the game does it.
            speed = (f64::from(speed) * 0.9) as f32;
        }
        if position < 0.0 {
            (position, speed) = (0.0, 0.0);
        }
        if position > last {
            (position, speed) = (last, 0.0);
        }
        (position, speed)
    }

    /// Where a list at `position` stops after a drag of `dy`, and after
    /// how many frames.
    fn glide(position: f32, dy: f32) -> (f32, u32) {
        let (mut position, mut speed) = (position, -dy / 24.0);
        for frame in 1..200 {
            (position, speed) = menumain_step(position, speed, 7.0, true);
            if speed == 0.0 {
                return (position, frame);
            }
        }
        panic!("still gliding at {position}");
    }

    /// The same for a list with a cursor, which never snaps: where it is
    /// once the controller counts it as still, and after how many frames.
    fn coast(position: f32, dy: f32) -> (f32, u32) {
        let (mut position, mut speed) = (position, -dy / 24.0);
        for frame in 1..200 {
            (position, speed) = menumain_step(position, speed, 7.0, false);
            if list_still(speed, false) {
                return (position, frame);
            }
        }
        panic!("still coasting at {position}");
    }

    #[test]
    fn menumain_model_matches_the_log() {
        // The shop's list, 2026-09-28: from row 3, a drag of 4.8 down
        // turned back at 1.65 and ended on row 4, 47 frames later.
        let (mut position, mut speed) = (3.0, -0.2);
        for _ in 0..13 {
            (position, speed) = menumain_step(position, speed, 7.0, true);
        }
        assert!((position - 1.6534).abs() < 0.001, "{position}");
        assert_eq!(glide(3.0, 4.8), (4.0, 47));
    }

    #[test]
    fn a_drag_step_scrolls_one_row_either_way() {
        for start in [1.0, 3.0, 5.0] {
            let (on, frames) = glide(start, -LIST_DRAG);
            assert_eq!(on, start + 1.0);
            assert!(frames <= 12, "{frames}");
            assert_eq!(glide(start, LIST_DRAG).0, start - 1.0);
        }
    }

    #[test]
    fn a_list_with_a_cursor_coasts_without_snapping() {
        // Edit Troopers' item list, 2026-09-29 (cursor 0): from row 0 a
        // drag of 3.5 up went 0.14583333 (speed 0.13125), 0.27708334, ...
        // and came to rest halfway between rows, at 1.4583331, its speed
        // still not 0 two hundred frames later.
        let (mut position, mut speed) = (0.0, 3.5 / 24.0);
        (position, speed) = menumain_step(position, speed, 10.0, false);
        assert!((position - 0.14583333).abs() < 1e-6, "{position}");
        assert!((speed - 0.13125).abs() < 1e-6, "{speed}");
        (position, speed) = menumain_step(position, speed, 10.0, false);
        assert!((position - 0.27708334).abs() < 1e-6, "{position}");
        for _ in 0..200 {
            (position, speed) = menumain_step(position, speed, 10.0, false);
        }
        assert!((position - 1.4583331).abs() < 1e-5, "{position}");
        assert_ne!(speed, 0.0);
    }

    #[test]
    fn a_coast_counts_as_still_before_its_speed_reaches_zero() {
        // Waiting for exactly 0 left the controller waiting for good.
        assert!(list_still(0.0, false));
        assert!(list_still(1e-10, false));
        assert!(list_still(-1e-10, false));
        // With about 5 points (0.12 rows) left to coast, near enough:
        // the list finishes on its own.
        assert!(list_still(0.011, false));
        assert!(!list_still(0.05, false));
        assert!(!list_still(-0.05, false));
        // A snapping list does reach 0, and its snap can turn back
        // through slow speeds on the way, so only 0 is still.
        assert!(list_still(0.0, true));
        assert!(!list_still(1e-10, true));
    }

    fn coasting(first: f32) -> ListMenu {
        ListMenu {
            snaps: false,
            ..items(first)
        }
    }

    fn drag_dy(list: &ListMenu, i: usize) -> f32 {
        match list.scroll_toward(i) {
            Some(Gesture::Drag { dy, .. }) => dy,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_list_with_a_cursor_is_dragged_onto_a_whole_row() {
        // Row 6 from the top (4 shown): rows 3-6 are the nearest view.
        // Counted as still with at most 0.12 rows to go (it rounds to the
        // row, and the next drag aims from wherever it really is).
        let dy = drag_dy(&coasting(0.0), 6);
        let (on, frames) = coast(0.0, dy);
        assert_eq!(on.round(), 3.0);
        assert!((on - 3.0).abs() < 0.13, "{on}");
        assert!(frames <= 32, "{frames}");
        // Left alone it goes on to the row itself.
        let (mut position, mut speed) = (0.0, -dy / 24.0);
        for _ in 0..200 {
            (position, speed) = menumain_step(position, speed, 7.0, false);
        }
        assert!((position - 3.0).abs() < 0.001, "{position}");
        // One row: about 20 frames, where the 2026-09-29 log's took 30.
        let (on, frames) = coast(2.0, drag_dy(&coasting(2.0), 6));
        assert!((on - 3.0).abs() < 0.13, "{on}");
        assert!(frames <= 22, "{frames}");
        // Back up to row 0.
        let (on, _) = coast(3.0, drag_dy(&coasting(3.0), 0));
        assert!(on.abs() < 0.13, "{on}");
        // From where the old drag left it, halfway between rows: onto
        // the next whole row, not another half.
        let halfway = 1.4583331;
        let (on, _) = coast(halfway, drag_dy(&coasting(halfway), 5));
        assert!((on - 2.0).abs() < 0.13, "{on}");
        // And from where this one leaves it, short of the row: the
        // next drag makes up the difference.
        let short = 2.88;
        let (on, _) = coast(short, drag_dy(&coasting(short), 7));
        assert!((on - 4.0).abs() < 0.13, "{on}");
    }

    #[test]
    fn a_list_moved_by_touch_takes_the_focus_along() {
        // 2026-09-29: scrolled to the end by touch with the focus on row
        // 0, and the controller dragged it straight back, over and over.
        // Now the focus moves onto the rows shown instead.
        assert_eq!(list_keep_focus(&items(4.0), 0, false), (4, None));
        assert_eq!(list_keep_focus(&items(0.0), 7, false), (3, None));
    }

    #[test]
    fn a_list_follows_the_focus_a_command_moved() {
        // Down past the last row shown: scroll to it.
        match list_keep_focus(&items(0.0), 4, true) {
            (4, Some(Gesture::Drag { dy, .. })) => assert!(dy < 0.0),
            other => panic!("{other:?}"),
        }
        // On screen, or still gliding: leave both alone.
        assert_eq!(list_keep_focus(&items(0.0), 2, true), (2, None));
        assert_eq!(list_keep_focus(&items(0.0), 2, false), (2, None));
        let mut moving = items(4.0);
        moving.settled = false;
        assert_eq!(list_keep_focus(&moving, 0, false), (0, None));
        assert_eq!(list_keep_focus(&moving, 0, true), (0, None));
    }

    #[test]
    fn a_list_that_snaps_keeps_its_one_row_drag() {
        assert_eq!(drag_dy(&items(0.0), 6), -LIST_DRAG);
        assert_eq!(drag_dy(&items(3.0), 0), LIST_DRAG);
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
