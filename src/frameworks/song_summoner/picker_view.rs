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
    /// Highlighted row of the current list.
    highlight: Option<usize>,
    /// Repeating `NSTimer`, retained, while something animates.
    timer: id,
    last_tick: Instant,
    transition: Option<Transition>,
    opened: Instant,
    row_cache: Vec<((u64, usize, bool), Rc<Bitmap>)>,
    nav_bar: Option<((String, Option<String>), Rc<Bitmap>)>,
    tab_bar: Option<(usize, Rc<Bitmap>)>,
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
                    Some(r) if lifted_on == Some(r) => match &list.rows[r] {
                        Row::Song { id, .. } => Action::Pick(*id),
                        Row::Group { .. } => Action::Push(r),
                        Row::Header(_) => Action::None,
                    },
                    _ => {
                        animate = true;
                        Action::None
                    }
                }
            }
        }
        _ => Action::None,
    };

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
            }
        }
    }
    if animate {
        start_timer(env, controller);
    }
    () = msg![env; view setNeedsDisplay];
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
    let highlighted = picker.highlight == Some(row_index) && picker.transition.is_none();
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

fn render_nav_bar(fonts: &Fonts, title: &str, back: Option<&str>) -> Bitmap {
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
    canvas.bitmap
}

fn render_tab_bar(ctx: &Ctx, selected: usize) -> Bitmap {
    let mut canvas = Canvas::new(W, TAB_H);
    canvas.vgradient(0, 0, W, TAB_H, TAB_TOP, (0, 0, 0));
    canvas.fill_rect(0, 0, W, 1, TAB_RULE, 1.0);
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
    let state = &mut env.framework_state.song_summoner;
    let picker = state.pickers.get_mut(&controller)?;
    let fighters = &state.fighters;

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

    let nav_key = (picker.list().title.clone(), picker.list().back.clone());
    let nav_bar = match &picker.nav_bar {
        Some((key, bitmap)) if *key == nav_key => bitmap.clone(),
        _ => {
            let bitmap = Rc::new(render_nav_bar(&ctx.fonts, &nav_key.0, nav_key.1.as_deref()));
            picker.nav_bar = Some((nav_key, bitmap.clone()));
            bitmap
        }
    };
    canvas.blit(&nav_bar, 0, 0, 1.0);

    let tab_bar = match &picker.tab_bar {
        Some((tab, bitmap)) if *tab == picker.tab => bitmap.clone(),
        _ => {
            let bitmap = Rc::new(render_tab_bar(&ctx, picker.tab));
            picker.tab_bar = Some((picker.tab, bitmap.clone()));
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
}
