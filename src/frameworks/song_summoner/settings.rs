/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Setup menu's settings: `<user data>/song_summoner_settings.txt`.
//!
//! Plain `key=value` lines, so a player can open it in Notepad and send it
//! with a bug report. `#` starts a comment line. Keys this build doesn't
//! know are kept when the file is rewritten, so going back to an older
//! build and forward again loses nothing. A bad value only resets its own
//! key. Host-only: no guest memory, no Objective-C.
//!
//! `SetupPill.kt` (Android) reads `gear_auto_dim` from the same file.

use super::keys::KeyBindings;
use super::pad::{self, Bindings};
pub use crate::gles::present::{CursorColour, CursorStyle};
use crate::options::ConfirmButton;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "song_summoner_settings.txt";

pub fn path() -> PathBuf {
    crate::paths::user_data_base_path().join(FILE_NAME)
}

/// Whose button icons to draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GlyphStyle {
    /// Whatever the connected pad reports itself as.
    Auto,
    Xbox,
    PlayStation,
    Nintendo,
}

impl GlyphStyle {
    pub const ALL: [GlyphStyle; 4] = [
        GlyphStyle::Auto,
        GlyphStyle::Xbox,
        GlyphStyle::PlayStation,
        GlyphStyle::Nintendo,
    ];
    pub fn name(self) -> &'static str {
        match self {
            GlyphStyle::Auto => "auto",
            GlyphStyle::Xbox => "xbox",
            GlyphStyle::PlayStation => "playstation",
            GlyphStyle::Nintendo => "nintendo",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GlyphStyle::Auto => "Auto",
            GlyphStyle::Xbox => "Xbox",
            GlyphStyle::PlayStation => "PlayStation",
            GlyphStyle::Nintendo => "Nintendo",
        }
    }
    fn from_name(name: &str) -> Option<GlyphStyle> {
        GlyphStyle::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// How fast a held direction repeats in lists.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollSpeed {
    Slow,
    Normal,
    Fast,
}

impl ScrollSpeed {
    pub const ALL: [ScrollSpeed; 3] = [ScrollSpeed::Slow, ScrollSpeed::Normal, ScrollSpeed::Fast];
    pub fn name(self) -> &'static str {
        match self {
            ScrollSpeed::Slow => "slow",
            ScrollSpeed::Normal => "normal",
            ScrollSpeed::Fast => "fast",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ScrollSpeed::Slow => "Slow",
            ScrollSpeed::Normal => "Normal",
            ScrollSpeed::Fast => "Fast",
        }
    }
    fn from_name(name: &str) -> Option<ScrollSpeed> {
        ScrollSpeed::ALL.into_iter().find(|s| s.name() == name)
    }
    /// (delay before repeating, time between repeats), in seconds.
    pub fn timing(self) -> (f32, f32) {
        match self {
            ScrollSpeed::Slow => (0.5, 0.12),
            ScrollSpeed::Normal => (pad::REPEAT_DELAY, pad::REPEAT_INTERVAL),
            ScrollSpeed::Fast => (0.25, 0.04),
        }
    }
}

/// Dead zone steps the menu offers.
pub const DEADZONES: [f32; 6] = [0.05, 0.1, 0.15, 0.2, 0.3, 0.4];

/// The cursor's look (drawn by `gles::present`), by name in the file and
/// label in the menu.
pub fn cursor_style_name(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Game => "game",
        CursorStyle::Outline => "outline",
        CursorStyle::Bold => "bold",
    }
}

pub fn cursor_style_label(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Game => "Like the game",
        CursorStyle::Outline => "Outlined",
        CursorStyle::Bold => "Bold",
    }
}

pub fn cursor_style_from_name(name: &str) -> Option<CursorStyle> {
    CursorStyle::ALL.into_iter().find(|&s| cursor_style_name(s) == name)
}

pub fn cursor_colour_name(colour: CursorColour) -> &'static str {
    match colour {
        CursorColour::Gold => "gold",
        CursorColour::White => "white",
        CursorColour::Yellow => "yellow",
        CursorColour::Sky => "sky",
    }
}

pub fn cursor_colour_label(colour: CursorColour) -> &'static str {
    match colour {
        CursorColour::Gold => "Gold",
        CursorColour::White => "White",
        CursorColour::Yellow => "Yellow",
        CursorColour::Sky => "Sky blue",
    }
}

pub fn cursor_colour_from_name(name: &str) -> Option<CursorColour> {
    CursorColour::ALL.into_iter().find(|&c| cursor_colour_name(c) == name)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The menu has opened by itself once. It never does again.
    pub first_run_done: bool,
    pub pad: Bindings,
    pub keys: KeyBindings,
    pub glyph_style: GlyphStyle,
    /// The left stick's dead zone, or `None` for `--deadzone=`'s.
    pub deadzone: Option<f32>,
    pub show_fps: bool,
    /// Android: the gear fades after a few seconds without a touch.
    pub gear_auto_dim: bool,
    pub scroll_speed: ScrollSpeed,
    /// The shop's password is typed on the device's own keyboard (a real
    /// one on desktop, the on-screen one on Android) as well as with the
    /// controller on the game's keyboard.
    pub device_keyboard: bool,
    /// How the controller's cursor looks: for players who find the game's
    /// faint highlight hard to see, or can't tell its colours apart.
    pub cursor_style: CursorStyle,
    pub cursor_colour: CursorColour,
    /// Lines this build doesn't know, kept in order.
    other: Vec<(String, String)>,
}

impl Settings {
    pub fn defaults(confirm: ConfirmButton) -> Settings {
        Settings {
            first_run_done: false,
            pad: Bindings::defaults(confirm),
            keys: KeyBindings::default(),
            glyph_style: GlyphStyle::Auto,
            deadzone: None,
            show_fps: false,
            gear_auto_dim: true,
            scroll_speed: ScrollSpeed::Normal,
            // Desktops have a keyboard to hand; on Android the on-screen
            // one would cover the game's, so it's the player's choice.
            device_keyboard: !cfg!(target_os = "android"),
            cursor_style: CursorStyle::Game,
            cursor_colour: CursorColour::Gold,
            other: Vec::new(),
        }
    }

    /// Parse the file's text. Returns the settings and a warning for each
    /// value that couldn't be used.
    pub fn parse(text: &str, confirm: ConfirmButton) -> (Settings, Vec<String>) {
        let mut lines: Vec<(String, String)> = Vec::new();
        let mut warnings = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {}: no '=' in {line:?}", n + 1));
                continue;
            };
            let (key, value) = (key.trim().to_string(), value.trim().to_string());
            // The last of a repeated key wins.
            lines.retain(|(k, _)| *k != key);
            lines.push((key, value));
        }
        let get = |key: &str| lookup(&lines, key);

        let mut s = Settings::defaults(confirm);
        let bool_of =|key: &str, default: bool, warnings: &mut Vec<String>| match get(key) {
            None => default,
            Some("true") => true,
            Some("false") => false,
            Some(other) => {
                warnings.push(format!("{key}: expected true or false, not {other:?}"));
                default
            }
        };
        s.first_run_done = bool_of("first_run_done", false, &mut warnings);
        s.show_fps = bool_of("show_fps", false, &mut warnings);
        s.gear_auto_dim = bool_of("gear_auto_dim", true, &mut warnings);
        if let Some(value) = get("glyphs") {
            match GlyphStyle::from_name(value) {
                Some(style) => s.glyph_style = style,
                None => warnings.push(format!("glyphs: unknown style {value:?}")),
            }
        }
        if let Some(value) = get("cursor") {
            match cursor_style_from_name(value) {
                Some(style) => s.cursor_style = style,
                None => warnings.push(format!("cursor: unknown look {value:?}")),
            }
        }
        if let Some(value) = get("cursor_colour") {
            match cursor_colour_from_name(value) {
                Some(colour) => s.cursor_colour = colour,
                None => warnings.push(format!("cursor_colour: unknown colour {value:?}")),
            }
        }
        match get("password_typing") {
            None => {}
            Some("keyboard") => s.device_keyboard = true,
            Some("controller") => s.device_keyboard = false,
            Some(other) => {
                warnings.push(format!("password_typing: expected keyboard or controller, not {other:?}"))
            }
        }
        if let Some(value) = get("scroll_speed") {
            match ScrollSpeed::from_name(value) {
                Some(speed) => s.scroll_speed = speed,
                None => warnings.push(format!("scroll_speed: unknown speed {value:?}")),
            }
        }
        if let Some(value) = get("deadzone") {
            match value.parse::<f32>() {
                Ok(d) if (0.0..=0.9).contains(&d) => s.deadzone = Some(d),
                _ => warnings.push(format!("deadzone: expected 0 to 0.9, not {value:?}")),
            }
        }
        let (pad_bindings, pad_warnings) = Bindings::from_settings(get, confirm);
        s.pad = pad_bindings;
        warnings.extend(pad_warnings);
        let (key_bindings, key_warnings) = KeyBindings::from_settings(get);
        s.keys = key_bindings;
        warnings.extend(key_warnings);

        s.other = lines
            .iter()
            .filter(|(k, _)| !is_known_key(k))
            .cloned()
            .collect();
        (s, warnings)
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# Song Summoner Revival settings, written by the Setup menu\n\
             # (F2, Select + Start, or the gear on Android). You can edit this\n\
             # file while the game is closed.\n",
        );
        let mut line = |k: &str, v: &str| {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push('\n');
        };
        line("first_run_done", bool_str(self.first_run_done));
        line("glyphs", self.glyph_style.name());
        if let Some(d) = self.deadzone {
            line("deadzone", &format!("{d:.2}"));
        }
        line("show_fps", bool_str(self.show_fps));
        line("gear_auto_dim", bool_str(self.gear_auto_dim));
        line("scroll_speed", self.scroll_speed.name());
        line("cursor", cursor_style_name(self.cursor_style));
        line("cursor_colour", cursor_colour_name(self.cursor_colour));
        line(
            "password_typing",
            if self.device_keyboard { "keyboard" } else { "controller" },
        );
        for (k, v) in self.pad.to_settings() {
            line(&k, &v);
        }
        for (k, v) in self.keys.to_settings() {
            line(&k, &v);
        }
        for (k, v) in &self.other {
            line(k, v);
        }
        out
    }

    /// Read the file. A missing file gives the defaults; warnings are logged.
    pub fn load(path: &Path, confirm: ConfirmButton) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (settings, warnings) = Settings::parse(&text, confirm);
                for w in warnings {
                    log!("setup: {}: {}", path.display(), w);
                }
                settings
            }
            Err(_) => Settings::defaults(confirm),
        }
    }

    /// Write the file through a temporary one, so a crash mid-write never
    /// leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }
}

fn lookup<'a>(lines: &'a [(String, String)], key: &str) -> Option<&'a str> {
    lines
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

fn is_known_key(key: &str) -> bool {
    matches!(
        key,
        "first_run_done"
            | "glyphs"
            | "deadzone"
            | "show_fps"
            | "gear_auto_dim"
            | "scroll_speed"
            | "password_typing"
            | "cursor"
            | "cursor_colour"
    ) || key.starts_with("pad.")
        || key.starts_with("key.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::PadButton;
    use pad::Role;

    #[test]
    fn missing_file_gives_the_defaults() {
        let path = std::env::temp_dir().join("touchHLE-no-such-settings-file.txt");
        let s = Settings::load(&path, ConfirmButton::South);
        assert_eq!(s, Settings::defaults(ConfirmButton::South));
        assert!(!s.first_run_done);
    }

    #[test]
    fn empty_text_gives_the_defaults() {
        let (s, warnings) = Settings::parse("", ConfirmButton::South);
        assert_eq!(s, Settings::defaults(ConfirmButton::South));
        assert!(warnings.is_empty());
    }

    #[test]
    fn round_trip() {
        let mut s = Settings::defaults(ConfirmButton::South);
        s.first_run_done = true;
        s.glyph_style = GlyphStyle::Nintendo;
        s.deadzone = Some(0.2);
        s.show_fps = true;
        s.gear_auto_dim = false;
        s.scroll_speed = ScrollSpeed::Fast;
        s.device_keyboard = !s.device_keyboard;
        s.cursor_style = CursorStyle::Bold;
        s.cursor_colour = CursorColour::Sky;
        s.pad.bind(Role::Skip, PadButton::FaceWest).unwrap();
        s.keys.bind(Role::Skip, 1, "K").unwrap();
        let (again, warnings) = Settings::parse(&s.to_text(), ConfirmButton::South);
        assert_eq!(again, s);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_cursor_look_is_saved_by_name() {
        let (s, warnings) = Settings::parse("cursor=outline\ncursor_colour=yellow\n", ConfirmButton::South);
        assert_eq!((s.cursor_style, s.cursor_colour), (CursorStyle::Outline, CursorColour::Yellow));
        assert!(warnings.is_empty());
        // Defaults: the game's own look, in gold.
        let d = Settings::defaults(ConfirmButton::South);
        assert_eq!((d.cursor_style, d.cursor_colour), (CursorStyle::Game, CursorColour::Gold));
        let (s, warnings) = Settings::parse("cursor=sparkly\ncursor_colour=red\n", ConfirmButton::South);
        assert_eq!((s.cursor_style, s.cursor_colour), (CursorStyle::Game, CursorColour::Gold));
        assert_eq!(warnings.len(), 2);
        for style in CursorStyle::ALL {
            assert_eq!(cursor_style_from_name(cursor_style_name(style)), Some(style));
        }
        for colour in CursorColour::ALL {
            assert_eq!(cursor_colour_from_name(cursor_colour_name(colour)), Some(colour));
        }
    }

    #[test]
    fn password_typing_is_saved_by_name() {
        let (s, warnings) = Settings::parse("password_typing=keyboard
", ConfirmButton::South);
        assert!(s.device_keyboard && warnings.is_empty());
        let (s, _) = Settings::parse("password_typing=controller
", ConfirmButton::South);
        assert!(!s.device_keyboard);
        assert!(s.to_text().contains("password_typing=controller
"));
        // A bad value keeps the default and warns.
        let (s, warnings) = Settings::parse("password_typing=pen
", ConfirmButton::South);
        assert_eq!(s.device_keyboard, Settings::defaults(ConfirmButton::South).device_keyboard);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn unknown_keys_are_kept_on_rewrite() {
        let text = "first_run_done=true\nfuture_option=42\n# a comment\n";
        let (s, warnings) = Settings::parse(text, ConfirmButton::South);
        assert!(warnings.is_empty());
        let rewritten = s.to_text();
        assert!(rewritten.contains("future_option=42\n"), "{rewritten}");
    }

    #[test]
    fn a_bad_value_only_resets_its_own_key() {
        let text = "first_run_done=true\nshow_fps=maybe\ndeadzone=7\nglyphs=xbox\n";
        let (s, warnings) = Settings::parse(text, ConfirmButton::South);
        assert!(s.first_run_done);
        assert!(!s.show_fps);
        assert_eq!(s.deadzone, None);
        assert_eq!(s.glyph_style, GlyphStyle::Xbox);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn confirm_button_option_only_seeds_unsaved_bindings() {
        // Nothing saved: east confirms, as the option asks.
        let (s, _) = Settings::parse("first_run_done=true\n", ConfirmButton::East);
        assert_eq!(s.pad.button(Role::Confirm), PadButton::FaceEast);
        // Saved: the file wins over the option.
        let saved = Settings::defaults(ConfirmButton::South).to_text();
        let (s, _) = Settings::parse(&saved, ConfirmButton::East);
        assert_eq!(s.pad.button(Role::Confirm), PadButton::FaceSouth);
    }

    #[test]
    fn whitespace_crlf_and_repeats() {
        let text = "\u{feff} show_fps = true \r\nshow_fps=false\r\n";
        let (s, warnings) = Settings::parse(text, ConfirmButton::South);
        assert!(!s.show_fps);
        assert!(warnings.is_empty());
    }

    #[test]
    fn save_then_load() {
        let path = std::env::temp_dir().join(format!(
            "touchHLE-settings-test-{}.txt",
            std::process::id()
        ));
        let mut s = Settings::defaults(ConfirmButton::South);
        s.first_run_done = true;
        s.save(&path).unwrap();
        let again = Settings::load(&path, ConfirmButton::South);
        let _ = std::fs::remove_file(&path);
        assert_eq!(again, s);
    }

    #[test]
    fn scroll_speeds_are_ordered() {
        let (slow, _) = ScrollSpeed::Slow.timing();
        let (normal, _) = ScrollSpeed::Normal.timing();
        let (fast, _) = ScrollSpeed::Fast.timing();
        assert!(slow > normal && normal > fast);
        assert_eq!(ScrollSpeed::Normal.timing(), (pad::REPEAT_DELAY, pad::REPEAT_INTERVAL));
    }
}
