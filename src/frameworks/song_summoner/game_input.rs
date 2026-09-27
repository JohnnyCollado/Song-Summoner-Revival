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

use super::pad::{self, Role};
use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
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
    },
    // The Hip-O-Drome's panel after picking a song: Create Trooper, No,
    // and the back icon.
    SceneButtons {
        main: "__Z11Palace_Mainv",
        buttons: &[0x47c, 0x480],
        back: Some(0x48c),
        over_cards: false,
    },
    // The Hip-O-Drome menu: Create Trooper, Pick of the Pops, Delete
    // Trooper, Learn About Tune Troopers, Leave.
    SceneButtons {
        main: "__Z11Palace_Mainv",
        buttons: &[0x454, 0x458, 0x45c, 0x460, 0x464],
        back: None,
        over_cards: false,
    },
    // A town's icon row (Soul Master's Place: Hip-O-Drome, troopers, data,
    // map). Up to seven icons; each town shows its own subset.
    SceneButtons {
        main: "__Z9Town_Mainv",
        buttons: &[0x60, 0x64, 0x68, 0x6c, 0x70, 0x74, 0x78],
        back: None,
        over_cards: false,
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
}

impl Gesture {
    fn start(self) -> (f32, f32) {
        match self {
            Gesture::Tap(x, y) | Gesture::Drag { x, y, .. } | Gesture::Slide { x, y, .. } => {
                (x, y)
            }
        }
    }

    fn end(self) -> (f32, f32) {
        match self {
            Gesture::Tap(x, y) => (x, y),
            Gesture::Drag { x, y, dy } => (x, y + dy),
            Gesture::Slide { y, to_x, .. } => (to_x, y),
        }
    }
}

/// Turns gestures into touch calls spread over frames.
#[derive(Default)]
pub struct TapQueue {
    pending: VecDeque<Gesture>,
    /// The gesture under way, and frames since its Began.
    current: Option<(Gesture, u32)>,
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
    }

    /// The call to make before the next frame, if any.
    pub fn next_frame(&mut self) -> Option<TouchStep> {
        match self.current {
            Some((Gesture::Tap(x, y), frames)) if frames + 1 >= HOLD_FRAMES => {
                self.current = None;
                Some(TouchStep::Ended(x, y))
            }
            Some((tap @ Gesture::Tap(..), frames)) => {
                self.current = Some((tap, frames + 1));
                None
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
        }
    }
}

impl From<&Dialog> for ButtonSet {
    fn from(dialog: &Dialog) -> ButtonSet {
        ButtonSet {
            rects: dialog.rects.clone(),
            enabled: dialog.enabled.clone(),
            back: dialog.outside_cancels.then_some(OUTSIDE),
        }
    }
}

fn rect_center((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
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

/// Where the controller is in the card list: on the cards, or on icon `i`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CardFocus {
    Cards,
    Icon(usize),
}

impl CardList {
    /// The middle card, roughly, for the outline.
    pub fn card_rect(&self) -> (f32, f32, f32, f32) {
        (self.centre_x - 55.0, 85.0, 110.0, 145.0)
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
}

impl Widget {
    pub fn work(&self) -> u32 {
        match self {
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
    task_manage: u32,
    prim_work: u32,
    card_list_work: u32,
    /// Each [SCENE_BUTTONS] group's main function, resolved (0 if this
    /// copy of the game lacks it).
    scene_mains: [u32; SCENE_GROUPS],
    /// Holds the offset of `MainView`'s `m_mode` ivar.
    m_mode_offset: ConstPtr<u32>,
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
    /// Frames since the last scroll drag, so a drag's glide shows up in the
    /// list's speed before the next is judged.
    since_drag: u32,
    /// Menus open last frame (work addresses), to spot a new one.
    known_menus: Vec<u32>,
    /// Running tasks' main functions last frame, and the outline last
    /// frame (game rect, window rect), both only for logging changes.
    logged_tasks: Vec<u32>,
    logged_outline: Option<Option<((f32, f32, f32, f32), (f32, f32, f32, f32))>>,
    /// `None` until looked up; `Some(None)` if this copy lacks a symbol.
    game: Option<Option<Game>>,
    /// Two `CGPoint`s of guest memory for the touch calls' arguments.
    points: Option<MutPtr<CGPoint>>,
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
            let rects: Vec<_> = group.buttons.iter().filter_map(|&o| sprite(o)).collect();
            if rects.is_empty() {
                continue;
            }
            let set = ButtonSet {
                enabled: vec![true; rects.len()],
                rects,
                back: group.back.and_then(sprite).map(rect_center),
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
    })
}

/// The card list, if it's on screen and taking touches: its key lock
/// (`+0x4c`) off, and at least one of its bottom icons shown.
fn card_list(env: &Environment, game: Game) -> Option<CardList> {
    let work = game.card_list_work;
    if read_u32(env, work + 0x4c) == 1 {
        return None;
    }
    let icon = |offset: u32| read_prim_rect(env, game, read_u32(env, work + offset));
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
    widgets
}

/// A controller button that the picker didn't take.
pub fn handle_pad_button(env: &mut Environment, button: PadButton, pressed: bool) {
    if !pressed {
        return;
    }
    let Some(role) = pad::role(button, env.options.confirm_button) else {
        return;
    };
    let state = &mut env.framework_state.song_summoner.game_input;
    state.pad_mode = true;
    if state.commands.len() < MAX_QUEUED_COMMANDS {
        state.commands.push_back(role);
    }
}

/// A finger touched the screen: hide the controller's focus.
pub fn touch_used(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.game_input;
    if state.pad_mode {
        state.pad_mode = false;
        if let Some(window) = env.window.as_mut() {
            window.set_focus_marker(None);
        }
    }
}

/// Carry out queued controller commands against the open menu, if any, and
/// return the rectangle to outline (in the game's 480×320 points).
fn run_commands(env: &mut Environment, game: Game) -> Option<(f32, f32, f32, f32)> {
    let widgets = open_widgets(env, game);
    let state = &mut env.framework_state.song_summoner.game_input;
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
            })
            .collect();
        log!("input: menus open: {:?}", kinds);
    }
    let focused = state.focus.map(|(work, _)| work);
    let widget = choose_menu(&open, &state.known_menus, focused).map(|i| widgets[i].clone());
    state.known_menus = open;

    let Some(widget) = widget else {
        state.focus = None;
        // No menu: confirm is a tap anywhere.
        while let Some(role) = state.commands.pop_front() {
            if role == Role::Confirm {
                state.taps.tap(CENTER.0, CENTER.1);
            }
        }
        return None;
    };
    if state.focus.map(|(work, _)| work) != Some(widget.work()) {
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
                log!("input: dialog, buttons {:?}", dialog.rects);
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
        | Widget::List(_) => None,
    };
    if let Some(set) = set {
        let mut index = match state.focus {
            Some((w, i)) if w == work && i < set.rects.len() => i,
            _ => first_enabled(&set.enabled),
        };
        while let Some(role) = state.commands.pop_front() {
            let (new_index, tap) = button_command(&set, index, role);
            index = new_index;
            if let Some((x, y)) = tap {
                state.taps.tap(x, y);
            }
        }
        state.focus = Some((work, index));
        return set.rects.get(index).copied();
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
        return list.rows.get(focus).copied().filter(|_| list.row_visible(focus));
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

/// Outline `rect` (game points) in the window's (portrait) coordinates.
fn show_focus(env: &mut Environment, main_view: id, rect: Option<(f32, f32, f32, f32)>) {
    let show = env.framework_state.song_summoner.game_input.pad_mode;
    let rect = match rect {
        Some((x, y, w, h)) if show => {
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
            Some(((x, y, w, h), on_screen))
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
        show_focus(env, main_view, None);
        return;
    }
    log_tasks(env, game);
    let focus = run_commands(env, game);
    show_focus(env, main_view, focus);

    let state = &mut env.framework_state.song_summoner.game_input;
    if state.taps.is_idle() {
        return;
    }
    let Some(step) = state.taps.next_frame() else {
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

    #[test]
    fn a_back_sprite_is_what_back_taps() {
        let set = ButtonSet {
            rects: vec![(268.0, 188.0, 206.0, 44.0), (268.0, 243.0, 206.0, 44.0)],
            enabled: vec![true, true],
            back: Some(rect_center((428.0, 298.0, 48.0, 48.0))),
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
}
