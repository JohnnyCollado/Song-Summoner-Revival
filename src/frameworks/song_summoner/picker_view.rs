/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The music picker: `_touchHLE_SSPickerView`, a host `UIView` that draws
//! the whole 480×320 picker itself (see [super::picker_render]) and handles
//! its own touches.
//!
//! Its only outputs are the two delegate calls of the game's picker
//! contract, made on the game's `iPodView2` object ([Picker::controller]):
//! `mediaPicker:didPickMediaNumber:` when a song is tapped, and
//! `mediaPickerDidCancel:` for Cancel. Hiding the picker, showing the
//! confirmation panel and bringing the picker back on "No" are all the
//! game's own doing (dev-docs/song-summoner-re.md §2).
//!
//! Layout: a navigation bar (title, Cancel, and ‹ Back when drilled into an
//! artist/album/playlist), a list of 55-point rows with A–Z section headers
//! and an index strip, and a four-tab bar (Song, Artist, Album, Playlist).
//!
//! It also works with a game controller ([handle_role], roles in
//! [super::pad]): the D-pad moves a focus (the touched-row glow) up and
//! down or by A–Z section, the shoulder buttons switch tabs, and the
//! confirm/back buttons pick or open a row and go ‹ Back or Cancel. The
//! first press after using touch only shows the focus. While the controller
//! is in use, its button icons ([super::glyphs]) appear next to Back/Cancel
//! and at the ends of the tab bar.

use super::glyphs::{Family, Glyph};
use super::pad::{self, Role};
use super::picker_art::{Art, FIGHTER_COUNT};
use super::picker_render::*;
use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_context::{
    CGContextDrawImage, CGContextRestoreGState, CGContextSaveGState, CGContextScaleCTM,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, CGImageRelease};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{NSInteger, NSTimeInterval};
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::media::artwork::{self, Bitmap};
use crate::media::index::{self, Library};
use crate::media::library;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, SEL};
use crate::window::DeviceOrientation;
use crate::Environment;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// `-[ViewManager end_iPodView]` removes window subviews with this tag.
pub const PICKER_VIEW_TAG: NSInteger = 0x123;

const TAB_NAMES: [&str; 4] = ["Song", "Artist", "Album", "Playlist"];
const TAB_SONG: usize = 0;
const TAB_ARTIST: usize = 1;
const TAB_ALBUM: usize = 2;
const TAB_PLAYLIST: usize = 3;

/// A touch that moves less than this (in points) is a tap.
const TAP_SLOP: f32 = 8.0;
const FRAME_SECONDS: f64 = 1.0 / 60.0;
const ROW_CACHE_SIZE: usize = 24;
const LOADING_FPS: f32 = 12.0;
/// Rows a controller page (D-pad left/right in lists without sections)
/// moves: one screenful.
const ROWS_PER_PAGE: i32 = LIST_H / ROW_H;
/// Size the controller button icons are drawn at.
const PROMPT_GLYPH_SIZE: u32 = 18;

enum Row {
    Header(char),
    Song {
        id: u64,
        title: String,
        subtitle: String,
        artwork: bool,
    },
    Group {
        name: String,
        songs: Vec<u64>,
        /// Albums show the art of one of their songs.
        artwork: Option<Option<u64>>,
    },
}

impl Row {
    fn height(&self) -> i32 {
        match self {
            Row::Header(_) => HDR_H,
            _ => ROW_H,
        }
    }
}

struct ListView {
    /// Identifies the list in the row cache.
    uid: u64,
    title: String,
    /// Label of the ‹ Back button, for drilled-in lists.
    back: Option<String>,
    rows: Vec<Row>,
    /// Top of each row, from the top of the list.
    tops: Vec<i32>,
    height: i32,
    scroll: f32,
    /// Points per second, positive when the content moves up.
    velocity: f32,
    has_index: bool,
    /// The controller's row. Kept per list, so ‹ Back returns to it.
    focus: Option<usize>,
}

impl ListView {
    fn new(uid: u64, title: String, back: Option<String>, rows: Vec<Row>) -> ListView {
        let mut tops = Vec::with_capacity(rows.len());
        let mut y = 0;
        for row in &rows {
            tops.push(y);
            y += row.height();
        }
        let has_index = back.is_none() && rows.iter().any(|r| matches!(r, Row::Header(_)));
        ListView {
            uid,
            title,
            back,
            rows,
            tops,
            height: y,
            scroll: 0.0,
            velocity: 0.0,
            has_index,
            focus: None,
        }
    }

    fn max_scroll(&self) -> f32 {
        (self.height - LIST_H).max(0) as f32
    }

    fn row_at(&self, list_y: f32) -> Option<usize> {
        let y = (list_y + self.scroll).floor() as i32;
        if y < 0 || y >= self.height {
            return None;
        }
        let i = self.tops.partition_point(|&top| top <= y);
        i.checked_sub(1)
    }

    /// Rows at least partly inside the list area.
    fn visible_rows(&self) -> std::ops::Range<usize> {
        let top = self.scroll.floor() as i32;
        let start = self.tops.partition_point(|&t| t <= top).saturating_sub(1);
        let end = self.tops.partition_point(|&t| t < top + LIST_H);
        start..end.max(start)
    }

    /// The section letter at the top of the list, for the index strip.
    fn current_letter(&self) -> Option<char> {
        let top = self.scroll.max(0.0) as i32;
        let mut letter = None;
        for (row, &row_top) in self.rows.iter().zip(&self.tops) {
            if row_top > top {
                break;
            }
            if let Row::Header(c) = row {
                letter = Some(*c);
            }
        }
        letter.or_else(|| {
            self.rows.iter().find_map(|r| match r {
                Row::Header(c) => Some(*c),
                _ => None,
            })
        })
    }

    /// Scroll so the section for `letter` (or the next one that exists) is
    /// at the top.
    fn jump_to(&mut self, letter: char) {
        let wanted = INDEX_LETTERS.find(letter).unwrap_or(0);
        let mut target = None;
        for (row, &top) in self.rows.iter().zip(&self.tops) {
            if let Row::Header(c) = row {
                let at = INDEX_LETTERS.find(*c).unwrap_or(usize::MAX);
                target = Some(top);
                if at >= wanted {
                    break;
                }
            }
        }
        if let Some(top) = target {
            self.scroll = (top as f32).min(self.max_scroll());
            self.velocity = 0.0;
        }
    }

    fn is_selectable(&self, row: usize) -> bool {
        !matches!(self.rows[row], Row::Header(_))
    }

    fn is_fully_visible(&self, row: usize) -> bool {
        let top = self.tops[row] as f32;
        let bottom = top + self.rows[row].height() as f32;
        top >= self.scroll && bottom <= self.scroll + LIST_H as f32
    }

    /// The first selectable row whose top is on screen (or the last one,
    /// if the list is scrolled past them all).
    fn first_visible_selectable(&self) -> Option<usize> {
        let top = self.scroll.max(0.0).ceil() as i32;
        (0..self.rows.len())
            .find(|&r| self.tops[r] >= top && self.is_selectable(r))
            .or_else(|| (0..self.rows.len()).rev().find(|&r| self.is_selectable(r)))
    }

    /// Scroll just enough to show `row` whole, and the section header just
    /// above it if it's the first of its section.
    fn scroll_to_show(&mut self, row: usize) {
        let top = if row > 0 && !self.is_selectable(row - 1) {
            self.tops[row - 1]
        } else {
            self.tops[row]
        };
        let bottom = self.tops[row] + self.rows[row].height();
        if (top as f32) < self.scroll {
            self.scroll = top as f32;
        } else if bottom as f32 > self.scroll + LIST_H as f32 {
            self.scroll = (bottom - LIST_H) as f32;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        self.velocity = 0.0;
    }

    fn focus_row(&mut self, row: usize) {
        self.focus = Some(row);
        self.scroll_to_show(row);
    }

    /// Put the focus on screen if it isn't: the user may have scrolled away
    /// by touch since the controller was last used.
    fn focus_visible(&mut self) {
        match self.focus {
            Some(row) if row < self.rows.len() && self.is_fully_visible(row) => {}
            _ => {
                self.focus = self.first_visible_selectable();
                self.velocity = 0.0;
            }
        }
    }

    /// Move the focus `steps` selectable rows down (up if negative),
    /// stopping at the ends. Without a focus, this only focuses the top
    /// visible row.
    fn move_focus(&mut self, steps: i32) {
        let Some(mut row) = self.focus else {
            if let Some(first) = self.first_visible_selectable() {
                self.focus_row(first);
            }
            return;
        };
        for _ in 0..steps.unsigned_abs() {
            let next = if steps > 0 {
                (row + 1..self.rows.len()).find(|&r| self.is_selectable(r))
            } else {
                (0..row).rev().find(|&r| self.is_selectable(r))
            };
            match next {
                Some(r) => row = r,
                None => break,
            }
        }
        self.focus_row(row);
    }

    /// D-pad left/right: the next A–Z section, or back to the start of the
    /// current one (then the one before). Lists without sections move a
    /// page instead.
    fn jump_section(&mut self, forward: bool) {
        if !self.rows.iter().any(|r| matches!(r, Row::Header(_))) {
            self.move_focus(if forward { ROWS_PER_PAGE } else { -ROWS_PER_PAGE });
            return;
        }
        let Some(current) = self.focus.or_else(|| self.first_visible_selectable()) else {
            return;
        };
        let header_before = |r: usize| (0..r).rev().find(|&h| !self.is_selectable(h));
        let first_after = |h: usize| (h + 1..self.rows.len()).find(|&r| self.is_selectable(r));
        let header = if forward {
            (current + 1..self.rows.len()).find(|&h| !self.is_selectable(h))
        } else {
            match header_before(current) {
                Some(h) if first_after(h) != Some(current) => Some(h),
                Some(h) => header_before(h),
                None => None,
            }
        };
        match header.and_then(|h| first_after(h).map(|r| (h, r))) {
            Some((header, row)) => {
                // Like the index strip: the section's header at the top.
                self.focus = Some(row);
                self.scroll = (self.tops[header] as f32).min(self.max_scroll());
                self.velocity = 0.0;
            }
            // Past the last section, or before the first: the very end or
            // the very start.
            None => {
                let end = if forward {
                    (0..self.rows.len()).rev().find(|&r| self.is_selectable(r))
                } else {
                    (0..self.rows.len()).find(|&r| self.is_selectable(r))
                };
                if let Some(row) = end {
                    self.focus_row(row);
                }
            }
        }
    }

    /// Advance momentum and the bounce back from overscroll. Returns true
    /// while there's still movement.
    fn step(&mut self, dt: f32) -> bool {
        let max = self.max_scroll();
        self.scroll += self.velocity * dt;
        let target = self.scroll.clamp(0.0, max);
        if self.scroll != target {
            // Rubber band: pull back to the edge, and kill the momentum
            // quickly.
            self.velocity *= (-dt * 20.0).exp();
            self.scroll += (target - self.scroll) * (1.0 - (-dt * 14.0).exp());
            if (target - self.scroll).abs() < 0.5 {
                self.scroll = target;
                self.velocity = 0.0;
            }
        } else {
            // About UIScrollView's normal deceleration rate.
            self.velocity *= (-dt * 2.0).exp();
            if self.velocity.abs() < 15.0 {
                self.velocity = 0.0;
            }
        }
        self.velocity != 0.0 || self.scroll != self.scroll.clamp(0.0, max)
    }
}

enum TouchKind {
    List {
        start: CGPoint,
        start_scroll: f32,
        last_y: f32,
        last_time: Instant,
        moved: bool,
        row: Option<usize>,
    },
    Index,
    Tab(usize),
    Cancel,
    Back,
    Ignored,
}

type NavBarKey = (String, Option<String>, Option<(Family, Glyph)>);

struct Held {
    role: Role,
    since: Instant,
    /// Repeats done so far.
    fired: u32,
}

struct Transition {
    /// The list area as it looked before the push/pop.
    from: Bitmap,
    start: Instant,
    forward: bool,
}

pub(super) struct Picker {
    /// The game's `iPodView2`. Not retained: it owns us.
    pub controller: id,
    /// Retained.
    pub view: id,
    pub tab: usize,
    stacks: [Vec<ListView>; 4],
    generation: u64,
    touch: Option<TouchKind>,
    /// Highlighted row of the current list, while touched.
    highlight: Option<usize>,
    /// The controller was used more recently than touch: show its focus
    /// and button icons.
    pad_mode: bool,
    /// Whose button icons to show.
    family: Family,
    /// A held direction, for repeating it.
    held: Option<Held>,
    /// Hidden while the game shows its confirmation panel.
    hidden: bool,
    /// Repeating `NSTimer`, retained, while something animates.
    timer: id,
    last_tick: Instant,
    transition: Option<Transition>,
    opened: Instant,
    row_cache: Vec<((u64, usize, bool), Rc<Bitmap>)>,
    /// Keyed by title, ‹ Back label and back button icon.
    nav_bar: Option<(NavBarKey, Rc<Bitmap>)>,
    /// Keyed by tab and the previous/next tab buttons' icons.
    tab_bar: Option<((usize, Option<(Family, (Glyph, Glyph))>), Rc<Bitmap>)>,
    index_strip: Option<(Option<char>, Rc<Bitmap>)>,
}

impl Picker {
    fn list(&self) -> &ListView {
        self.stacks[self.tab].last().unwrap()
    }
    fn list_mut(&mut self) -> &mut ListView {
        self.stacks[self.tab].last_mut().unwrap()
    }
}

/// Everything drawing needs besides the picker itself.
struct Ctx {
    art: Rc<Art>,
    fonts: Rc<Fonts>,
}

fn ctx(env: &mut Environment) -> Ctx {
    if env.framework_state.song_summoner.art.is_none() {
        let art = Rc::new(Art::load(env));
        env.framework_state.song_summoner.art = Some(art);
    }
    let state = &mut env.framework_state.song_summoner;
    let fonts = state
        .fonts
        .get_or_insert_with(|| Rc::new(Fonts::load()))
        .clone();
    Ctx {
        art: state.art.clone().unwrap(),
        fonts,
    }
}

fn next_uid(env: &mut Environment) -> u64 {
    let state = &mut env.framework_state.song_summoner;
    state.next_list_uid += 1;
    state.next_list_uid
}

fn song_row(song: &index::Song, subtitle_tab: usize, artwork: bool) -> Row {
    let subtitle = if subtitle_tab == TAB_ARTIST {
        song.display_album()
    } else {
        song.display_artist()
    };
    Row::Song {
        id: song.id,
        title: single_line(&song.title),
        subtitle: single_line(subtitle),
        artwork,
    }
}

/// Put a header row before each section's first entry.
fn with_headers(entries: Vec<Row>, sections: &[index::Section]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(entries.len() + sections.len());
    let mut next = sections.iter().peekable();
    for (i, row) in entries.into_iter().enumerate() {
        while let Some(section) = next.next_if(|s| s.first == i) {
            rows.push(Row::Header(section.letter));
        }
        rows.push(row);
    }
    rows
}

fn group_rows(library: &Library, groups: &[index::Group], artwork: bool) -> Vec<Row> {
    let entries = groups
        .iter()
        .map(|g| {
            let songs: Vec<u64> = g.songs.iter().map(|&i| library.songs[i].id).collect();
            let art = artwork.then(|| {
                g.songs
                    .iter()
                    .map(|&i| &library.songs[i])
                    .find(|s| s.has_art)
                    .map(|s| s.id)
            });
            Row::Group {
                name: single_line(&g.name),
                songs,
                artwork: art,
            }
        })
        .collect();
    with_headers(entries, &index::group_sections(groups))
}

fn build_root(env: &mut Environment, library: &Library, tab: usize) -> ListView {
    let rows = match tab {
        TAB_SONG => with_headers(
            library
                .by_title
                .iter()
                .map(|&i| song_row(&library.songs[i], TAB_SONG, false))
                .collect(),
            &library.title_sections(),
        ),
        TAB_ARTIST => group_rows(library, &library.artists, false),
        TAB_ALBUM => group_rows(library, &library.albums, true),
        _ => group_rows(library, &library.playlists, false),
    };
    let uid = next_uid(env);
    ListView::new(uid, TAB_NAMES[tab].to_string(), None, rows)
}

fn build_group(env: &mut Environment, name: &str, songs: &[u64], tab: usize) -> ListView {
    let library = library::current();
    let rows = songs
        .iter()
        .filter_map(|&id| library.get(id))
        .map(|song| song_row(song, tab, true))
        .collect();
    let uid = next_uid(env);
    ListView::new(
        uid,
        name.to_string(),
        Some(TAB_NAMES[tab].to_string()),
        rows,
    )
}

fn build_all_roots(env: &mut Environment) -> [Vec<ListView>; 4] {
    let library = library::current();
    [
        vec![build_root(env, &library, TAB_SONG)],
        vec![build_root(env, &library, TAB_ARTIST)],
        vec![build_root(env, &library, TAB_ALBUM)],
        vec![build_root(env, &library, TAB_PLAYLIST)],
    ]
}

// ---------------------------------------------------------------------------
// Lifecycle, used by picker_hook.rs
// ---------------------------------------------------------------------------

/// Make the picker for `controller`, starting on `tab`, and return its view.
pub(super) fn create(env: &mut Environment, controller: id, tab: usize) -> id {
    destroy(env, controller);
    // A song summoned since the last picker now has a trooper.
    env.framework_state.song_summoner.fighters.clear();
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: W as f32,
            height: H as f32,
        },
    };
    let view: id = msg_class![env; _touchHLE_SSPickerView alloc];
    let view: id = msg![env; view initWithFrame:frame];
    () = msg![env; view setTag:PICKER_VIEW_TAG];
    () = msg![env; view setOpaque:true];
    apply_rotation(env, view);

    let generation = library::generation();
    let stacks = build_all_roots(env);
    let picker = Picker {
        controller,
        view,
        tab: tab.min(3),
        stacks,
        generation,
        touch: None,
        highlight: None,
        pad_mode: false,
        family: Family::Xbox,
        held: None,
        hidden: false,
        timer: nil,
        last_tick: Instant::now(),
        transition: None,
        opened: Instant::now(),
        row_cache: Vec::new(),
        nav_bar: None,
        tab_bar: None,
        index_strip: None,
    };
    log!(
        "picker: open on tab {} ({} songs)",
        TAB_NAMES[picker.tab],
        library::current().len()
    );
    env.framework_state
        .song_summoner
        .pickers
        .insert(controller, picker);
    if library::is_scanning() {
        start_timer(env, controller);
    }
    // A new layer starts with nothing to show and isn't flagged for
    // drawing, so without this the picker stays black until a touch.
    () = msg![env; view setNeedsDisplay];
    view
}

pub(super) fn destroy(env: &mut Environment, controller: id) {
    stop_timer(env, controller);
    let Some(picker) = env
        .framework_state
        .song_summoner
        .pickers
        .remove(&controller)
    else {
        return;
    };
    log!("picker: closed");
    () = msg![env; (picker.view) removeFromSuperview];
    release(env, picker.view);
}

pub(super) fn view_of(env: &mut Environment, controller: id) -> id {
    env.framework_state
        .song_summoner
        .pickers
        .get(&controller)
        .map_or(nil, |p| p.view)
}

pub(super) fn tab_of(env: &mut Environment, controller: id) -> usize {
    env.framework_state
        .song_summoner
        .pickers
        .get(&controller)
        .map_or(0, |p| p.tab)
}

/// The game hides the picker while its confirmation panel is up, and shows
/// the same one again after "No", so the scroll position is kept. Only a
/// half-finished touch or fling is dropped.
/// The Setup menu is opening and takes every release until it closes:
/// stop repeating a held direction now, or it would scroll on and on.
pub(super) fn release_held(env: &mut Environment) {
    for picker in env.framework_state.song_summoner.pickers.values_mut() {
        picker.held = None;
    }
}

pub(super) fn set_hidden(env: &mut Environment, controller: id, hidden: bool) {
    let Some(picker) = env
        .framework_state
        .song_summoner
        .pickers
        .get_mut(&controller)
    else {
        return;
    };
    picker.touch = None;
    picker.highlight = None;
    picker.held = None;
    picker.hidden = hidden;
    picker.list_mut().velocity = 0.0;
    let view = picker.view;
    log!("picker: {}", if hidden { "hide" } else { "show" });
    () = msg![env; view setHidden:hidden];
    if !hidden {
        // One tick picks up a library rescan that finished while hidden.
        start_timer(env, controller);
        () = msg![env; view setNeedsDisplay];
    }
}

/// UIKit views live in the portrait window; turn ours to match the
/// landscape screen the game draws in.
pub(super) fn apply_rotation(env: &mut Environment, view: id) {
    let angle = match env.window.as_ref().map(|w| w.current_rotation()) {
        Some(DeviceOrientation::LandscapeLeft) => std::f32::consts::FRAC_PI_2,
        Some(DeviceOrientation::LandscapeRight) => -std::f32::consts::FRAC_PI_2,
        _ => 0.0,
    };
    let screen: id = msg_class![env; UIScreen mainScreen];
    let screen_bounds: CGRect = msg![env; screen bounds];
    let center = CGPoint {
        x: screen_bounds.size.width / 2.0,
        y: screen_bounds.size.height / 2.0,
    };
    () = msg![env; view setCenter:center];
    let transform = CGAffineTransform::make_rotation(angle);
    () = msg![env; view setTransform:transform];
}

// ---------------------------------------------------------------------------
// Animation timer
// ---------------------------------------------------------------------------

fn start_timer(env: &mut Environment, controller: id) {
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return;
    };
    if picker.timer != nil {
        return;
    }
    picker.last_tick = Instant::now();
    let view = picker.view;
    let selector: SEL = env
        .objc
        .register_host_selector("_touchHLE_tick:".to_string(), &mut env.mem);
    let interval: NSTimeInterval = FRAME_SECONDS;
    let timer: id = msg_class![env; NSTimer scheduledTimerWithTimeInterval:interval
                                                                   target:view
                                                                 selector:selector
                                                                 userInfo:nil
                                                                  repeats:true];
    retain(env, timer);
    if let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) {
        picker.timer = timer;
    }
}

fn stop_timer(env: &mut Environment, controller: id) {
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return;
    };
    let timer = std::mem::replace(&mut picker.timer, nil);
    if timer != nil {
        () = msg![env; timer invalidate];
        release(env, timer);
    }
}

fn controller_for_view(env: &mut Environment, view: id) -> Option<id> {
    env.framework_state
        .song_summoner
        .pickers
        .values()
        .find(|p| p.view == view)
        .map(|p| p.controller)
}

/// One animation frame. Returns true while anything is still moving.
fn tick(env: &mut Environment, controller: id) -> bool {
    // A finished background scan swaps the library: rebuild the lists.
    let generation = library::generation();
    let stale = env
        .framework_state
        .song_summoner
        .pickers
        .get(&controller)
        .is_some_and(|p| p.generation != generation);
    if stale {
        let stacks = build_all_roots(env);
        if let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) {
            log!("picker: library changed, rebuilding lists");
            picker.stacks = stacks;
            picker.generation = generation;
            picker.row_cache.clear();
            picker.highlight = None;
            picker.touch = None;
        }
    }

    let (repeat_delay, repeat_interval) = super::setup::repeat_timing(env);
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return false;
    };
    let now = Instant::now();
    let dt = now.duration_since(picker.last_tick).as_secs_f32().min(0.1);
    picker.last_tick = now;

    let dragging = matches!(picker.touch, Some(TouchKind::List { .. }));
    let mut moving = false;
    if !dragging {
        moving |= picker.list_mut().step(dt);
    }
    if let Some(held) = &mut picker.held {
        let due = pad::repeats_due_at(
            held.since.elapsed().as_secs_f32(),
            repeat_delay,
            repeat_interval,
        );
        // At most a few per frame, in case a frame came very late.
        let extra = due.saturating_sub(held.fired).min(4);
        held.fired = due;
        let role = held.role;
        for _ in 0..extra {
            pad_action(picker.list_mut(), role, false);
        }
        moving = true;
    }
    if let Some(t) = &picker.transition {
        if t.start.elapsed().as_secs_f32() >= SLIDE_SECONDS {
            picker.transition = None;
        } else {
            moving = true;
        }
    }
    moving |= library::is_scanning();
    moving
}

// ---------------------------------------------------------------------------
// Touch handling
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Action {
    None,
    Pick(u64),
    Cancel,
    Push(usize),
    Pop,
    SwitchTab(usize),
}

fn touch_point(env: &mut Environment, view: id, touches: id) -> Option<CGPoint> {
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return None;
    }
    Some(msg![env; touch locationInView:view])
}

fn cancel_button_x(fonts: &Fonts) -> i32 {
    let width = fonts.width(true, 12.0, "Cancel").ceil() as i32 + 20;
    W - width - 6
}

fn back_button_width(fonts: &Fonts, label: &str) -> i32 {
    fonts.width(true, 12.0, &format!("‹ {label}")).ceil() as i32 + 20
}

fn index_letter_at(y: f32) -> char {
    let step = (LIST_H - 8) as f32 / INDEX_LETTERS.len() as f32;
    let i = ((y - NAV_H as f32 - 4.0) / step).floor().clamp(0.0, 26.0) as usize;
    INDEX_LETTERS.chars().nth(i).unwrap_or('#')
}

fn touches_began(env: &mut Environment, view: id, touches: id) {
    let Some(controller) = controller_for_view(env, view) else {
        return;
    };
    let Some(p) = touch_point(env, view, touches) else {
        return;
    };
    let fonts = ctx(env).fonts;
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return;
    };
    // Touch takes over from the controller: hide its focus and icons.
    picker.pad_mode = false;
    picker.held = None;
    if picker.transition.is_some() {
        picker.touch = Some(TouchKind::Ignored);
        return;
    }
    let (x, y) = (p.x, p.y);
    let kind = if y < NAV_H as f32 {
        if x >= (cancel_button_x(&fonts) - 6) as f32 {
            TouchKind::Cancel
        } else if let Some(back) = &picker.list().back {
            if x <= (back_button_width(&fonts, back) + 12) as f32 {
                TouchKind::Back
            } else {
                TouchKind::Ignored
            }
        } else {
            TouchKind::Ignored
        }
    } else if y >= (H - TAB_H) as f32 {
        TouchKind::Tab(((x / TAB_W as f32) as usize).min(3))
    } else if picker.list().has_index && x >= (INDEX_X - 8) as f32 {
        let letter = index_letter_at(y);
        picker.list_mut().jump_to(letter);
        TouchKind::Index
    } else {
        let list = picker.list_mut();
        // Touching a flinging list just stops it, as on iOS.
        let was_moving = list.velocity != 0.0;
        list.velocity = 0.0;
        let row = if was_moving {
            None
        } else {
            list.row_at(y - NAV_H as f32)
                .filter(|&r| !matches!(list.rows[r], Row::Header(_)))
        };
        picker.highlight = row;
        TouchKind::List {
            start: p,
            start_scroll: picker.list().scroll,
            last_y: y,
            last_time: Instant::now(),
            moved: false,
            row,
        }
    };
    picker.touch = Some(kind);
    () = msg![env; view setNeedsDisplay];
}

fn touches_moved(env: &mut Environment, view: id, touches: id) {
    let Some(controller) = controller_for_view(env, view) else {
        return;
    };
    let Some(p) = touch_point(env, view, touches) else {
        return;
    };
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return;
    };
    let mut clear_highlight = false;
    match picker.touch {
        Some(TouchKind::Index) => {
            let letter = index_letter_at(p.y);
            picker.list_mut().jump_to(letter);
        }
        Some(TouchKind::List {
            start,
            start_scroll,
            ref mut last_y,
            ref mut last_time,
            ref mut moved,
            ..
        }) => {
            let dx = p.x - start.x;
            let dy = p.y - start.y;
            if !*moved && dx.abs().max(dy.abs()) > TAP_SLOP {
                *moved = true;
                clear_highlight = true;
            }
            let now = Instant::now();
            let dt = now.duration_since(*last_time).as_secs_f32().max(1.0 / 240.0);
            let instant_velocity = -(p.y - *last_y) / dt;
            *last_y = p.y;
            *last_time = now;
            let moved_now = *moved;
            let list = picker.stacks[picker.tab].last_mut().unwrap();
            if moved_now {
                let max = list.max_scroll();
                let raw = start_scroll - dy;
                // Past the ends the content follows the finger at half speed.
                list.scroll = if raw < 0.0 {
                    raw * 0.5
                } else if raw > max {
                    max + (raw - max) * 0.5
                } else {
                    raw
                };
                list.velocity = 0.6 * instant_velocity + 0.4 * list.velocity;
            }
        }
        _ => {}
    }
    if clear_highlight {
        picker.highlight = None;
    }
    () = msg![env; view setNeedsDisplay];
}

fn touches_ended(env: &mut Environment, view: id, touches: id, cancelled: bool) {
    let Some(controller) = controller_for_view(env, view) else {
        return;
    };
    let p = touch_point(env, view, touches).unwrap_or(CGPoint { x: -1.0, y: -1.0 });
    let fonts = ctx(env).fonts;
    let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) else {
        return;
    };
    let touch = picker.touch.take();
    picker.highlight = None;
    let mut animate = false;
    let action = match touch {
        _ if cancelled => {
            animate = true;
            Action::None
        }
        Some(TouchKind::Cancel) if p.y < NAV_H as f32 && p.x >= (cancel_button_x(&fonts) - 6) as f32 => {
            Action::Cancel
        }
        Some(TouchKind::Back) if p.y < NAV_H as f32 => Action::Pop,
        Some(TouchKind::Tab(tab)) if p.y >= (H - TAB_H) as f32 && ((p.x / TAB_W as f32) as usize).min(3) == tab => {
            Action::SwitchTab(tab)
        }
        Some(TouchKind::List {
            moved,
            row,
            last_time,
            ..
        }) => {
            let list = picker.list_mut();
            if moved {
                // A finger that stopped before lifting doesn't fling.
                if last_time.elapsed().as_secs_f32() > 0.1 {
                    list.velocity = 0.0;
                }
                animate = true;
                Action::None
            } else {
                let lifted_on = list.row_at(p.y - NAV_H as f32);
                match row {
                    Some(r) if lifted_on == Some(r) => {
                        // A controller picks up from the tapped row.
                        list.focus = Some(r);
                        match &list.rows[r] {
                            Row::Song { id, .. } => Action::Pick(*id),
                            Row::Group { .. } => Action::Push(r),
                            Row::Header(_) => Action::None,
                        }
                    }
                    _ => {
                        animate = true;
                        Action::None
                    }
                }
            }
        }
        _ => Action::None,
    };

    animate |= perform(env, controller, action);
    if animate {
        start_timer(env, controller);
    }
    () = msg![env; view setNeedsDisplay];
}

/// Carry out a tap's or a button's result. Returns true if it started an
/// animation.
fn perform(env: &mut Environment, controller: id, action: Action) -> bool {
    let mut animate = false;
    match action {
        Action::None => {}
        Action::Pick(pid) => {
            log!("picker: pick {:016X}", pid);
            let number: id = msg_class![env; NSNumber numberWithUnsignedLongLong:pid];
            () = msg![env; controller mediaPicker:controller didPickMediaNumber:number];
        }
        Action::Cancel => {
            log!("picker: cancel");
            () = msg![env; controller mediaPickerDidCancel:controller];
        }
        Action::Push(row) => {
            let snapshot = snapshot_list(env, controller);
            let target = env
                .framework_state
                .song_summoner
                .pickers
                .get(&controller)
                .and_then(|picker| match &picker.list().rows[row] {
                    Row::Group { name, songs, .. } => {
                        Some((name.clone(), songs.clone(), picker.tab))
                    }
                    _ => None,
                });
            if let Some((name, songs, tab)) = target {
                let list = build_group(env, &name, &songs, tab);
                if let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller)
                {
                    picker.stacks[picker.tab].push(list);
                    if picker.pad_mode {
                        picker.list_mut().focus_visible();
                    }
                    picker.transition = snapshot.map(|from| Transition {
                        from,
                        start: Instant::now(),
                        forward: true,
                    });
                }
                animate = true;
            }
        }
        Action::Pop => {
            let snapshot = snapshot_list(env, controller);
            if let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) {
                if picker.stacks[picker.tab].len() > 1 {
                    picker.stacks[picker.tab].pop();
                    picker.transition = snapshot.map(|from| Transition {
                        from,
                        start: Instant::now(),
                        forward: false,
                    });
                    animate = true;
                }
            }
        }
        Action::SwitchTab(tab) => {
            if let Some(picker) = env.framework_state.song_summoner.pickers.get_mut(&controller) {
                // Back to the tab's top level, where it was scrolled to.
                picker.stacks[tab].truncate(1);
                picker.tab = tab;
                picker.list_mut().velocity = 0.0;
                if picker.pad_mode {
                    picker.list_mut().focus_visible();
                }
            }
        }
    }
    animate
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

/// What a controller command does to the current list. Tab switching is the
/// picker's, not the list's, so [Role::PrevTab]/[Role::NextTab] do nothing
/// here. `waking`: this is the first press since touch was used, which only
/// brings the focus back into view (so a stray press can't pick a song).
fn pad_action(list: &mut ListView, role: Role, waking: bool) -> Action {
    let moves = matches!(
        role,
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection | Role::Confirm
    );
    if waking && moves {
        list.focus_visible();
        return Action::None;
    }
    match role {
        Role::Up => list.move_focus(-1),
        Role::Down => list.move_focus(1),
        Role::PrevSection => list.jump_section(false),
        Role::NextSection => list.jump_section(true),
        Role::Confirm => {
            match list.focus.map(|r| (r, &list.rows[r])) {
                Some((_, Row::Song { id, .. })) => return Action::Pick(*id),
                Some((r, Row::Group { .. })) => return Action::Push(r),
                _ => {}
            }
            // Nothing focused yet: focus something, don't act.
            list.move_focus(0);
        }
        Role::Back => {
            return if list.back.is_some() {
                Action::Pop
            } else {
                Action::Cancel
            };
        }
        Role::PrevTab
        | Role::NextTab
        | Role::Info
        | Role::Skip
        | Role::ZoomOut
        | Role::ZoomIn => {}
    }
    Action::None
}

/// A controller button or key went down or up, already mapped to its
/// command. `family` is the pad's make, for its icons (`None` for a key).
/// Returns false if there's no picker on screen to take it.
pub(super) fn handle_role(
    env: &mut Environment,
    role: Role,
    pressed: bool,
    family: Option<Family>,
) -> bool {
    // While the picker is hidden, the game's own confirmation panel is up,
    // and that's the game's to handle.
    let Some(controller) = env
        .framework_state
        .song_summoner
        .pickers
        .values()
        .find(|p| !p.hidden)
        .map(|p| p.controller)
    else {
        return false;
    };
    // Buttons the picker has no use for still don't reach the game.
    // The triggers only zoom battle's map.
    if matches!(role, Role::ZoomOut | Role::ZoomIn) {
        return true;
    }
    let picker = env
        .framework_state
        .song_summoner
        .pickers
        .get_mut(&controller)
        .unwrap();
    if !pressed {
        if picker.held.as_ref().is_some_and(|h| h.role == role) {
            picker.held = None;
        }
        return true;
    }
    // Mid-slide, or while a finger is down, the list is spoken for.
    if picker.transition.is_some() || picker.touch.is_some() || library::is_scanning() {
        return true;
    }
    if let Some(family) = family {
        picker.family = family;
    }
    let waking = !picker.pad_mode;
    picker.pad_mode = true;
    let action = match role {
        Role::PrevTab => Action::SwitchTab(pad::tab_after(picker.tab, -1)),
        Role::NextTab => Action::SwitchTab(pad::tab_after(picker.tab, 1)),
        _ => pad_action(picker.list_mut(), role, waking),
    };
    picker.held = pad::repeats(role).then(|| Held {
        role,
        since: Instant::now(),
        fired: 0,
    });
    let view = picker.view;
    let holding = picker.held.is_some();
    if perform(env, controller, action) || holding {
        start_timer(env, controller);
    }
    () = msg![env; view setNeedsDisplay];
    true
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// The portrait `-[MusicLibraryCell fighterImageDraw]` draws for a fighter
/// id from `IPD_ID2Fighter` (see song-summoner-re.md, "Fighter portraits").
fn portrait_for_fid(fid: i32) -> Option<u32> {
    match fid {
        -2 => None,
        // Never summoned: the teal music note.
        -1 => Some(89),
        // Summoned, but the fighter's look has since changed: "MUSIC
        // FIGHTER".
        999 => Some(87),
        // "?"
        998 => Some(88),
        n => (0..FIGHTER_COUNT as i32).contains(&n).then_some(n as u32),
    }
}

/// Song Summoner's own picker gets each song's portrait from its persistent
/// ID with `IPD_ID2Fighter(unsigned long long)`, which checks the save
/// data's trooper roster: songs that were never summoned get a default
/// image, summoned ones their trooper. We call the game's function so the
/// picker matches it exactly.
fn fighter_for(env: &mut Environment, pid: u64) -> Option<u32> {
    if let Some(&known) = env.framework_state.song_summoner.fighters.get(&pid) {
        return known;
    }
    let function = *env
        .framework_state
        .song_summoner
        .id_to_fighter
        .get_or_insert_with(|| {
            let addr = env.bins.first()?.exported_symbols.get("__Z14IPD_ID2Fightery").copied();
            if addr.is_none() {
                log!("picker: IPD_ID2Fighter not found, no fighter portraits");
            }
            // The symbol table lacks the Thumb flag for the game's C++
            // functions, but they are all Thumb (song-summoner-re.md §2).
            addr.map(|a| GuestFunction::from_addr_and_thumb_flag(a & !1, true))
        });
    let fighter = function.and_then(|f| {
        let fid: i32 = f.call_from_host(env, (pid,));
        portrait_for_fid(fid)
    });
    let state = &mut env.framework_state.song_summoner;
    if state.fighters.len() < 8 {
        log!("picker: fighter for {:016X} is {:?}", pid, fighter);
    }
    state.fighters.insert(pid, fighter);
    fighter
}

/// Look up portraits for the song rows about to be drawn. This calls into
/// the game, so it happens before drawing borrows the picker.
fn resolve_fighters(env: &mut Environment, controller: id) {
    let Some(picker) = env.framework_state.song_summoner.pickers.get(&controller) else {
        return;
    };
    let fighters = &env.framework_state.song_summoner.fighters;
    let list = picker.list();
    let missing: Vec<u64> = list
        .visible_rows()
        .filter_map(|r| match list.rows[r] {
            Row::Song { id, .. } if !fighters.contains_key(&id) => Some(id),
            _ => None,
        })
        .collect();
    for pid in missing {
        fighter_for(env, pid);
    }
}

fn draw_row_background(canvas: &mut Canvas, art: &Art, y: i32, highlighted: bool) {
    let image = if highlighted { &art.touch_bg } else { &art.cellbg };
    match image {
        Some(image) => canvas.blit(image, 0, y, 1.0),
        None if highlighted => {
            canvas.fill_rect(0, y, W, ROW_H, (20, 60, 110), 1.0);
            canvas.rounded_rect(1, y + 1, W - 2, ROW_H - 2, 3.0, None, Some(NAV_RULE));
        }
        None => canvas.vgradient(0, y, W, ROW_H, (40, 40, 44), (10, 10, 12)),
    }
}

fn draw_thumbnail(canvas: &mut Canvas, art: &Art, song_art: Option<u64>, y: i32) {
    let user_art = song_art
        .and_then(artwork::load)
        .map(|b| b.scaled(50, 50));
    match (user_art, &art.noartwork) {
        (Some(bitmap), _) => canvas.blit(&bitmap, 4, y + 2, 1.0),
        (None, Some(placeholder)) => canvas.blit(placeholder, 4, y + 2, 1.0),
        (None, None) => canvas.fill_rect(4, y + 2, 50, 50, (60, 60, 66), 1.0),
    }
}

fn render_row(
    row: &Row,
    highlighted: bool,
    ctx: &Ctx,
    fighters: &HashMap<u64, Option<u32>>,
) -> Bitmap {
    let mut canvas = Canvas::new(W, row.height());
    let fonts = &ctx.fonts;
    match row {
        Row::Header(letter) => {
            canvas.vgradient(0, 0, W, HDR_H, HEADER_TOP, HEADER_BOTTOM);
            canvas.text(fonts, true, 14.0, &letter.to_string(), 12.0, 3.0, WHITE);
        }
        Row::Song {
            id,
            title,
            subtitle,
            artwork,
        } => {
            draw_row_background(&mut canvas, &ctx.art, 0, highlighted);
            if let Some(Some(n)) = fighters.get(id) {
                if let Some(portrait) = ctx.art.fighter(*n) {
                    canvas.blit(&portrait, FIGHTER_X, 0, 1.0);
                }
            }
            let x = if *artwork {
                let has_art = library::current().get(*id).is_some_and(|s| s.has_art);
                draw_thumbnail(&mut canvas, &ctx.art, has_art.then_some(*id), 0);
                ART_TEXT_X
            } else {
                TEXT_X
            };
            let max = (TEXT_MAX_X - x) as f32;
            let title = fonts.ellipsize(true, 16.0, title, max);
            let subtitle = fonts.ellipsize(false, 13.0, subtitle, max);
            canvas.text(fonts, true, 16.0, &title, x as f32, 9.0, WHITE);
            canvas.text(fonts, false, 13.0, &subtitle, x as f32, 31.0, GREY);
        }
        Row::Group {
            name,
            songs,
            artwork,
        } => {
            draw_row_background(&mut canvas, &ctx.art, 0, highlighted);
            let x = match artwork {
                Some(song_art) => {
                    draw_thumbnail(&mut canvas, &ctx.art, *song_art, 0);
                    ART_TEXT_X
                }
                None => TEXT_X,
            };
            let count = if songs.len() == 1 {
                "1 song  ›".to_string()
            } else {
                format!("{} songs  ›", songs.len())
            };
            let count_width = fonts.width(false, 13.0, &count);
            let count_x = W as f32 - 30.0 - count_width;
            canvas.text(fonts, false, 13.0, &count, count_x.round(), 20.0, GREY);
            let name = fonts.ellipsize(true, 16.0, name, count_x - 12.0 - x as f32);
            canvas.text(fonts, true, 16.0, &name, x as f32, 17.0, WHITE);
        }
    }
    canvas.bitmap
}

fn cached_row(
    picker: &mut Picker,
    row_index: usize,
    ctx: &Ctx,
    fighters: &HashMap<u64, Option<u32>>,
) -> Rc<Bitmap> {
    let focused = picker.pad_mode && picker.list().focus == Some(row_index);
    let highlighted =
        (picker.highlight == Some(row_index) || focused) && picker.transition.is_none();
    let key = (picker.list().uid, row_index, highlighted);
    if let Some(pos) = picker.row_cache.iter().position(|(k, _)| *k == key) {
        let entry = picker.row_cache.remove(pos);
        let bitmap = entry.1.clone();
        picker.row_cache.push(entry);
        return bitmap;
    }
    let bitmap = Rc::new(render_row(
        &picker.list().rows[row_index],
        highlighted,
        ctx,
        fighters,
    ));
    if picker.row_cache.len() >= ROW_CACHE_SIZE {
        picker.row_cache.remove(0);
    }
    picker.row_cache.push((key, bitmap.clone()));
    bitmap
}

/// The list area (480×220) of the current list.
fn render_list_area(
    picker: &mut Picker,
    ctx: &Ctx,
    fighters: &HashMap<u64, Option<u32>>,
) -> Bitmap {
    let mut canvas = Canvas::new(W, LIST_H);
    canvas.fill_rect(0, 0, W, LIST_H, BACKGROUND, 1.0);
    let scroll = picker.list().scroll.round() as i32;
    let visible = picker.list().visible_rows();
    for r in visible {
        let top = picker.list().tops[r] - scroll;
        let bitmap = cached_row(picker, r, ctx, fighters);
        canvas.blit(&bitmap, 0, top, 1.0);
    }
    if picker.list().rows.is_empty() && !library::is_scanning() {
        let message = if library::current().is_empty() {
            "No songs found"
        } else {
            "No playlists: put songs in folders"
        };
        canvas.text_centered(
            &ctx.fonts,
            true,
            14.0,
            message,
            (W / 2) as f32,
            (LIST_H / 2 - 10) as f32,
            GREY,
        );
    }
    if picker.list().has_index {
        let letter = picker.list().current_letter();
        let strip = match &picker.index_strip {
            Some((cached, bitmap)) if *cached == letter => bitmap.clone(),
            _ => {
                let bitmap = Rc::new(render_index_strip(&ctx.fonts, letter));
                picker.index_strip = Some((letter, bitmap.clone()));
                bitmap
            }
        };
        canvas.blit(&strip, INDEX_X, 2, 1.0);
    }
    canvas.bitmap
}

fn render_index_strip(fonts: &Fonts, active: Option<char>) -> Bitmap {
    let height = LIST_H - 4;
    let mut canvas = Canvas::new(INDEX_W, height);
    canvas.rounded_rect(0, 0, INDEX_W, height, 7.0, Some(((0, 0, 0), 130.0 / 255.0)), None);
    let step = (LIST_H - 8) as f32 / INDEX_LETTERS.len() as f32;
    for (i, letter) in INDEX_LETTERS.chars().enumerate() {
        let rgb = if Some(letter) == active { CYAN } else { INDEX_TEXT };
        canvas.text_centered(
            fonts,
            true,
            7.0,
            &letter.to_string(),
            (INDEX_W / 2) as f32,
            2.0 + i as f32 * step,
            rgb,
        );
    }
    canvas.bitmap
}

/// `glyph`: the controller's back button icon, drawn beside whichever of
/// ‹ Back or Cancel it presses.
fn render_nav_bar(
    fonts: &Fonts,
    title: &str,
    back: Option<&str>,
    glyph: Option<&Bitmap>,
) -> Bitmap {
    let mut canvas = Canvas::new(W, NAV_H);
    canvas.vgradient(0, 0, W, NAV_H, NAV_TOP, NAV_BOTTOM);
    canvas.fill_rect(0, NAV_H - 2, W, 1, NAV_RULE, 1.0);
    canvas.fill_rect(0, NAV_H - 1, W, 1, (0, 0, 0), 1.0);
    let title = fonts.ellipsize(true, 19.0, &single_line(title), 250.0);
    canvas.text_centered(fonts, true, 19.0, &title, (W / 2) as f32, 11.0, WHITE);

    let button = |canvas: &mut Canvas, x: i32, label: &str| {
        let width = fonts.width(true, 12.0, label).ceil() as i32 + 20;
        canvas.rounded_rect(x, 8, width, 27, 5.0, Some((BUTTON_FILL, 1.0)), Some(BUTTON_STROKE));
        canvas.text(fonts, true, 12.0, label, (x + 10) as f32, 14.0, WHITE);
    };
    button(&mut canvas, cancel_button_x(fonts), "Cancel");
    if let Some(back) = back {
        button(&mut canvas, 6, &format!("‹ {back}"));
    }
    if let Some(glyph) = glyph {
        let size = glyph.width as i32;
        let x = match back {
            Some(back) => 6 + back_button_width(fonts, back) + 4,
            None => cancel_button_x(fonts) - 4 - size,
        };
        canvas.blit(glyph, x, 8 + (27 - size) / 2, 1.0);
    }
    canvas.bitmap
}

/// `bumpers`: the controller's left and right shoulder button icons, drawn
/// at the ends of the bar since they switch tabs.
fn render_tab_bar(
    ctx: &Ctx,
    selected: usize,
    bumpers: Option<(&Bitmap, &Bitmap)>,
) -> Bitmap {
    let mut canvas = Canvas::new(W, TAB_H);
    canvas.vgradient(0, 0, W, TAB_H, TAB_TOP, (0, 0, 0));
    canvas.fill_rect(0, 0, W, 1, TAB_RULE, 1.0);
    if let Some((left, right)) = bumpers {
        let y = (TAB_H - left.height as i32) / 2;
        canvas.blit(left, 3, y, 0.8);
        canvas.blit(right, W - 3 - right.width as i32, y, 0.8);
    }
    for (i, label) in TAB_NAMES.iter().enumerate() {
        let x = i as i32 * TAB_W;
        let is_selected = i == selected;
        if is_selected {
            canvas.rounded_rect(
                x + 20,
                3,
                TAB_W - 40,
                TAB_H - 6,
                5.0,
                Some((TAB_PLATE, 1.0)),
                None,
            );
        }
        let icon_x = x + (TAB_W - 40) / 2;
        match &ctx.art.tab_icons[i] {
            Some(icon) if is_selected => canvas.blit_tinted(icon, icon_x, 2, CYAN),
            Some(icon) => canvas.blit(icon, icon_x, 2, 0.45),
            None => {
                let rgb = if is_selected { CYAN } else { GREY };
                canvas.rounded_rect(icon_x + 8, 8, 24, 24, 4.0, None, Some(rgb));
            }
        }
        let rgb = if is_selected { CYAN } else { GREY };
        canvas.text_centered(
            &ctx.fonts,
            true,
            10.0,
            label,
            (x + TAB_W / 2) as f32,
            42.0,
            rgb,
        );
    }
    canvas.bitmap
}

fn render_loading(canvas: &mut Canvas, ctx: &Ctx, elapsed: f32) {
    canvas.reset_clip();
    canvas.fill_rect(0, 0, W, H, (0, 0, 0), 170.0 / 255.0);
    if !ctx.art.cube.is_empty() {
        let frame = (elapsed * LOADING_FPS) as usize % ctx.art.cube.len();
        let cube = &ctx.art.cube[frame];
        canvas.blit(cube, (W - cube.width as i32) / 2, 118, 1.0);
    }
    canvas.text_centered(
        &ctx.fonts,
        true,
        14.0,
        "Reading your music library…",
        (W / 2) as f32,
        178.0,
        CYAN,
    );
    let count = library::scan_progress();
    let count_text = if count == 1 {
        "1 song".to_string()
    } else {
        format!("{} songs", with_thousands(count))
    };
    canvas.text_centered(
        &ctx.fonts,
        false,
        12.0,
        &count_text,
        (W / 2) as f32,
        198.0,
        LOADING_COUNT,
    );
}

fn with_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The list area as it looks now, for the slide animation.
fn snapshot_list(env: &mut Environment, controller: id) -> Option<Bitmap> {
    resolve_fighters(env, controller);
    let ctx = ctx(env);
    let state = &mut env.framework_state.song_summoner;
    let picker = state.pickers.get_mut(&controller)?;
    Some(render_list_area(picker, &ctx, &state.fighters))
}

/// The whole 480×320 picker.
fn render(env: &mut Environment, controller: id) -> Option<Bitmap> {
    resolve_fighters(env, controller);
    let ctx = ctx(env);
    // The player's own buttons for back and the tabs.
    let back_glyph = pad::glyph_of(super::setup::button_for(env, Role::Back));
    let tab_glyphs = (
        pad::glyph_of(super::setup::button_for(env, Role::PrevTab)),
        pad::glyph_of(super::setup::button_for(env, Role::NextTab)),
    );
    let state = &mut env.framework_state.song_summoner;
    let picker = state.pickers.get_mut(&controller)?;
    let fighters = &state.fighters;
    // The controller's icons, only while it's in use.
    let family = picker.pad_mode.then_some(picker.family);

    let mut canvas = Canvas::new(W, H);
    canvas.fill_rect(0, 0, W, H, BACKGROUND, 1.0);

    let list_area = render_list_area(picker, &ctx, fighters);
    match &picker.transition {
        Some(t) => {
            let progress = (t.start.elapsed().as_secs_f32() / SLIDE_SECONDS).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - progress).powi(3);
            let direction = if t.forward { 1.0 } else { -1.0 };
            let old_x = (-direction * W as f32 * eased).round() as i32;
            let new_x = (direction * W as f32 * (1.0 - eased)).round() as i32;
            canvas.set_clip(0, NAV_H, W, LIST_H);
            canvas.blit(&t.from, old_x, NAV_H, 1.0);
            canvas.blit(&list_area, new_x, NAV_H, 1.0);
            canvas.reset_clip();
        }
        None => canvas.blit(&list_area, 0, NAV_H, 1.0),
    }

    let nav_key = (
        picker.list().title.clone(),
        picker.list().back.clone(),
        family.map(|f| (f, back_glyph)),
    );
    let nav_bar = match &picker.nav_bar {
        Some((key, bitmap)) if *key == nav_key => bitmap.clone(),
        _ => {
            let glyph = nav_key
                .2
                .map(|(f, g)| state.glyphs.get(f, g, PROMPT_GLYPH_SIZE));
            let bitmap = Rc::new(render_nav_bar(
                &ctx.fonts,
                &nav_key.0,
                nav_key.1.as_deref(),
                glyph.as_deref(),
            ));
            picker.nav_bar = Some((nav_key, bitmap.clone()));
            bitmap
        }
    };
    canvas.blit(&nav_bar, 0, 0, 1.0);

    let tab_key = (picker.tab, family.map(|f| (f, tab_glyphs)));
    let tab_bar = match &picker.tab_bar {
        Some((key, bitmap)) if *key == tab_key => bitmap.clone(),
        _ => {
            let bumpers = family.map(|f| {
                (
                    state.glyphs.get(f, tab_glyphs.0, PROMPT_GLYPH_SIZE),
                    state.glyphs.get(f, tab_glyphs.1, PROMPT_GLYPH_SIZE),
                )
            });
            let bumpers = bumpers.as_ref().map(|(l, r)| (&**l, &**r));
            let bitmap = Rc::new(render_tab_bar(&ctx, picker.tab, bumpers));
            picker.tab_bar = Some((tab_key, bitmap.clone()));
            bitmap
        }
    };
    canvas.blit(&tab_bar, 0, H - TAB_H, 1.0);

    if library::is_scanning() {
        render_loading(&mut canvas, &ctx, picker.opened.elapsed().as_secs_f32());
    }
    Some(canvas.bitmap)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_SSPickerView: UIView

- (())drawRect:(CGRect)_rect {
    let Some(controller) = controller_for_view(env, this) else {
        return;
    };
    let Some(bitmap) = render(env, controller) else {
        return;
    };
    let context = UIGraphicsGetCurrentContext(env);
    let image = crate::image::Image::from_pixel_vec(bitmap.pixels, (bitmap.width, bitmap.height));
    let image = cg_image::from_image(env, image);
    let rect = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: W as f32,
            height: H as f32,
        },
    };
    // As on a real iPhone, drawRect:'s context is top-down (UIKit) but
    // CGContextDrawImage draws bottom-up (Quartz), which would put the
    // picker upside down. Flip the context first. touchHLE keeps the
    // layer's context between draws, so the flip must be undone after,
    // or every other frame comes out flipped back.
    CGContextSaveGState(env, context);
    CGContextTranslateCTM(env, context, 0.0, H as f32);
    CGContextScaleCTM(env, context, 1.0, -1.0);
    CGContextDrawImage(env, context, rect, image);
    CGContextRestoreGState(env, context);
    CGImageRelease(env, image);
}

- (())touchesBegan:(id)touches withEvent:(id)_event {
    touches_began(env, this, touches);
}
- (())touchesMoved:(id)touches withEvent:(id)_event {
    touches_moved(env, this, touches);
}
- (())touchesEnded:(id)touches withEvent:(id)_event {
    touches_ended(env, this, touches, false);
}
- (())touchesCancelled:(id)touches withEvent:(id)_event {
    touches_ended(env, this, touches, true);
}

- (())_touchHLE_tick:(id)_timer {
    let Some(controller) = controller_for_view(env, this) else {
        return;
    };
    let still_moving = tick(env, controller);
    () = msg![env; this setNeedsDisplay];
    if !still_moving {
        stop_timer(env, controller);
    }
}

@end

};

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values are from -[MusicLibraryCell fighterImageDraw] in the
    // game (song-summoner-re.md §3e), not from portrait_for_fid itself.
    #[test]
    fn never_summoned_songs_show_the_music_note() {
        assert_eq!(portrait_for_fid(-1), Some(89));
    }

    #[test]
    fn special_ids_use_their_own_images() {
        assert_eq!(portrait_for_fid(999), Some(87)); // "MUSIC FIGHTER"
        assert_eq!(portrait_for_fid(998), Some(88)); // "?"
    }

    #[test]
    fn minus_two_draws_nothing() {
        assert_eq!(portrait_for_fid(-2), None);
    }

    #[test]
    fn summoned_songs_show_their_trooper() {
        // IPD_ID2Fighter already took 10 off getGraphicNo's number, so the
        // id is the portrait file number as is.
        assert_eq!(portrait_for_fid(0), Some(0));
        assert_eq!(portrait_for_fid(42), Some(42));
        assert_eq!(portrait_for_fid(FIGHTER_COUNT as i32 - 1), Some(FIGHTER_COUNT - 1));
    }

    #[test]
    fn ids_without_a_portrait_file_draw_nothing() {
        assert_eq!(portrait_for_fid(FIGHTER_COUNT as i32), None);
        assert_eq!(portrait_for_fid(-3), None);
    }

    // Controller navigation. The list area is LIST_H = 220 points: four
    // 55-point rows, or fewer with 22-point section headers among them.

    fn song(id: u64) -> Row {
        Row::Song {
            id,
            title: format!("Song {id}"),
            subtitle: String::new(),
            artwork: false,
        }
    }

    fn group(name: &str) -> Row {
        Row::Group {
            name: name.to_string(),
            songs: vec![1, 2],
            artwork: None,
        }
    }

    /// Sections A, B, C, D of three songs each: rows are
    /// 0:A 1 2 3  4:B 5 6 7  8:C 9 10 11  12:D 13 14 15.
    fn sectioned() -> ListView {
        let mut rows = Vec::new();
        let mut id = 1;
        for letter in ['A', 'B', 'C', 'D'] {
            rows.push(Row::Header(letter));
            for _ in 0..3 {
                rows.push(song(id));
                id += 1;
            }
        }
        ListView::new(1, "Song".to_string(), None, rows)
    }

    /// Ten songs, no headers, drilled in (has a Back button).
    fn plain() -> ListView {
        let rows = (1..=10).map(song).collect();
        ListView::new(2, "Album".to_string(), Some("Album".to_string()), rows)
    }

    fn fully_visible(list: &ListView, row: usize) -> bool {
        let top = list.tops[row] as f32;
        let bottom = top + list.rows[row].height() as f32;
        top >= list.scroll && bottom <= list.scroll + LIST_H as f32
    }

    #[test]
    fn first_press_focuses_the_top_visible_row_without_moving() {
        let mut list = sectioned();
        assert_eq!(list.focus, None);
        list.move_focus(1);
        // Row 0 is the "A" header, which can't be focused.
        assert_eq!(list.focus, Some(1));
        assert_eq!(list.scroll, 0.0);
    }

    #[test]
    fn first_press_on_a_scrolled_list_stays_where_the_user_is() {
        let mut list = sectioned();
        // Scrolled so row 5 (first of B) is at the top.
        list.scroll = list.tops[5] as f32;
        list.move_focus(1);
        assert_eq!(list.focus, Some(5));
    }

    #[test]
    fn up_and_down_skip_headers_and_stop_at_the_ends() {
        let mut list = sectioned();
        list.move_focus(1); // focus row 1
        list.move_focus(1);
        list.move_focus(1);
        assert_eq!(list.focus, Some(3));
        list.move_focus(1); // over the "B" header
        assert_eq!(list.focus, Some(5));
        list.move_focus(-1);
        assert_eq!(list.focus, Some(3));
        list.move_focus(-10);
        assert_eq!(list.focus, Some(1));
        list.move_focus(100);
        assert_eq!(list.focus, Some(15));
        list.move_focus(1);
        assert_eq!(list.focus, Some(15));
    }

    #[test]
    fn focus_is_scrolled_into_view() {
        let mut list = sectioned();
        list.move_focus(1);
        for _ in 0..8 {
            list.move_focus(1);
            let row = list.focus.unwrap();
            assert!(fully_visible(&list, row), "row {row} at scroll {}", list.scroll);
        }
        assert!(list.scroll > 0.0);
        // The scroll never goes past the end of the list.
        list.move_focus(100);
        assert!(list.scroll <= list.max_scroll());
        assert!(fully_visible(&list, 15));
    }

    #[test]
    fn moving_up_into_a_section_shows_its_header() {
        let mut list = sectioned();
        list.scroll = list.max_scroll();
        list.focus = Some(13); // first song of D
        list.move_focus(-1); // row 11, last of C
        list.move_focus(-1);
        list.move_focus(-1); // row 9, first of C
        assert_eq!(list.focus, Some(9));
        // The "C" header (row 8) is on screen too.
        assert!(list.scroll <= list.tops[8] as f32);
    }

    #[test]
    fn right_jumps_to_the_next_section() {
        let mut list = sectioned();
        list.focus = Some(2);
        list.jump_section(true);
        assert_eq!(list.focus, Some(5)); // first of B
        // With its header at the top of the list.
        assert_eq!(list.scroll, list.tops[4] as f32);
        list.jump_section(true);
        assert_eq!(list.focus, Some(9));
        list.jump_section(true);
        assert_eq!(list.focus, Some(13));
        // No section after D: go to the last song.
        list.jump_section(true);
        assert_eq!(list.focus, Some(15));
        assert!(fully_visible(&list, 15));
    }

    #[test]
    fn left_goes_to_the_start_of_the_section_then_the_one_before() {
        let mut list = sectioned();
        list.focus = Some(11); // last of C
        list.jump_section(false);
        assert_eq!(list.focus, Some(9)); // first of C
        list.jump_section(false);
        assert_eq!(list.focus, Some(5)); // first of B
        list.jump_section(false);
        assert_eq!(list.focus, Some(1));
        list.jump_section(false);
        assert_eq!(list.focus, Some(1));
        assert_eq!(list.scroll, 0.0);
    }

    #[test]
    fn left_and_right_page_through_lists_without_sections() {
        let mut list = plain();
        list.move_focus(1); // focus row 0
        list.jump_section(true);
        assert_eq!(list.focus, Some(4)); // a page is 4 rows
        list.jump_section(true);
        list.jump_section(true);
        assert_eq!(list.focus, Some(9)); // stops at the end
        list.jump_section(false);
        assert_eq!(list.focus, Some(5));
        assert!(fully_visible(&list, 5));
    }

    #[test]
    fn empty_lists_have_nothing_to_focus() {
        let mut list = ListView::new(3, "Playlist".to_string(), None, Vec::new());
        list.move_focus(1);
        list.jump_section(true);
        list.jump_section(false);
        assert_eq!(list.focus, None);
        assert_eq!(pad_action(&mut list, Role::Confirm, false), Action::None);
    }

    #[test]
    fn confirm_picks_a_song_or_opens_a_group() {
        let mut list = plain();
        list.focus = Some(2);
        assert_eq!(pad_action(&mut list, Role::Confirm, false), Action::Pick(3));

        let rows = vec![Row::Header('A'), group("Abba"), group("Air")];
        let mut list = ListView::new(4, "Artist".to_string(), None, rows);
        list.focus = Some(2);
        assert_eq!(pad_action(&mut list, Role::Confirm, false), Action::Push(2));
    }

    #[test]
    fn confirm_without_a_focus_only_shows_one() {
        let mut list = plain();
        assert_eq!(pad_action(&mut list, Role::Confirm, false), Action::None);
        assert_eq!(list.focus, Some(0));
    }

    #[test]
    fn back_leaves_a_group_or_cancels_at_the_top() {
        let mut list = plain(); // drilled in
        assert_eq!(pad_action(&mut list, Role::Back, false), Action::Pop);
        let mut list = sectioned(); // a tab's top level
        assert_eq!(pad_action(&mut list, Role::Back, false), Action::Cancel);
    }

    #[test]
    fn first_press_after_touch_only_brings_the_focus_back() {
        // The user scrolled away by touch, so the old focus is off screen.
        for role in [Role::Down, Role::Up, Role::NextSection, Role::Confirm] {
            let mut list = sectioned();
            list.focus = Some(1);
            list.scroll = list.tops[9] as f32;
            assert_eq!(pad_action(&mut list, role, true), Action::None, "{role:?}");
            assert_eq!(list.focus, Some(9), "{role:?}");
        }
        // A focus that's still on screen is kept as is.
        let mut list = sectioned();
        list.scroll = list.tops[9] as f32;
        list.focus = Some(10);
        assert_eq!(pad_action(&mut list, Role::Down, true), Action::None);
        assert_eq!(list.focus, Some(10));
        // Back still works straight away.
        assert_eq!(pad_action(&mut list, Role::Back, true), Action::Cancel);
    }
}
