/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Setup menu at run time: opening and closing it, pausing the game
//! behind it, feeding it input and carrying out what it asks for.
//!
//! The menu's logic is `setup_menu.rs` and its drawing `setup_view.rs`;
//! this is the glue to the engine. While the menu is open the game is
//! paused: the `-[MainView mainLoop]` wrapper (`game_input.rs`) asks
//! [before_frame] first and skips the game's frame, and we show its last
//! frame again with the menu drawn over it (`Window::set_overlays`). The
//! user's music is paused too.
//!
//! It opens by itself on the first frame of the first launch only, then
//! with F2 (desktop), Select + Start, or the Android gear button.
//!
//! Anything that needs a system picker or another app restarts the game:
//! on the desktop after asking (the picker opens first), on Android by
//! handing the job to `SetupActivity`, since touchHLE quits as soon as it
//! loses focus (see CLAUDE.md).

use super::credits;
use super::glyphs::Family;
use super::pad::{self, ChordOutcome, Role};
use super::picker_render::Fonts;
use super::settings::{self, GlyphStyle, Settings};
use super::setup_menu::{Change, Command, Info, Menu, Platform, PLATFORM};
use super::keys::key_label;
use super::setup_view::{self, Hit, Hud, Look, Pill};
use super::support;
use crate::gles::present::Overlay;
use crate::options::Options;
use crate::window::{Event, PadButton};
use crate::Environment;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The game file's name in the data folder, on both platforms.
pub const IPA_NAME: &str = "Song Summoner The Unsung Heroes Encore.ipa";

const TOAST_TIME: Duration = Duration::from_secs(6);
const FLASH_TIME: Duration = Duration::from_secs(3);

pub const REOPEN_TOAST: &str = "Setup only opens by itself the first time. To open it \
    again, press F2, or hold Select and press Start.";

#[derive(Default)]
pub struct State {
    settings: Option<Settings>,
    menu: Option<Menu>,
    info: Info,
    chord: pad::Chord,
    /// The menu changed and needs drawing again.
    dirty: bool,
    /// The make of the controller used last.
    family: Option<Family>,
    /// The keyboard was used more recently than a controller.
    keyboard_last: bool,
    toast_until: Option<Instant>,
    flash_at: Option<Instant>,
    /// Frames since `fps_since`, and the last full second's rate.
    frames: u32,
    fps_since: Option<Instant>,
    fps: Option<f32>,
    /// What the HUD overlay shows now (toast up, FPS), to redraw only on
    /// change.
    hud: Hud,
    first_frame_seen: bool,
    /// A touch went down on the menu: its moves and lift are ours too, even
    /// if the menu closed meanwhile.
    touch_is_ours: bool,
    /// Where that touch went down (menu points) and the scroll then, and
    /// whether it has moved far enough to be a drag.
    touch_start: Option<(f32, f32, i32)>,
    dragging: bool,
}

pub fn is_song_summoner(env: &Environment) -> bool {
    !env.bundle.is_null() && env.bundle.bundle_identifier() == support::BUNDLE_ID
}

/// The data folder: saves, log, options, library.
fn base() -> PathBuf {
    crate::paths::user_data_base_path().to_path_buf()
}

/// The `--deadzone=` the options gave, before the settings file's.
static BASE_DEADZONE: OnceLock<f32> = OnceLock::new();

/// At startup, before the window exists: let the settings file adjust the
/// options (the stick's dead zone) and restore saves if a restore was
/// asked for. Song Summoner only.
pub fn apply_at_startup(options: &mut Options) {
    let _ = BASE_DEADZONE.set(options.deadzone);
    let s = Settings::load(&settings::path(), options.confirm_button);
    if let Some(d) = s.deadzone {
        options.deadzone = d;
    }
    crate::gles::present::set_cursor_look(s.cursor_style, s.cursor_colour);
    super::mirror::note_start(&base());
    support::restore_pending(&base());
}

fn settings(env: &mut Environment) -> &mut Settings {
    let confirm = env.options.confirm_button;
    env.framework_state
        .song_summoner
        .setup
        .settings
        .get_or_insert_with(|| Settings::load(&settings::path(), confirm))
}

/// The shop's password is typed on the device's keyboard too (Setup >
/// Game > Shop password).
pub fn device_keyboard(env: &mut Environment) -> bool {
    settings(env).device_keyboard
}

/// What a controller button does in the game and the picker, with the
/// player's mapping.
pub fn role_for(env: &mut Environment, button: PadButton) -> Option<Role> {
    settings(env).pad.role(button)
}

/// The button that does `role`, for drawing its icon.
pub fn button_for(env: &mut Environment, role: Role) -> PadButton {
    settings(env).pad.button(role)
}

/// Seconds before a held direction repeats, and between repeats.
pub fn repeat_timing(env: &mut Environment) -> (f32, f32) {
    settings(env).scroll_speed.timing()
}

/// Whose button icons to draw, for a pad of this `SDL_GameControllerType`.
pub fn family_for(env: &mut Environment, controller_type: u32) -> Family {
    match settings(env).glyph_style {
        GlyphStyle::Auto => Family::from_sdl_type(controller_type),
        GlyphStyle::Xbox => Family::Xbox,
        GlyphStyle::PlayStation => Family::PlayStation,
        GlyphStyle::Nintendo => Family::Nintendo,
    }
}

pub fn is_open(env: &Environment) -> bool {
    env.framework_state.song_summoner.setup.menu.is_some()
}

fn open(env: &mut Environment) {
    if !is_song_summoner(env) || is_open(env) {
        return;
    }
    log!("setup: menu opened, game paused");
    super::game_input::hide_soft_keyboard(env);
    super::game_input::release_held(env);
    super::picker_view::release_held(env);
    settings(env);
    refresh_info(env);
    let confirm = env.options.confirm_button;
    // The first menu starts where setup is still missing.
    let first_run = !settings(env).first_run_done;
    let no_music = crate::media::source::roots().is_empty();
    let state = &mut env.framework_state.song_summoner.setup;
    let mut menu = Menu::new(PLATFORM, confirm);
    menu.start_on_music(first_run && no_music);
    state.menu = Some(menu);
    state.dirty = true;
    crate::frameworks::media_player::music_player::host_pause(env, true);
}

fn close(env: &mut Environment) {
    let state = &mut env.framework_state.song_summoner.setup;
    if state.menu.take().is_none() {
        return;
    }
    log!("setup: menu closed, game resumed");
    state.hud = Hud::default();
    if let Some(window) = env.window.as_mut() {
        window.set_overlays(Vec::new());
    }
    crate::frameworks::media_player::music_player::host_pause(env, false);
}

/// The game is quitting: copy the last changes to the save folder.
pub fn before_exit(env: &mut Environment) {
    if is_song_summoner(env) {
        super::mirror::tick(env, true);
    }
}

/// F2 or the Android gear: open the menu, or (if open) back out of a wait
/// or close it.
pub fn open_key(env: &mut Environment) {
    if !is_song_summoner(env) {
        return;
    }
    if is_open(env) {
        run(env, |menu, s, _| menu.menu_key(s));
    } else {
        open(env);
    }
}

/// Run a menu method with the settings and info, then carry out what it
/// asked for.
fn run(env: &mut Environment, f: impl FnOnce(&mut Menu, &mut Settings, &Info) -> Vec<Command>) {
    let confirm = env.options.confirm_button;
    let state = &mut env.framework_state.song_summoner.setup;
    let Some(menu) = state.menu.as_mut() else {
        return;
    };
    let settings = state
        .settings
        .get_or_insert_with(|| Settings::load(&settings::path(), confirm));
    let flash_before = menu.flash.clone();
    let commands = f(menu, settings, &state.info);
    if menu.flash != flash_before && menu.flash.is_some() {
        state.flash_at = Some(Instant::now());
    }
    state.dirty = true;
    for command in commands {
        apply(env, command);
    }
}

/// A controller button. Returns true if the menu (or its shortcut) took
/// it; otherwise it's for the picker or the game.
pub fn handle_pad_button(env: &mut Environment, button: PadButton, pressed: bool, controller_type: u32) -> bool {
    if !is_song_summoner(env) {
        return false;
    }
    let family = family_for(env, controller_type);
    let state = &mut env.framework_state.song_summoner.setup;
    state.family = Some(family);
    state.keyboard_last = false;
    match state.chord.feed(button, pressed) {
        ChordOutcome::OpenMenu => {
            open_key(env);
            true
        }
        ChordOutcome::Swallow => {
            if pressed && button == PadButton::Back {
                run(env, |menu, _, _| {
                    menu.select_pressed();
                    Vec::new()
                });
            }
            // Select, and the Start of the shortcut, never reach the game.
            true
        }
        ChordOutcome::Pass => {
            if !is_open(env) {
                return false;
            }
            run(env, |menu, s, info| menu.pad_button(button, pressed, s, info));
            true
        }
    }
}

/// A key (desktop). Returns the command it maps to for the picker or the
/// game, if the menu didn't take it. Keys do nothing while the game's text
/// field has the keyboard.
pub fn handle_key(env: &mut Environment, key: &str, pressed: bool) -> Option<Role> {
    if !is_song_summoner(env) {
        return None;
    }
    if env
        .window
        .as_ref()
        .is_some_and(|w| w.is_text_input_active())
    {
        return None;
    }
    env.framework_state.song_summoner.setup.keyboard_last = true;
    if is_open(env) {
        if pressed {
            run(env, |menu, s, info| menu.key(key, s, info));
        }
        return None;
    }
    settings(env).keys.role(key)
}

/// A touch. Returns true if the menu took it.
pub fn handle_touch(env: &mut Environment, event: &Event) -> bool {
    let (map, down, up) = match event {
        Event::TouchesDown(map) => (map, true, false),
        Event::TouchesMove(map) => (map, false, false),
        Event::TouchesUp(map) => (map, false, true),
        _ => return false,
    };
    let open = is_open(env);
    let state = &mut env.framework_state.song_summoner.setup;
    if !open {
        // The lift of a touch that closed the menu.
        let ours = state.touch_is_ours;
        if up {
            state.touch_is_ours = false;
        }
        return ours;
    }
    // Where the finger is, in the menu's points.
    let at = map.values().next().and_then(|&point| {
        let window = env.window.as_ref()?;
        let (fx, fy) = window.screen_point_to_shown_fraction(point);
        Some((fx * setup_view::W as f32, fy * setup_view::H as f32))
    });
    let state = &mut env.framework_state.song_summoner.setup;
    if down {
        state.touch_is_ours = true;
        // A tap acts when the finger lifts; a finger that moves scrolls
        // the list instead (2026-09-28: touch couldn't scroll at all).
        let scroll = state.menu.as_ref().map_or(0, |m| m.scroll);
        state.touch_start = at.map(|(x, y)| (x, y, scroll));
        state.dragging = false;
        return true;
    }
    let Some((x0, y0, from)) = state.touch_start else {
        if up {
            state.touch_is_ours = false;
        }
        return true;
    };
    if let Some((_, y)) = at {
        let dy = y - y0;
        if !state.dragging && dy.abs() >= DRAG_START {
            state.dragging = true;
        }
        let scrollable = state
            .menu
            .as_ref()
            .is_some_and(|m| matches!(m.mode, super::setup_menu::Mode::Normal));
        if state.dragging && scrollable {
            if let Some(menu) = state.menu.as_mut() {
                menu.drag(from, dy.round() as i32);
                state.dirty = true;
            }
        }
    }
    if up {
        state.touch_is_ours = false;
        state.touch_start = None;
        if !std::mem::take(&mut state.dragging) {
            tap(env, x0, y0);
        }
    }
    true
}

/// How far a finger moves, in menu points, before it scrolls rather than
/// taps.
const DRAG_START: f32 = 8.0;

fn tap(env: &mut Environment, x: f32, y: f32) {
    let fonts = fonts(env);
    let hit = {
        let state = &mut env.framework_state.song_summoner.setup;
        let (Some(menu), Some(settings)) = (state.menu.as_ref(), state.settings.as_ref()) else {
            return;
        };
        if matches!(menu.mode, super::setup_menu::Mode::Tester { .. }) {
            None
        } else {
            Some(setup_view::hit(&fonts, menu, settings, &state.info, x, y))
        }
    };
    run(env, |menu, s, info| match hit {
        // A tap leaves the tester.
        None => {
            menu.select_pressed();
            Vec::new()
        }
        Some(Hit::Tab(i)) => menu.tap_tab(i, s, info),
        Some(Hit::Row(i, slot)) => menu.tap_row(i, slot, s, info),
        Some(Hit::DialogYes) => menu.tap_dialog(true, s, info),
        Some(Hit::DialogNo) => menu.tap_dialog(false, s, info),
        Some(Hit::Panel) => {
            menu.tap_panel();
            Vec::new()
        }
        Some(Hit::Outside) => menu.tap_outside(s, info),
    });
}

fn fonts(env: &mut Environment) -> Rc<Fonts> {
    env.framework_state
        .song_summoner
        .fonts
        .get_or_insert_with(|| Rc::new(Fonts::load()))
        .clone()
}

/// Before each of the game's frames. Returns true if the game is paused
/// (the menu is open), so its frame must be skipped.
pub fn before_frame(env: &mut Environment) -> bool {
    if !is_song_summoner(env) {
        return false;
    }
    let now = Instant::now();
    let state = &mut env.framework_state.song_summoner.setup;
    if !state.first_frame_seen {
        state.first_frame_seen = true;
        if !settings(env).first_run_done {
            log!("setup: first launch, opening the menu");
            open(env);
        }
    }
    let state = &mut env.framework_state.song_summoner.setup;
    if state
        .flash_at
        .is_some_and(|at| now.duration_since(at) >= FLASH_TIME)
    {
        state.flash_at = None;
        if let Some(menu) = state.menu.as_mut() {
            menu.flash = None;
            state.dirty = true;
        }
    }
    if state.toast_until.is_some_and(|until| now >= until) {
        state.toast_until = None;
    }
    count_frame(env, now);
    // Android: copy changed saves to the save folder.
    super::mirror::tick(env, false);

    if is_open(env) {
        if env.framework_state.song_summoner.setup.dirty {
            draw_menu(env);
        }
        crate::frameworks::opengles::present_again(env);
        return true;
    }
    update_hud(env);
    false
}

fn count_frame(env: &mut Environment, now: Instant) {
    let show = settings(env).show_fps;
    let state = &mut env.framework_state.song_summoner.setup;
    if !show {
        state.fps = None;
        state.fps_since = None;
        return;
    }
    state.frames += 1;
    let since = *state.fps_since.get_or_insert(now);
    let elapsed = now.duration_since(since).as_secs_f32();
    if elapsed >= 1.0 {
        state.fps = Some(state.frames as f32 / elapsed);
        state.frames = 0;
        state.fps_since = Some(now);
    }
}

fn draw_menu(env: &mut Environment) {
    let fonts = fonts(env);
    let ss = &mut env.framework_state.song_summoner;
    let state = &mut ss.setup;
    state.dirty = false;
    let (Some(menu), Some(settings)) = (state.menu.as_mut(), state.settings.as_ref()) else {
        return;
    };
    let look = Look {
        family: state.family.unwrap_or(match settings.glyph_style {
            GlyphStyle::PlayStation => Family::PlayStation,
            GlyphStyle::Nintendo => Family::Nintendo,
            _ => Family::Xbox,
        }),
        keyboard: state.keyboard_last,
    };
    let bitmap = setup_view::render(&fonts, &mut ss.glyphs, menu, settings, &state.info, &look);
    let overlay = Overlay {
        width: bitmap.width,
        height: bitmap.height,
        pixels: Rc::new(bitmap.pixels),
        rect: (0.0, 0.0, 1.0, 1.0),
    };
    if let Some(window) = env.window.as_mut() {
        window.set_overlays(vec![overlay]);
    }
}

/// The toast, FPS counter and Password pill over the running game.
fn update_hud(env: &mut Environment) {
    let pill = if super::game_input::password_pill(env) {
        let info = Role::Info;
        let state = &env.framework_state.song_summoner.setup;
        let keyboard = state.keyboard_last;
        let family = state.family;
        let s = settings(env);
        Some(if keyboard {
            Pill::Key(s.keys.keys(info)[0].map(key_label).unwrap_or_default())
        } else {
            let family = family.unwrap_or(match s.glyph_style {
                GlyphStyle::PlayStation => Family::PlayStation,
                GlyphStyle::Nintendo => Family::Nintendo,
                _ => Family::Xbox,
            });
            Pill::Button(family, pad::glyph_of(s.pad.button(info)))
        })
    } else {
        None
    };
    let state = &env.framework_state.song_summoner.setup;
    let wanted = Hud {
        toast: state.toast_until.is_some(),
        fps: state.fps.map(|f| f.round() as u32),
        pill,
    };
    if wanted == state.hud {
        return;
    }
    let fonts = fonts(env);
    let ss = &mut env.framework_state.song_summoner;
    ss.setup.hud = wanted.clone();
    let overlays = match setup_view::render_hud(&fonts, &mut ss.glyphs, &wanted) {
        Some(bitmap) => vec![Overlay {
            width: bitmap.width,
            height: bitmap.height,
            pixels: Rc::new(bitmap.pixels),
            rect: (0.0, 0.0, 1.0, 1.0),
        }],
        None => Vec::new(),
    };
    if let Some(window) = env.window.as_mut() {
        window.set_overlays(overlays);
    }
}

/// Facts the rows show, gathered when the menu opens.
fn refresh_info(env: &mut Environment) {
    let base = base();
    let ipa = base.join(IPA_NAME);
    let game_file = std::fs::metadata(&ipa).ok().map(|meta| {
        (
            IPA_NAME.to_string(),
            base.display().to_string(),
            meta.len() >> 20,
        )
    });
    let library = crate::media::library::current();
    let roots = crate::media::source::roots();
    let folders = roots
        .iter()
        .enumerate()
        .map(|(i, root)| {
            let count = library
                .songs
                .iter()
                .filter(|s| song_in_root(&s.locator, i, root))
                .count();
            (folder_name(root), Some(count))
        })
        .collect();
    let fullscreen = env.window.as_ref().is_some_and(|w| w.is_fullscreen());
    let save_folder = std::fs::read_to_string(base.join(super::mirror::POINTER_FILE))
        .ok()
        .map(|uri| folder_name(uri.trim()))
        .filter(|name| !name.is_empty());
    env.framework_state.song_summoner.setup.info = Info {
        game_file,
        folders,
        data_dir: match &save_folder {
            // Android: the folder the player knows is the save folder.
            Some(name) if PLATFORM == Platform::Android => name.clone(),
            _ => base.display().to_string(),
        },
        save_folder,
        version: crate::VERSION.to_string(),
        fullscreen,
        latest_backup: support::latest_backup(&base),
    };
}

/// Whether a song's locator belongs to music folder `index` at `root`.
fn song_in_root(locator: &str, index: usize, root: &str) -> bool {
    if locator.starts_with("content://") {
        // Android: document URIs built from the folder's tree URI.
        return locator.starts_with(&format!("{root}/document/"));
    }
    if Path::new(locator).is_absolute() {
        Path::new(locator).starts_with(root)
    } else {
        index == 0
    }
}

/// A music folder as the player knows it: the path on the desktop, a
/// readable "Internal storage › Music" for an Android tree URI.
pub fn folder_name(root: &str) -> String {
    let Some(tree) = root.split("/tree/").nth(1) else {
        return root.to_string();
    };
    let decoded = percent_decode(tree.split('/').next().unwrap_or(tree));
    match decoded.split_once(':') {
        Some(("primary", "")) => "Internal storage".to_string(),
        Some(("primary", path)) => format!("Internal storage › {}", path.replace('/', " › ")),
        Some((_, "")) => "SD card".to_string(),
        Some((_, path)) => format!("SD card › {}", path.replace('/', " › ")),
        None => decoded,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn flash(env: &mut Environment, text: impl Into<String>) {
    let state = &mut env.framework_state.song_summoner.setup;
    if let Some(menu) = state.menu.as_mut() {
        menu.flash = Some(text.into());
        state.flash_at = Some(Instant::now());
        state.dirty = true;
    }
}

fn save_settings(env: &mut Environment) {
    let s = settings(env).clone();
    if let Err(e) = s.save(&settings::path()) {
        log!("setup: couldn't save {}: {}", settings::path().display(), e);
        flash(env, "Couldn't save the settings");
    }
    // Settings that act at once.
    let base = *BASE_DEADZONE.get().unwrap_or(&Options::default().deadzone);
    env.options.deadzone = s.deadzone.unwrap_or(base);
    crate::gles::present::set_cursor_look(s.cursor_style, s.cursor_colour);
}

fn apply(env: &mut Environment, command: Command) {
    log_dbg!("setup: {:?}", command);
    match command {
        Command::Close => close(env),
        Command::SaveSettings => save_settings(env),
        Command::ShowReopenToast => {
            env.framework_state.song_summoner.setup.toast_until = Some(Instant::now() + TOAST_TIME);
        }
        Command::PickGameFile => pick_game_file(env),
        Command::PickMusicFolder => pick_music_folder(env),
        Command::Apply(change) => apply_change(env, change),
        Command::OpenDataFolder => {
            if let Err(e) = platform::open_path(&base()) {
                flash(env, format!("Couldn't open the folder: {e}"));
            }
        }
        Command::OpenTipPage => {
            if let Err(e) = platform::open_url(credits::TIP_LINK) {
                flash(env, format!("Couldn't open the page: {e}"));
            }
        }
        Command::ToggleFullscreen => {
            if let Some(window) = env.window.as_mut() {
                window.toggle_fullscreen();
            }
            refresh_info(env);
        }
        Command::BugReport => {
            let about = about_text(env);
            match support::write_bug_report(&base(), &about) {
                Ok(path) => {
                    log!("setup: wrote {}", path.display());
                    flash(env, format!("Saved to {}", shown_path(&path)));
                    if PLATFORM == Platform::Desktop {
                        let _ = platform::open_path(&base().join(support::REPORTS_DIR));
                    }
                }
                Err(e) => flash(env, format!("Couldn't write the report: {e}")),
            }
        }
        Command::BackupSaves => match support::backup_saves(&base()) {
            Ok(path) => {
                flash(env, format!("Saved to {}", shown_path(&path)));
                refresh_info(env);
            }
            Err(e) => flash(env, e),
        },
        Command::WriteLicenses => match support::write_licenses(&base()) {
            Ok(path) => {
                flash(env, format!("Saved to {}", shown_path(&path)));
                if PLATFORM == Platform::Desktop {
                    let _ = platform::open_path(&path);
                }
            }
            Err(e) => flash(env, format!("Couldn't write them: {e}")),
        },
    }
}

/// A path relative to the data folder, for messages.
fn shown_path(path: &Path) -> String {
    path.strip_prefix(base())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn about_text(env: &mut Environment) -> String {
    let info = env.framework_state.song_summoner.setup.info.clone();
    let settings_path = settings::path();
    format!(
        "Song Summoner Revival {}\nPlatform: {} ({})\nGame file: {}\nMusic folders: {}\n\
         Songs: {}\nSettings: {}\n",
        crate::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        info.game_file
            .map_or("missing".to_string(), |(n, _, mb)| format!("{n}, {mb} MB")),
        info.folders.len(),
        crate::media::library::current().len(),
        settings_path.display(),
    )
}

/// Whether a file looks like the game: a zip with an app bundle in
/// `Payload/`.
pub fn check_game_file(path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|_| "That file isn't an .ipa (not a zip archive)".to_string())?;
    let has_app = (0..archive.len()).any(|i| {
        archive.by_index(i).is_ok_and(|entry| {
            let name = entry.name();
            name.starts_with("Payload/") && name.contains(".app/")
        })
    });
    if has_app {
        Ok(())
    } else {
        Err("That .ipa has no app inside".to_string())
    }
}

fn pick_game_file(env: &mut Environment) {
    let Some(path) = platform::pick_file(env) else {
        return;
    };
    match check_game_file(&path) {
        Ok(()) => {
            let shown = path.display().to_string();
            run(env, |menu, _, _| {
                menu.picked(Change::GameFile(Some(shown)));
                Vec::new()
            });
        }
        Err(e) => flash(env, e),
    }
}

fn pick_music_folder(env: &mut Environment) {
    let Some(path) = platform::pick_folder(env) else {
        return;
    };
    let shown = path.display().to_string();
    if crate::media::source::roots().contains(&shown) {
        flash(env, "That folder is already in your library");
        return;
    }
    run(env, |menu, _, _| {
        menu.picked(Change::AddFolder(Some(shown)));
        Vec::new()
    });
}

/// The player said yes: save the change and restart.
fn apply_change(env: &mut Environment, change: Change) {
    save_settings(env);
    let base = base();
    let result: Result<Option<&str>, String> = match &change {
        Change::GameFile(Some(path)) => {
            replace_game_file(Path::new(path), &base.join(IPA_NAME)).map(|()| None)
        }
        Change::GameFile(None) => Ok(Some("change_ipa")),
        Change::AddFolder(Some(path)) => {
            let mut roots = crate::media::source::roots();
            roots.push(path.clone());
            crate::media::source::write_roots(&roots)
                .map(|()| None)
                .map_err(|e| e.to_string())
        }
        Change::AddFolder(None) => Ok(Some("add_music")),
        Change::RemoveFolder(i) => {
            let mut roots = crate::media::source::roots();
            if *i < roots.len() {
                roots.remove(*i);
            }
            crate::media::source::write_roots(&roots)
                .map(|()| Some("rescan"))
                .map_err(|e| e.to_string())
        }
        Change::Rescan => Ok(Some("rescan")),
        Change::OpenDataFolder => Ok(Some("open_data_folder")),
        Change::RestoreSaves(name) => support::request_restore(&base, name).map(|()| Some("restart")),
        Change::SaveFolder => Ok(Some("pick_save_folder")),
        Change::TipPage => Ok(Some("open_tip_page")),
    };
    let action = match result {
        Ok(action) => action,
        Err(e) => {
            flash(env, format!("Nothing changed: {e}"));
            return;
        }
    };
    log!("setup: restarting for {:?}", change);
    let drop_bundle_arg = matches!(change, Change::GameFile(_));
    if let Err(e) = platform::restart(env, action.unwrap_or("restart"), drop_bundle_arg) {
        flash(env, format!("Couldn't restart: {e}"));
        return;
    }
    crate::frameworks::uikit::exit_app(env);
}

/// Copy a new game file into place through a temporary one, so a failed
/// copy never leaves half a file where the game is looked for.
fn replace_game_file(from: &Path, to: &Path) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    let part = to.with_extension("ipa.part");
    std::fs::copy(from, &part).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&part, to) {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    Ok(())
}

/// The command line to start again with: the same arguments, less the
/// game's path when the game file changed (so the new one in the data
/// folder is used).
#[cfg_attr(target_os = "android", allow(dead_code))]
pub fn restart_args(args: &[String], drop_bundle_arg: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut dropped = !drop_bundle_arg;
    let mut app_args = false;
    for arg in args {
        if arg == "--" {
            app_args = true;
        }
        if !dropped && !app_args && !arg.starts_with("--") {
            dropped = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

#[cfg(not(target_os = "android"))]
mod platform {
    use super::*;

    pub fn open_path(path: &Path) -> Result<(), String> {
        let program = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(program)
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Open a web page in the default browser (only ever our own links).
    pub fn open_url(url: &str) -> Result<(), String> {
        let mut command = if cfg!(windows) {
            // `start`'s first quoted argument is a window title.
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else {
            std::process::Command::new("xdg-open")
        };
        command.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn pick_file(env: &mut Environment) -> Option<PathBuf> {
        env.on_parent_stack_in_coroutine(|_, _| {
            rfd::FileDialog::new()
                .set_title("Choose your Song Summoner .ipa")
                .add_filter("iPhone app", &["ipa"])
                .pick_file()
        })
    }

    pub fn pick_folder(env: &mut Environment) -> Option<PathBuf> {
        let first = crate::media::source::roots().first().map(PathBuf::from);
        env.on_parent_stack_in_coroutine(move |_, _| {
            crate::media::scan_windows::ask_for_folder(first.as_deref())
        })
    }

    /// Start touchHLE again with the same arguments. The caller then quits
    /// this one.
    pub fn restart(_env: &mut Environment, _action: &str, drop_bundle_arg: bool) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        std::process::Command::new(exe)
            .args(restart_args(&args, drop_bundle_arg))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "android")]
mod platform {
    use super::*;

    pub fn open_path(_path: &Path) -> Result<(), String> {
        Err("not while the game runs".to_string())
    }

    pub fn open_url(_url: &str) -> Result<(), String> {
        Err("not while the game runs".to_string())
    }

    pub fn pick_file(_env: &mut Environment) -> Option<PathBuf> {
        None
    }

    pub fn pick_folder(_env: &mut Environment) -> Option<PathBuf> {
        None
    }

    /// Hand the job to SetupActivity (`MainActivity.requestSetup`), which
    /// runs it and then starts the game again. The caller then quits.
    pub fn restart(env: &mut Environment, action: &str, _drop: bool) -> Result<(), String> {
        let action = action.to_string();
        env.on_parent_stack_in_coroutine(move |_, _| jni_request_setup(&action))
    }

    fn jni_request_setup(action: &str) -> Result<(), String> {
        use jni::objects::JValue;
        use jni::JNIEnv;
        extern "C" {
            fn SDL_AndroidGetJNIEnv() -> *mut std::ffi::c_void;
        }
        let raw = unsafe { SDL_AndroidGetJNIEnv() };
        if raw.is_null() {
            return Err("no JNIEnv for this thread".to_string());
        }
        let mut env =
            unsafe { JNIEnv::from_raw(raw as *mut jni::sys::JNIEnv) }.map_err(|e| e.to_string())?;
        let result = (|| {
            let jaction = env.new_string(action)?;
            env.call_static_method(
                "org/touchhle/android/MainActivity",
                "requestSetup",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&jaction)],
            )?;
            env.delete_local_ref(jaction)?;
            Ok::<(), jni::errors::Error>(())
        })();
        if result.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        result.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn restart_keeps_the_arguments() {
        let args = strings(&["game.ipa", "--fullscreen"]);
        assert_eq!(restart_args(&args, false), args);
    }

    #[test]
    fn restart_after_a_new_game_file_drops_the_old_path() {
        let args = strings(&["--fullscreen", "old.ipa", "--", "app-arg"]);
        assert_eq!(
            restart_args(&args, true),
            strings(&["--fullscreen", "--", "app-arg"])
        );
        // Nothing positional before "--": nothing dropped.
        let args = strings(&["--", "x"]);
        assert_eq!(restart_args(&args, true), args);
    }

    #[test]
    fn android_folder_names_are_readable() {
        assert_eq!(
            folder_name("content://com.android.externalstorage.documents/tree/primary%3AMusic"),
            "Internal storage › Music"
        );
        assert_eq!(
            folder_name("content://com.android.externalstorage.documents/tree/primary%3AMusic%2FOSTs"),
            "Internal storage › Music › OSTs"
        );
        assert_eq!(
            folder_name("content://com.android.externalstorage.documents/tree/1234-5678%3A"),
            "SD card"
        );
        assert_eq!(folder_name("D:\\Music"), "D:\\Music");
    }

    #[test]
    fn songs_belong_to_their_folder() {
        let tree = "content://x/tree/primary%3AMusic";
        assert!(song_in_root(&format!("{tree}/document/primary%3AMusic%2Fa.mp3"), 1, tree));
        assert!(!song_in_root("content://x/tree/other/document/a", 0, tree));
        // Desktop: relative locators are the first folder's.
        assert!(song_in_root("Rock/a.mp3", 0, "D:\\Music"));
        assert!(!song_in_root("Rock/a.mp3", 1, "E:\\OSTs"));
        let root = std::env::temp_dir().join("osts");
        let song = root.join("a.mp3").to_string_lossy().into_owned();
        assert!(song_in_root(&song, 1, &root.to_string_lossy()));
    }

    #[test]
    fn a_file_that_isnt_a_zip_isnt_the_game() {
        let path = std::env::temp_dir().join(format!("touchHLE-not-ipa-{}.ipa", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let result = check_game_file(&path);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn a_zip_with_an_app_is_the_game() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("touchHLE-ipa-{}.ipa", std::process::id()));
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            zip.start_file("Payload/Game.app/Info.plist", zip::write::FileOptions::default())
                .unwrap();
            zip.write_all(b"plist").unwrap();
            zip.finish().unwrap();
        }
        let result = check_game_file(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(result, Ok(()));
    }
}
