/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Setup menu's logic: tabs, rows, remapping and the restart dialog.
//!
//! Pure state: input comes in as roles, raw buttons and key names, and the
//! menu answers with [Command]s for `setup.rs` to carry out (save the
//! settings, restart, open a folder…). Drawing is `setup_view.rs`.
//!
//! Navigation: the menu opens on the tab list. L1/R1 cycle the tabs from
//! anywhere; Up/Down on the tab list cycle them too. Confirm steps into a
//! tab's rows, Back steps out, and Back on the tab list resumes the game.
//! Changing the game file or the music folders asks first, since the game
//! has to restart for it (Cancel is selected by default).

use super::credits;
use super::keys::{self, KeyBindings, KEY_ACTIONS};
use super::pad::{self, Bindings, Role, REMAPPABLE};
use super::settings::{
    cursor_colour_label, cursor_style_label, CursorColour, CursorStyle, GlyphStyle, ScrollSpeed,
    Settings, DEADZONES,
};
use crate::options::ConfirmButton;
use crate::window::PadButton;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    Desktop,
    Android,
}

pub const PLATFORM: Platform = if cfg!(target_os = "android") {
    Platform::Android
} else {
    Platform::Desktop
};

/// How far one Up/Down past the first or last row scrolls, in points.
pub const SCROLL_STEP: i32 = 40;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Game,
    Music,
    Controller,
    Keyboard,
    Data,
    Credits,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Game => "Game",
            Tab::Music => "Music",
            Tab::Controller => "Controller",
            Tab::Keyboard => "Keyboard",
            Tab::Data => "Data & help",
            Tab::Credits => "Credits",
        }
    }
}

/// The tabs. Both platforms have the keyboard's: Android takes a
/// Bluetooth or USB one.
pub fn tabs(_platform: Platform) -> Vec<Tab> {
    vec![
        Tab::Game,
        Tab::Music,
        Tab::Controller,
        Tab::Keyboard,
        Tab::Data,
        Tab::Credits,
    ]
}

/// Facts about the install the rows show, gathered by `setup.rs`.
#[derive(Clone, Debug, Default)]
pub struct Info {
    /// The game file's name, folder and size in MB.
    pub game_file: Option<(String, String, u64)>,
    /// Each music folder's name and song count (if known).
    pub folders: Vec<(String, Option<usize>)>,
    /// The folder holding saves, log and library, as the user sees it.
    pub data_dir: String,
    pub version: String,
    pub fullscreen: bool,
    /// The newest save backup's file name.
    pub latest_backup: Option<String>,
    /// Android: the save folder the player picked, by name.
    pub save_folder: Option<String>,
}

/// What a row does when chosen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RowId {
    ChangeGameFile,
    GlyphStyle,
    Fullscreen,
    ShowFps,
    GearAutoDim,
    CursorStyle,
    CursorColour,
    PasswordTyping,
    MusicFolder(usize),
    AddMusicFolder,
    Rescan,
    ScrollSpeed,
    PadAction(Role),
    Deadzone,
    ResetPad,
    KeyAction(Role),
    ResetKeys,
    OpenDataFolder,
    SaveFolder,
    BugReport,
    BackupSaves,
    RestoreSaves,
    ButtonTester,
    Licenses,
    TipJar,
    /// Shows something; choosing it does nothing.
    Info,
}

/// What a row shows on its right.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Text(String),
    /// Controller buttons; `joined` draws a + between them (a shortcut).
    Buttons { buttons: Vec<Glyphish>, joined: bool },
    /// Up to two keys; the slot the cursor is on is highlighted.
    Keys(Vec<String>),
}

/// A controller icon: a button, or one of the "whole control" icons.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Glyphish {
    Button(PadButton),
    DPadAll,
    StickLeft,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Heading(String),
    Paragraph(String),
    Item {
        id: RowId,
        label: String,
        sub: Option<String>,
        value: Value,
        /// Choosing it restarts the game.
        restarts: bool,
    },
}

fn item(id: RowId, label: impl Into<String>, sub: Option<String>, value: Value) -> Row {
    Row::Item {
        id,
        label: label.into(),
        sub,
        value,
        restarts: false,
    }
}

fn text(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

fn on_off(b: bool) -> Value {
    text(if b { "On" } else { "Off" })
}

fn deadzone_label(d: Option<f32>) -> String {
    match d {
        None => "Default".to_string(),
        Some(d) => format!("{} %", (d * 100.0).round()),
    }
}

/// The rows of a tab.
pub fn rows(tab: Tab, platform: Platform, settings: &Settings, info: &Info) -> Vec<Row> {
    let android = platform == Platform::Android;
    let mut rows = Vec::new();
    match tab {
        Tab::Game => {
            rows.push(Row::Heading("Game file".into()));
            let (label, sub) = match &info.game_file {
                Some((name, dir, mb)) => (name.clone(), Some(format!("{dir} · {mb} MB"))),
                None => ("No game file".to_string(), None),
            };
            rows.push(Row::Item {
                id: RowId::ChangeGameFile,
                label,
                sub,
                value: text("Change…"),
                restarts: true,
            });
            rows.push(Row::Heading("Display".into()));
            rows.push(item(
                RowId::GlyphStyle,
                "Button icons",
                Some("Auto picks them from the connected controller".into()),
                text(settings.glyph_style.label()),
            ));
            if !android {
                rows.push(item(
                    RowId::Fullscreen,
                    "Fullscreen",
                    Some("Same as F11".into()),
                    on_off(info.fullscreen),
                ));
            }
            rows.push(item(RowId::ShowFps, "Show FPS", None, on_off(settings.show_fps)));
            if android {
                rows.push(item(
                    RowId::GearAutoDim,
                    "Fade the gear button",
                    Some("Fades after a few seconds, comes back on a touch".into()),
                    on_off(settings.gear_auto_dim),
                ));
            }
            rows.push(Row::Heading("Cursor".into()));
            rows.push(item(
                RowId::CursorStyle,
                "Look",
                Some("Outlined and Bold are easier to see. This menu shows it".into()),
                text(cursor_style_label(settings.cursor_style)),
            ));
            rows.push(item(
                RowId::CursorColour,
                "Colour",
                Some("Always on a dark edge, so any colour stands out".into()),
                text(cursor_colour_label(settings.cursor_colour)),
            ));
            rows.push(Row::Heading("Shop password".into()));
            let (typing, how) = match (settings.device_keyboard, android) {
                (false, _) => ("Controller", "On the game's keyboard"),
                (true, true) => ("Phone keyboard", "Opens with the game's keyboard"),
                (true, false) => ("Keyboard too", "Type it, or use the controller"),
            };
            rows.push(item(RowId::PasswordTyping, "Type it with", Some(how.into()), text(typing)));
        }
        Tab::Music => {
            rows.push(Row::Heading("Music folders".into()));
            for (i, (name, songs)) in info.folders.iter().enumerate() {
                rows.push(Row::Item {
                    id: RowId::MusicFolder(i),
                    label: name.clone(),
                    sub: songs.map(|n| format!("{} songs", with_thousands(n))),
                    value: text("Remove"),
                    restarts: true,
                });
            }
            rows.push(Row::Item {
                id: RowId::AddMusicFolder,
                label: "+ Add music folder".into(),
                sub: None,
                value: Value::None,
                restarts: true,
            });
            rows.push(Row::Heading("Library".into()));
            rows.push(Row::Item {
                id: RowId::Rescan,
                label: "Rescan now".into(),
                sub: Some("Picks up new and changed songs".into()),
                value: Value::None,
                restarts: true,
            });
            rows.push(item(
                RowId::ScrollSpeed,
                "Scroll speed",
                Some("How fast a held direction moves through lists".into()),
                text(settings.scroll_speed.label()),
            ));
        }
        Tab::Controller => {
            rows.push(Row::Heading("Actions: choose one, then press its new button".into()));
            for role in REMAPPABLE {
                rows.push(item(
                    RowId::PadAction(role),
                    action_label(role),
                    None,
                    Value::Buttons {
                        buttons: vec![Glyphish::Button(settings.pad.button(role))],
                        joined: false,
                    },
                ));
            }
            rows.push(Row::Heading("Fixed".into()));
            rows.push(item(
                RowId::Info,
                "Move / scroll",
                Some("The D-pad and left stick do the same thing".into()),
                Value::Buttons {
                    buttons: vec![Glyphish::DPadAll, Glyphish::StickLeft],
                    joined: false,
                },
            ));
            rows.push(item(
                RowId::Info,
                "Open Setup",
                Some("Hold Select, then press Start".into()),
                Value::Buttons {
                    buttons: vec![
                        Glyphish::Button(PadButton::Back),
                        Glyphish::Button(PadButton::Start),
                    ],
                    joined: true,
                },
            ));
            rows.push(Row::Heading("Stick".into()));
            rows.push(item(
                RowId::Deadzone,
                "Left stick dead zone",
                Some("Raise it if the cursor drifts on its own".into()),
                text(deadzone_label(settings.deadzone)),
            ));
            rows.push(item(RowId::ResetPad, "Reset controller to defaults", None, Value::None));
        }
        Tab::Keyboard => {
            rows.push(Row::Heading("Actions: two keys each; Delete clears one".into()));
            for role in KEY_ACTIONS {
                let keys = settings
                    .keys
                    .keys(role)
                    .into_iter()
                    .map(|k| k.map(keys::key_label).unwrap_or_default())
                    .collect();
                rows.push(item(
                    RowId::KeyAction(role),
                    action_label(role),
                    None,
                    Value::Keys(keys),
                ));
            }
            rows.push(item(
                RowId::Info,
                "Open Setup",
                Some("Fixed".into()),
                Value::Keys(vec![keys::MENU_KEY.to_string()]),
            ));
            rows.push(item(RowId::ResetKeys, "Reset keyboard to defaults", None, Value::None));
        }
        Tab::Data => {
            rows.push(Row::Heading("Folders".into()));
            if android {
                rows.push(Row::Item {
                    id: RowId::SaveFolder,
                    label: info
                        .save_folder
                        .clone()
                        .unwrap_or_else(|| "No save folder".to_string()),
                    sub: Some("Your saves, settings, backups and bug reports are kept here".into()),
                    value: text("Change…"),
                    restarts: true,
                });
            }
            rows.push(Row::Item {
                id: RowId::OpenDataFolder,
                label: "Open data folder".into(),
                sub: Some(if android {
                    "Shows your save folder in the Files app".to_string()
                } else {
                    format!("{}: saves, log, music index", info.data_dir)
                }),
                value: Value::None,
                restarts: android,
            });
            rows.push(Row::Heading("Support".into()));
            rows.push(item(
                RowId::BugReport,
                "Make a bug-report file",
                Some("The log, options, settings and versions. Never the game or your saves.".into()),
                Value::None,
            ));
            rows.push(item(
                RowId::BackupSaves,
                "Back up saves",
                Some("Copies your saves to backups/ in the data folder".into()),
                Value::None,
            ));
            if let Some(name) = &info.latest_backup {
                rows.push(Row::Item {
                    id: RowId::RestoreSaves,
                    label: "Restore saves".into(),
                    sub: Some(format!("From {name}")),
                    value: Value::None,
                    restarts: true,
                });
            }
            rows.push(item(
                RowId::ButtonTester,
                "Button tester",
                Some("Shows what the game gets from each button".into()),
                Value::None,
            ));
            rows.push(Row::Heading("Version".into()));
            rows.push(item(
                RowId::Info,
                format!("Song Summoner Revival {}", info.version),
                Some("Include this in bug reports".into()),
                Value::None,
            ));
        }
        Tab::Credits => {
            rows.push(Row::Heading("Emulator".into()));
            rows.push(item(
                RowId::Info,
                "touchHLE",
                Some(format!(
                    "{} {} · {}",
                    credits::TOUCHHLE_COPYRIGHT,
                    credits::TOUCHHLE_LICENSE,
                    credits::TOUCHHLE_URL
                )),
                Value::None,
            ));
            rows.push(item(
                RowId::Info,
                "Source code",
                Some(credits::SOURCE_URL.into()),
                Value::None,
            ));
            rows.push(Row::Heading("Button icons".into()));
            rows.push(item(
                RowId::Info,
                "Controller icons: Zacksly",
                Some(credits::ZACKSLY_CREDIT.into()),
                Value::None,
            ));
            rows.push(item(
                RowId::Info,
                "Gear icon: Material Icons",
                Some(credits::MATERIAL_CREDIT.into()),
                Value::None,
            ));
            rows.push(Row::Heading("Song Summoner".into()));
            rows.push(Row::Paragraph(format!(
                "{} {}",
                credits::SQUARE_ENIX_COPYRIGHT,
                credits::SQUARE_ENIX_RIGHTS
            )));
            rows.push(Row::Paragraph(credits::NOT_AFFILIATED.into()));
            rows.push(Row::Paragraph(credits::SUPPORT_SQUARE_ENIX.into()));
            rows.push(item(
                RowId::Licenses,
                "Open-source licenses",
                Some("Saves them to licenses.txt in the data folder".into()),
                Value::None,
            ));
            rows.push(Row::Heading("Support the developer".into()));
            rows.push(Row::Paragraph(credits::TIP_TEXT.into()));
            rows.push(Row::Item {
                id: RowId::TipJar,
                label: credits::TIP_TITLE.into(),
                sub: Some(credits::TIP_URL.into()),
                value: text("Open"),
                // Android's browser can't open over the game.
                restarts: android,
            });
        }
    }
    rows
}

pub fn action_label(role: Role) -> &'static str {
    match role {
        Role::Up => "Up",
        Role::Down => "Down",
        Role::PrevSection => "Left",
        Role::NextSection => "Right",
        Role::Confirm => "Confirm",
        Role::Back => "Back",
        Role::Info => "Info / status",
        Role::PrevTab => "Previous tab",
        Role::NextTab => "Next tab",
        Role::ZoomOut => "Zoom out (battle)",
        Role::ZoomIn => "Zoom in (battle)",
        Role::Skip => "Skip cutscene",
    }
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

/// A change that needs the game to restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Desktop: the file already picked. Android: `None`, picked after
    /// the game closes.
    GameFile(Option<String>),
    AddFolder(Option<String>),
    RemoveFolder(usize),
    Rescan,
    /// Android only: the file manager can't open while the game runs.
    OpenDataFolder,
    /// Put this backup's saves back, at the next start.
    RestoreSaves(String),
    /// Android: pick another save folder.
    SaveFolder,
    /// Android: open the developer's tip page in the browser.
    TipPage,
}

/// The restart dialog's title, text and Yes label.
pub fn dialog_text(change: &Change, platform: Platform, info: &Info) -> (String, String, String) {
    let android = platform == Platform::Android;
    let lost = if android {
        "Song Summoner closes now; anything since your last in-game save is lost. \
         It starts again by itself when this is done."
    } else {
        "The game restarts to use it; anything since your last in-game save is lost."
    };
    let (title, what, yes) = match change {
        Change::GameFile(picked) => (
            "Change the game file?",
            match picked {
                Some(path) => format!("New file: {path} (checked OK)."),
                None => "The file picker opens after the game closes. Your current \
                         file stays until the new one passes the check."
                    .to_string(),
            },
            if android { "Close and pick file" } else { "Save and restart" },
        ),
        Change::AddFolder(picked) => (
            "Add a music folder?",
            match picked {
                Some(path) => format!("New folder: {path}. It is scanned during the restart."),
                None => "The folder picker opens after the game closes, then the new \
                         folder is scanned."
                    .to_string(),
            },
            if android { "Close and pick folder" } else { "Save and restart" },
        ),
        Change::RemoveFolder(i) => {
            let (name, songs) = info.folders.get(*i).cloned().unwrap_or_default();
            let count = match songs {
                Some(n) => format!("Its {} songs leave", with_thousands(n)),
                None => "Its songs leave".to_string(),
            };
            // Desktop: the first folder's songs are the only ones with
            // folder-relative IDs, so the next folder's songs get new ones.
            let renames = !android && *i == 0 && info.folders.len() > 1;
            let warning = if renames {
                " Songs in your other folders will count as new songs, so \
                 troopers made from them lose their song."
            } else {
                ""
            };
            (
                "Remove this music folder?",
                format!(
                    "{name}: {count} your library. The files stay where they \
                     are.{warning}"
                ),
                "Remove and restart",
            )
        }
        Change::Rescan => (
            "Rescan your music?",
            "New and changed songs are picked up.".to_string(),
            "Rescan and restart",
        ),
        Change::SaveFolder => (
            "Change the save folder?",
            "The folder picker opens after the game closes. If the folder you pick \
             already has saves, you choose which saves to keep."
                .to_string(),
            "Close and pick folder",
        ),
        Change::RestoreSaves(name) => (
            "Restore your saves?",
            format!(
                "Your saves go back to {name}. The ones you have now are backed up                  first, so this can be undone."
            ),
            "Restore and restart",
        ),
        Change::OpenDataFolder => (
            "Open the data folder?",
            format!(
                "Your file manager opens at {} after the game closes.",
                info.data_dir
            ),
            "Close and open folder",
        ),
        Change::TipPage => (
            "Open the tip page?",
            format!(
                "Your browser opens at {} after the game closes. Thank you!",
                credits::TIP_URL
            ),
            "Close and open page",
        ),
    };
    (title.to_string(), format!("{what} {lost}"), yes.to_string())
}

/// What `setup.rs` should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Resume the game.
    Close,
    /// Write the settings file (they've already changed).
    SaveSettings,
    /// Desktop, first launch: tell the player how to get back here.
    ShowReopenToast,
    /// Desktop: ask for a file or folder, then call [Menu::picked].
    PickGameFile,
    PickMusicFolder,
    /// The player said yes: save it and restart.
    Apply(Change),
    /// Desktop: open Explorer at the data folder.
    OpenDataFolder,
    /// Desktop: open the tip page in the browser.
    OpenTipPage,
    ToggleFullscreen,
    BugReport,
    BackupSaves,
    WriteLicenses,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    Tabs,
    Rows,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Waiting for the new button for an action.
    WaitPad(Role),
    /// Waiting for the new key for an action's slot.
    WaitKey(Role, usize),
    /// The restart dialog. `yes` is the selected button.
    Dialog { change: Change, yes: bool },
    /// Showing what each input does. Select, F2 or a tap leaves.
    Tester { last: Option<(Tested, String)> },
}

/// What the button tester last saw, and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tested {
    /// Drawn as its icon: never text for controller buttons.
    Button(PadButton),
    /// A key, by its label.
    Key(String),
}

pub struct Menu {
    pub platform: Platform,
    pub tab: usize,
    pub focus: Focus,
    /// Among the tab's items (headings and paragraphs don't count).
    pub row: usize,
    /// Keyboard rows: which of the two keys.
    pub key_slot: usize,
    pub mode: Mode,
    /// A short message for the footer ("Saved").
    pub flash: Option<String>,
    /// The view's scroll position, in rows' pixels.
    pub scroll: i32,
    /// The view keeps the selected row on screen. Off after a drag or a
    /// scroll past the rows, until the pad or keyboard moves the
    /// selection again.
    pub follow: bool,
    /// Scrolling asked for past the first or last row (Up/Down there),
    /// which the view applies and clears, since only it knows the
    /// content's height.
    pub nudge: i32,
    /// For resetting the controller to the stock mapping.
    confirm: ConfirmButton,
}

impl Menu {
    pub fn new(platform: Platform, confirm: ConfirmButton) -> Menu {
        Menu {
            platform,
            tab: 0,
            focus: Focus::Tabs,
            row: 0,
            key_slot: 0,
            mode: Mode::Normal,
            flash: None,
            scroll: 0,
            follow: true,
            nudge: 0,
            confirm,
        }
    }

    /// The first menu, with no music folder yet: start on the Music tab,
    /// since that's what's left to set up.
    pub fn start_on_music(&mut self, no_music: bool) {
        if no_music {
            if let Some(i) = self.tabs().iter().position(|&t| t == Tab::Music) {
                self.tab = i;
            }
        }
    }

    pub fn tabs(&self) -> Vec<Tab> {
        tabs(self.platform)
    }
    pub fn current_tab(&self) -> Tab {
        self.tabs()[self.tab]
    }
    pub fn rows(&self, settings: &Settings, info: &Info) -> Vec<Row> {
        rows(self.current_tab(), self.platform, settings, info)
    }
    /// The ids of the current tab's items, in order.
    pub fn items(&self, settings: &Settings, info: &Info) -> Vec<RowId> {
        self.rows(settings, info)
            .into_iter()
            .filter_map(|r| match r {
                Row::Item { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }
    fn selected(&self, settings: &Settings, info: &Info) -> Option<RowId> {
        self.items(settings, info).get(self.row).copied()
    }

    /// Leave the menu. The first time, the menu has done its first-run
    /// job and never opens by itself again.
    fn close(&mut self, settings: &mut Settings) -> Vec<Command> {
        let mut out = Vec::new();
        if !settings.first_run_done {
            settings.first_run_done = true;
            out.push(Command::SaveSettings);
            if self.platform == Platform::Desktop {
                out.push(Command::ShowReopenToast);
            }
        }
        out.push(Command::Close);
        out
    }

    fn switch_tab(&mut self, step: i32) {
        self.tab = pad::tab_after_of(self.tab, step, self.tabs().len());
        self.row = 0;
        self.key_slot = 0;
        self.scroll = 0;
        self.follow = true;
        self.nudge = 0;
    }

    /// A finger dragged `dy` points (down is positive) since it went down
    /// with the view at `from`: the content moves with it.
    pub fn drag(&mut self, from: i32, dy: i32) {
        self.scroll = from - dy;
        self.nudge = 0;
        self.follow = false;
    }

    /// A wait for a button or key: a tap anywhere cancels it.
    fn cancel_wait(&mut self) -> bool {
        if matches!(self.mode, Mode::WaitPad(_) | Mode::WaitKey(..)) {
            self.mode = Mode::Normal;
            return true;
        }
        false
    }

    /// F2, or Select + Start, while the menu is open: leave whatever is
    /// waiting, or close the menu.
    pub fn menu_key(&mut self, settings: &mut Settings) -> Vec<Command> {
        match self.mode {
            Mode::Normal => self.close(settings),
            _ => {
                self.mode = Mode::Normal;
                Vec::new()
            }
        }
    }

    /// Select on its own: cancels a wait, leaves the tester.
    pub fn select_pressed(&mut self) {
        if matches!(self.mode, Mode::WaitPad(_) | Mode::Tester { .. }) {
            self.mode = Mode::Normal;
        }
    }

    /// A controller button (not Select, which comes through
    /// [Self::select_pressed]).
    pub fn pad_button(
        &mut self,
        button: PadButton,
        pressed: bool,
        settings: &mut Settings,
        info: &Info,
    ) -> Vec<Command> {
        if !pressed {
            return Vec::new();
        }
        match self.mode.clone() {
            Mode::WaitPad(role) => {
                match settings.pad.bind(role, button) {
                    Ok(()) => {
                        self.flash = Some("Saved".into());
                        self.mode = Mode::Normal;
                        return vec![Command::SaveSettings];
                    }
                    Err(_) => {
                        self.flash = Some("That button can't be used".into());
                    }
                }
                Vec::new()
            }
            Mode::Tester { .. } => {
                let does = settings
                    .pad
                    .role(button)
                    .map_or("nothing", action_label);
                self.mode = Mode::Tester {
                    last: Some((Tested::Button(button), does.to_string())),
                };
                Vec::new()
            }
            _ => match settings.pad.role(button) {
                Some(role) => self.role(role, settings, info),
                None => Vec::new(),
            },
        }
    }

    /// A key (desktop), by SDL scancode name.
    pub fn key(&mut self, key: &str, settings: &mut Settings, info: &Info) -> Vec<Command> {
        match self.mode.clone() {
            Mode::WaitKey(role, slot) => {
                match settings.keys.bind(role, slot, key) {
                    Ok(()) => {
                        self.flash = Some("Saved".into());
                        self.mode = Mode::Normal;
                        return vec![Command::SaveSettings];
                    }
                    Err(keys::KeyBindError::Reserved) => {
                        self.flash = Some("F2, F11 and F12 are taken".into());
                    }
                    Err(_) => {
                        self.flash = Some("That key is another action's only key".into());
                    }
                }
                Vec::new()
            }
            Mode::Tester { .. } => {
                let does = settings.keys.role(key).map_or("nothing", action_label);
                self.mode = Mode::Tester {
                    last: Some((Tested::Key(keys::key_label(key)), does.to_string())),
                };
                Vec::new()
            }
            _ => {
                if key == "Delete" && self.focus == Focus::Rows {
                    if let Some(RowId::KeyAction(role)) = self.selected(settings, info) {
                        return match settings.keys.clear(role, self.key_slot) {
                            Ok(()) => {
                                self.key_slot = 0;
                                vec![Command::SaveSettings]
                            }
                            Err(_) => {
                                self.flash = Some("Each action needs a key".into());
                                Vec::new()
                            }
                        };
                    }
                }
                match settings.keys.role(key) {
                    Some(role) => self.role(role, settings, info),
                    None => Vec::new(),
                }
            }
        }
    }

    /// A command, already mapped from its button or key.
    pub fn role(&mut self, role: Role, settings: &mut Settings, info: &Info) -> Vec<Command> {
        match self.mode.clone() {
            Mode::Dialog { change, yes } => {
                match role {
                    // Cancel is on the left and Yes on the right, and
                    // left/right always pick them, never wrapping round
                    // (the user's rule for every dialog, 2026-09-28).
                    Role::PrevSection => self.mode = Mode::Dialog { change, yes: false },
                    Role::NextSection => self.mode = Mode::Dialog { change, yes: true },
                    Role::Back => self.mode = Mode::Normal,
                    Role::Confirm => {
                        self.mode = Mode::Normal;
                        if yes {
                            return vec![Command::Apply(change)];
                        }
                    }
                    _ => (),
                }
                return Vec::new();
            }
            Mode::Normal => (),
            // Waits and the tester take raw input only.
            _ => return Vec::new(),
        }
        match (role, self.focus) {
            (Role::PrevTab, _) => self.switch_tab(-1),
            (Role::NextTab, _) => self.switch_tab(1),
            (Role::Up, Focus::Tabs) => self.switch_tab(-1),
            (Role::Down, Focus::Tabs) => self.switch_tab(1),
            (Role::Confirm, Focus::Tabs) => {
                self.focus = Focus::Rows;
                self.row = 0;
                self.key_slot = 0;
                self.follow = true;
            }
            (Role::Back, Focus::Tabs) => return self.close(settings),
            // Past the first or last row, Up/Down scroll the text there
            // (Credits ends in paragraphs no row reaches).
            (Role::Up, Focus::Rows) if self.row == 0 => {
                self.nudge -= SCROLL_STEP;
                self.follow = false;
            }
            (Role::Up, Focus::Rows) => {
                self.row -= 1;
                self.key_slot = 0;
                self.follow = true;
            }
            (Role::Down, Focus::Rows) => {
                let count = self.items(settings, info).len();
                if self.row + 1 < count {
                    self.row += 1;
                    self.key_slot = 0;
                    self.follow = true;
                } else {
                    self.nudge += SCROLL_STEP;
                    self.follow = false;
                }
            }
            (Role::Back, Focus::Rows) => self.focus = Focus::Tabs,
            (Role::Confirm, Focus::Rows) => return self.activate(settings, info),
            (Role::PrevSection, Focus::Rows) => return self.adjust(-1, settings, info),
            (Role::NextSection, Focus::Rows) => return self.adjust(1, settings, info),
            _ => (),
        }
        Vec::new()
    }

    fn activate(&mut self, settings: &mut Settings, info: &Info) -> Vec<Command> {
        let android = self.platform == Platform::Android;
        let Some(id) = self.selected(settings, info) else {
            return Vec::new();
        };
        let dialog = |change| Mode::Dialog { change, yes: false };
        match id {
            RowId::ChangeGameFile if android => self.mode = dialog(Change::GameFile(None)),
            RowId::ChangeGameFile => return vec![Command::PickGameFile],
            RowId::AddMusicFolder if android => self.mode = dialog(Change::AddFolder(None)),
            RowId::AddMusicFolder => return vec![Command::PickMusicFolder],
            RowId::MusicFolder(i) => self.mode = dialog(Change::RemoveFolder(i)),
            RowId::Rescan => self.mode = dialog(Change::Rescan),
            RowId::OpenDataFolder if android => self.mode = dialog(Change::OpenDataFolder),
            RowId::OpenDataFolder => return vec![Command::OpenDataFolder],
            RowId::TipJar if android => self.mode = dialog(Change::TipPage),
            RowId::TipJar => return vec![Command::OpenTipPage],
            RowId::SaveFolder => self.mode = dialog(Change::SaveFolder),
            RowId::PadAction(role) => self.mode = Mode::WaitPad(role),
            RowId::KeyAction(role) => self.mode = Mode::WaitKey(role, self.key_slot),
            RowId::ResetPad => {
                settings.pad = Bindings::defaults(self.confirm);
                self.flash = Some("Controller reset".into());
                return vec![Command::SaveSettings];
            }
            RowId::ResetKeys => {
                settings.keys = KeyBindings::default();
                self.flash = Some("Keyboard reset".into());
                return vec![Command::SaveSettings];
            }
            RowId::BugReport => return vec![Command::BugReport],
            RowId::BackupSaves => return vec![Command::BackupSaves],
            RowId::RestoreSaves => match &info.latest_backup {
                Some(name) => self.mode = dialog(Change::RestoreSaves(name.clone())),
                None => (),
            },
            RowId::Licenses => return vec![Command::WriteLicenses],
            RowId::ButtonTester => self.mode = Mode::Tester { last: None },
            RowId::GlyphStyle
            | RowId::Fullscreen
            | RowId::ShowFps
            | RowId::GearAutoDim
            | RowId::PasswordTyping
            | RowId::CursorStyle
            | RowId::CursorColour
            | RowId::ScrollSpeed
            | RowId::Deadzone => return self.adjust(1, settings, info),
            RowId::Info => (),
        }
        Vec::new()
    }

    /// Left/Right on a row: step a setting, or pick a key slot.
    fn adjust(&mut self, step: i32, settings: &mut Settings, info: &Info) -> Vec<Command> {
        fn cycle<T: Copy + PartialEq>(all: &[T], now: T, step: i32) -> T {
            let i = all.iter().position(|&x| x == now).unwrap_or(0);
            all[(i as i32 + step).rem_euclid(all.len() as i32) as usize]
        }
        match self.selected(settings, info) {
            Some(RowId::GlyphStyle) => {
                settings.glyph_style = cycle(&GlyphStyle::ALL, settings.glyph_style, step);
            }
            Some(RowId::ScrollSpeed) => {
                settings.scroll_speed = cycle(&ScrollSpeed::ALL, settings.scroll_speed, step);
            }
            Some(RowId::ShowFps) => settings.show_fps = !settings.show_fps,
            Some(RowId::GearAutoDim) => settings.gear_auto_dim = !settings.gear_auto_dim,
            Some(RowId::PasswordTyping) => settings.device_keyboard = !settings.device_keyboard,
            Some(RowId::CursorStyle) => {
                settings.cursor_style = cycle(&CursorStyle::ALL, settings.cursor_style, step);
            }
            Some(RowId::CursorColour) => {
                settings.cursor_colour = cycle(&CursorColour::ALL, settings.cursor_colour, step);
            }
            Some(RowId::Fullscreen) => return vec![Command::ToggleFullscreen],
            Some(RowId::Deadzone) => {
                let mut steps: Vec<Option<f32>> = vec![None];
                steps.extend(DEADZONES.iter().map(|&d| Some(d)));
                settings.deadzone = cycle(&steps, settings.deadzone, step);
            }
            Some(RowId::KeyAction(_)) => {
                self.key_slot = if step < 0 { 0 } else { 1 };
                return Vec::new();
            }
            _ => return Vec::new(),
        }
        vec![Command::SaveSettings]
    }

    /// Desktop: the file or folder the player picked, already checked.
    pub fn picked(&mut self, change: Change) {
        self.mode = Mode::Dialog { change, yes: false };
    }

    // --- Touch: the view says what was tapped. ---

    pub fn tap_tab(&mut self, i: usize, settings: &mut Settings, info: &Info) -> Vec<Command> {
        if self.cancel_wait() || self.mode != Mode::Normal {
            return Vec::new();
        }
        if self.focus == Focus::Tabs && i == self.tab {
            return self.role(Role::Confirm, settings, info);
        }
        if i != self.tab {
            self.tab = i;
            self.row = 0;
            self.scroll = 0;
            self.nudge = 0;
            self.follow = true;
        }
        self.focus = Focus::Tabs;
        Vec::new()
    }

    /// One tap on a row (and key slot) chooses it, like Confirm on it (the
    /// user found two taps clumsy, 2026-09-28). A tap while waiting for a
    /// button or key only cancels the wait.
    pub fn tap_row(&mut self, i: usize, slot: Option<usize>, settings: &mut Settings, info: &Info) -> Vec<Command> {
        if self.cancel_wait() || self.mode != Mode::Normal {
            return Vec::new();
        }
        self.focus = Focus::Rows;
        self.row = i;
        if let Some(slot) = slot {
            self.key_slot = slot;
        }
        // The tapped row is on screen already; don't jump the view.
        self.follow = false;
        self.activate(settings, info)
    }

    /// A tap on the panel off every row and tab.
    pub fn tap_panel(&mut self) {
        self.cancel_wait();
    }

    pub fn tap_dialog(&mut self, yes: bool, settings: &mut Settings, info: &Info) -> Vec<Command> {
        if let Mode::Dialog { change, .. } = self.mode.clone() {
            self.mode = Mode::Dialog { change, yes };
            return self.role(Role::Confirm, settings, info);
        }
        Vec::new()
    }

    /// A tap outside the panel: the same as Back, one level at a time.
    pub fn tap_outside(&mut self, settings: &mut Settings, info: &Info) -> Vec<Command> {
        match self.mode {
            Mode::Normal => self.role(Role::Back, settings, info),
            _ => {
                self.mode = Mode::Normal;
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(platform: Platform) -> (Menu, Settings, Info) {
        let info = Info {
            game_file: Some(("Song Summoner.ipa".into(), "C:\\Games".into(), 213)),
            folders: vec![("D:\\Music".into(), Some(1284)), ("E:\\OSTs".into(), Some(412))],
            data_dir: "C:\\Games".into(),
            version: "0.2.3".into(),
            fullscreen: false,
            latest_backup: Some("saves-2026-09-28_1904.zip".into()),
            save_folder: Some("Internal storage › Documents › SongSummoner".into()),
        };
        (
            Menu::new(platform, ConfirmButton::South),
            Settings::defaults(ConfirmButton::South),
            info,
        )
    }

    fn go(menu: &mut Menu, s: &mut Settings, info: &Info, roles: &[Role]) -> Vec<Command> {
        let mut out = Vec::new();
        for &r in roles {
            out.extend(menu.role(r, s, info));
        }
        out
    }

    fn tab_index(menu: &Menu, tab: Tab) -> usize {
        menu.tabs().iter().position(|&t| t == tab).unwrap()
    }

    /// Open `tab`'s rows and put the cursor on `id`.
    fn select(menu: &mut Menu, s: &Settings, info: &Info, tab: Tab, id: RowId) {
        menu.tab = tab_index(menu, tab);
        menu.focus = Focus::Rows;
        menu.row = menu.items(s, info).iter().position(|&r| r == id).unwrap();
    }

    #[test]
    fn opens_on_the_tabs() {
        let (menu, _, _) = setup(Platform::Desktop);
        assert_eq!(menu.focus, Focus::Tabs);
        assert_eq!(menu.current_tab(), Tab::Game);
        assert_eq!(menu.mode, Mode::Normal);
    }

    #[test]
    fn shoulders_cycle_tabs_from_either_level_and_wrap() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        let n = m.tabs().len();
        go(&mut m, &mut s, &info, &[Role::PrevTab]);
        assert_eq!(m.tab, n - 1);
        go(&mut m, &mut s, &info, &[Role::NextTab]);
        assert_eq!(m.tab, 0);
        // From the rows too, staying in the rows.
        go(&mut m, &mut s, &info, &[Role::Confirm, Role::Down, Role::NextTab]);
        assert_eq!(m.tab, 1);
        assert_eq!(m.focus, Focus::Rows);
        assert_eq!(m.row, 0);
    }

    #[test]
    fn up_and_down_on_the_tabs_wrap() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        let n = m.tabs().len();
        go(&mut m, &mut s, &info, &[Role::Up]);
        assert_eq!(m.tab, n - 1);
        go(&mut m, &mut s, &info, &[Role::Down]);
        assert_eq!(m.tab, 0);
    }

    #[test]
    fn rows_stop_at_the_ends_and_skip_headings() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        go(&mut m, &mut s, &info, &[Role::Confirm, Role::Up]);
        assert_eq!(m.row, 0);
        let count = m.items(&s, &info).len();
        for _ in 0..count + 5 {
            go(&mut m, &mut s, &info, &[Role::Down]);
        }
        assert_eq!(m.row, count - 1);
        // Items only: the Game tab's first heading isn't one.
        assert_eq!(m.items(&s, &info)[0], RowId::ChangeGameFile);
    }

    #[test]
    fn confirm_steps_in_and_back_steps_out() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(m.focus, Focus::Rows);
        go(&mut m, &mut s, &info, &[Role::Back]);
        assert_eq!(m.focus, Focus::Tabs);
    }

    #[test]
    fn back_on_the_tabs_resumes_the_game() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        s.first_run_done = true;
        let out = go(&mut m, &mut s, &info, &[Role::Back]);
        assert_eq!(out, vec![Command::Close]);
    }

    #[test]
    fn remapping_waits_for_the_next_button() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Controller, RowId::PadAction(Role::Skip));
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(m.mode, Mode::WaitPad(Role::Skip));
        // The release of the Confirm that started it is ignored.
        assert!(m.pad_button(PadButton::FaceSouth, false, &mut s, &info).is_empty());
        let out = m.pad_button(PadButton::FaceWest, true, &mut s, &info);
        assert_eq!(out, vec![Command::SaveSettings]);
        assert_eq!(s.pad.button(Role::Skip), PadButton::FaceWest);
        assert_eq!(m.mode, Mode::Normal);
    }

    #[test]
    fn remapping_to_a_used_button_swaps() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Controller, RowId::PadAction(Role::Confirm));
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        m.pad_button(PadButton::FaceEast, true, &mut s, &info);
        assert_eq!(s.pad.button(Role::Confirm), PadButton::FaceEast);
        assert_eq!(s.pad.button(Role::Back), PadButton::FaceSouth);
    }

    #[test]
    fn select_cancels_a_wait() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Controller, RowId::PadAction(Role::Info));
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        m.select_pressed();
        assert_eq!(m.mode, Mode::Normal);
        assert_eq!(s.pad, Bindings::defaults(ConfirmButton::South));
    }

    #[test]
    fn the_d_pad_cant_take_an_action() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Controller, RowId::PadAction(Role::Info));
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert!(m.pad_button(PadButton::DPadUp, true, &mut s, &info).is_empty());
        assert_eq!(m.mode, Mode::WaitPad(Role::Info));
        assert!(m.flash.is_some());
    }

    #[test]
    fn android_asks_before_the_game_closes_with_cancel_selected() {
        for (tab, id, change) in [
            (Tab::Game, RowId::ChangeGameFile, Change::GameFile(None)),
            (Tab::Music, RowId::AddMusicFolder, Change::AddFolder(None)),
            (Tab::Music, RowId::MusicFolder(1), Change::RemoveFolder(1)),
            (Tab::Music, RowId::Rescan, Change::Rescan),
            (Tab::Data, RowId::OpenDataFolder, Change::OpenDataFolder),
            (Tab::Data, RowId::SaveFolder, Change::SaveFolder),
        ] {
            let (mut m, mut s, info) = setup(Platform::Android);
            select(&mut m, &s, &info, tab, id);
            let out = go(&mut m, &mut s, &info, &[Role::Confirm]);
            assert!(out.is_empty(), "{id:?}");
            assert_eq!(m.mode, Mode::Dialog { change, yes: false }, "{id:?}");
        }
    }

    #[test]
    fn desktop_picks_first_then_asks() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Game, RowId::ChangeGameFile);
        let out = go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(out, vec![Command::PickGameFile]);
        m.picked(Change::GameFile(Some("E:\\new.ipa".into())));
        assert_eq!(
            m.mode,
            Mode::Dialog {
                change: Change::GameFile(Some("E:\\new.ipa".into())),
                yes: false
            }
        );
        // Removing a folder asks straight away, as it needs no picker.
        select(&mut m, &s, &info, Tab::Music, RowId::MusicFolder(0));
        m.mode = Mode::Normal;
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(
            m.mode,
            Mode::Dialog {
                change: Change::RemoveFolder(0),
                yes: false
            }
        );
    }

    #[test]
    fn the_dialog_does_nothing_unless_yes_is_chosen() {
        let (mut m, mut s, info) = setup(Platform::Android);
        select(&mut m, &s, &info, Tab::Music, RowId::Rescan);
        // Straight Confirm: Cancel was selected.
        let out = go(&mut m, &mut s, &info, &[Role::Confirm, Role::Confirm]);
        assert!(out.is_empty());
        assert_eq!(m.mode, Mode::Normal);
        // Back cancels too.
        let out = go(&mut m, &mut s, &info, &[Role::Confirm, Role::Back]);
        assert!(out.is_empty());
        assert_eq!(m.mode, Mode::Normal);
        // Move to Yes, then Confirm.
        let out = go(&mut m, &mut s, &info, &[Role::Confirm, Role::NextSection, Role::Confirm]);
        assert_eq!(out, vec![Command::Apply(Change::Rescan)]);
    }

    #[test]
    fn the_dialog_is_cancel_on_the_left_and_yes_on_the_right() {
        // The user's rule (2026-09-28), as for the game's own dialogs:
        // left always picks Cancel and right Yes, with no wrapping, and
        // up/down do nothing.
        let (mut m, mut s, info) = setup(Platform::Android);
        select(&mut m, &s, &info, Tab::Music, RowId::Rescan);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        let yes = |m: &Menu| matches!(m.mode, Mode::Dialog { yes: true, .. });
        go(&mut m, &mut s, &info, &[Role::PrevSection]);
        assert!(!yes(&m));
        go(&mut m, &mut s, &info, &[Role::NextSection, Role::NextSection]);
        assert!(yes(&m));
        go(&mut m, &mut s, &info, &[Role::Up, Role::Down, Role::Down]);
        assert!(yes(&m));
        go(&mut m, &mut s, &info, &[Role::PrevSection, Role::PrevSection]);
        assert!(!yes(&m));
    }

    #[test]
    fn restoring_saves_asks_first() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Data, RowId::RestoreSaves);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        let change = Change::RestoreSaves("saves-2026-09-28_1904.zip".into());
        assert_eq!(m.mode, Mode::Dialog { change: change.clone(), yes: false });
        // Right is Yes (up/down do nothing in a dialog).
        let out = go(&mut m, &mut s, &info, &[Role::NextSection, Role::Confirm]);
        assert_eq!(out, vec![Command::Apply(change)]);
        // No backup, no row.
        let none = Info { latest_backup: None, ..info };
        assert!(!rows(Tab::Data, Platform::Desktop, &s, &none)
            .iter()
            .any(|r| matches!(r, Row::Item { id: RowId::RestoreSaves, .. })));
    }

    #[test]
    fn both_platforms_have_the_keyboard_tab() {
        // Android takes a Bluetooth or USB keyboard too (2026-09-28).
        assert!(tabs(Platform::Desktop).contains(&Tab::Keyboard));
        assert!(tabs(Platform::Android).contains(&Tab::Keyboard));
    }

    #[test]
    fn closing_the_first_run_menu_remembers_it_and_toasts_on_desktop() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        let out = go(&mut m, &mut s, &info, &[Role::Back]);
        assert!(s.first_run_done);
        assert_eq!(
            out,
            vec![Command::SaveSettings, Command::ShowReopenToast, Command::Close]
        );
        // Android has the gear instead of a toast.
        let (mut m, mut s, info) = setup(Platform::Android);
        let out = go(&mut m, &mut s, &info, &[Role::Back]);
        assert_eq!(out, vec![Command::SaveSettings, Command::Close]);
    }

    #[test]
    fn menu_key_cancels_a_wait_before_closing() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        s.first_run_done = true;
        select(&mut m, &s, &info, Tab::Keyboard, RowId::KeyAction(Role::Skip));
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(m.mode, Mode::WaitKey(Role::Skip, 0));
        assert!(m.menu_key(&mut s).is_empty());
        assert_eq!(m.mode, Mode::Normal);
        assert_eq!(m.menu_key(&mut s), vec![Command::Close]);
    }

    #[test]
    fn keys_rebind_by_slot() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Keyboard, RowId::KeyAction(Role::Skip));
        // Right picks the second slot.
        go(&mut m, &mut s, &info, &[Role::NextSection, Role::Confirm]);
        assert_eq!(m.mode, Mode::WaitKey(Role::Skip, 1));
        let out = m.key("K", &mut s, &info);
        assert_eq!(out, vec![Command::SaveSettings]);
        assert_eq!(s.keys.keys(Role::Skip), [Some("F"), Some("K")]);
        // Delete clears the selected slot.
        m.key_slot = 1;
        assert_eq!(m.key("Delete", &mut s, &info), vec![Command::SaveSettings]);
        assert_eq!(s.keys.keys(Role::Skip), [Some("F"), None]);
    }

    #[test]
    fn keys_drive_the_menu_through_their_bindings() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        m.key("Return", &mut s, &info);
        assert_eq!(m.focus, Focus::Rows);
        m.key("Escape", &mut s, &info);
        assert_eq!(m.focus, Focus::Tabs);
        m.key("E", &mut s, &info);
        assert_eq!(m.tab, 1);
    }

    #[test]
    fn the_cursor_look_steps_with_left_and_right() {
        use crate::gles::present::{CursorColour, CursorStyle};
        let (mut m, mut s, info) = setup(Platform::Android);
        select(&mut m, &s, &info, Tab::Game, RowId::CursorStyle);
        assert_eq!(go(&mut m, &mut s, &info, &[Role::NextSection]), vec![Command::SaveSettings]);
        assert_eq!(s.cursor_style, CursorStyle::Outline);
        go(&mut m, &mut s, &info, &[Role::NextSection, Role::NextSection]);
        assert_eq!(s.cursor_style, CursorStyle::Game);
        select(&mut m, &s, &info, Tab::Game, RowId::CursorColour);
        go(&mut m, &mut s, &info, &[Role::PrevSection]);
        assert_eq!(s.cursor_colour, CursorColour::Sky);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(s.cursor_colour, CursorColour::Gold);
    }

    #[test]
    fn password_typing_is_a_setting_on_both_platforms() {
        for platform in [Platform::Desktop, Platform::Android] {
            let (mut m, mut s, info) = setup(platform);
            let before = s.device_keyboard;
            select(&mut m, &s, &info, Tab::Game, RowId::PasswordTyping);
            let out = go(&mut m, &mut s, &info, &[Role::NextSection]);
            assert_eq!(out, vec![Command::SaveSettings]);
            assert_eq!(s.device_keyboard, !before);
            go(&mut m, &mut s, &info, &[Role::Confirm]);
            assert_eq!(s.device_keyboard, before);
        }
    }

    #[test]
    fn settings_rows_step_with_left_right_and_save() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Game, RowId::GlyphStyle);
        let out = go(&mut m, &mut s, &info, &[Role::NextSection]);
        assert_eq!(out, vec![Command::SaveSettings]);
        assert_eq!(s.glyph_style, GlyphStyle::Xbox);
        go(&mut m, &mut s, &info, &[Role::PrevSection, Role::PrevSection]);
        assert_eq!(s.glyph_style, GlyphStyle::Nintendo);
        select(&mut m, &s, &info, Tab::Controller, RowId::Deadzone);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(s.deadzone, Some(DEADZONES[0]));
    }

    #[test]
    fn the_tester_reports_and_select_leaves() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Data, RowId::ButtonTester);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(m.mode, Mode::Tester { last: None });
        m.pad_button(PadButton::FaceSouth, true, &mut s, &info);
        // The button is shown as its icon, never by a text name.
        assert_eq!(
            m.mode,
            Mode::Tester {
                last: Some((Tested::Button(PadButton::FaceSouth), "Confirm".into()))
            }
        );
        // Back doesn't leave: the player may want to test it.
        m.pad_button(PadButton::FaceEast, true, &mut s, &info);
        assert!(matches!(m.mode, Mode::Tester { .. }));
        m.select_pressed();
        assert_eq!(m.mode, Mode::Normal);
    }

    #[test]
    fn taps_select_then_confirm() {
        let (mut m, mut s, info) = setup(Platform::Android);
        m.tap_tab(1, &mut s, &info);
        assert_eq!((m.tab, m.focus), (1, Focus::Tabs));
        // Tapping the selected tab steps in.
        m.tap_tab(1, &mut s, &info);
        assert_eq!(m.focus, Focus::Rows);
        // One tap on a row chooses it (2026-09-28: two felt broken).
        let rescan = m.items(&s, &info).iter().position(|&r| r == RowId::Rescan).unwrap();
        assert!(m.tap_row(rescan, None, &mut s, &info).is_empty());
        assert!(matches!(m.mode, Mode::Dialog { .. }));
        let out = m.tap_dialog(true, &mut s, &info);
        assert_eq!(out, vec![Command::Apply(Change::Rescan)]);
    }

    #[test]
    fn one_tap_starts_a_remap_and_any_tap_cancels_it() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        m.tap_tab(tab_index(&m, Tab::Controller), &mut s, &info);
        let skip = item_index(&m, &s, &info, RowId::PadAction(Role::Skip));
        m.tap_row(skip, None, &mut s, &info);
        assert_eq!(m.mode, Mode::WaitPad(Role::Skip));
        // A tap anywhere cancels the wait, and does nothing else.
        let before = s.clone();
        m.tap_row(0, None, &mut s, &info);
        assert_eq!(m.mode, Mode::Normal);
        assert_eq!(s, before);
        m.tap_row(skip, None, &mut s, &info);
        m.tap_panel();
        assert_eq!(m.mode, Mode::Normal);
        m.tap_row(skip, None, &mut s, &info);
        m.tap_tab(0, &mut s, &info);
        assert_eq!(m.mode, Mode::Normal);
        assert_eq!(m.current_tab(), Tab::Controller);
        // A key's slot: one tap waits for that slot's key.
        m.tap_tab(tab_index(&m, Tab::Keyboard), &mut s, &info);
        let key = item_index(&m, &s, &info, RowId::KeyAction(Role::Skip));
        m.tap_row(key, Some(1), &mut s, &info);
        assert_eq!(m.mode, Mode::WaitKey(Role::Skip, 1));
    }

    #[test]
    fn a_drag_scrolls_and_the_pad_brings_the_selection_back() {
        let (mut m, mut s, info) = setup(Platform::Desktop);
        m.scroll = 40;
        m.drag(40, -30);
        assert_eq!(m.scroll, 70);
        assert!(!m.follow);
        go(&mut m, &mut s, &info, &[Role::Confirm, Role::Down]);
        assert!(m.follow);
    }

    #[test]
    fn up_and_down_past_the_ends_scroll_the_text() {
        // Credits ends in paragraphs no row reaches.
        let (mut m, mut s, info) = setup(Platform::Desktop);
        m.tap_tab(tab_index(&m, Tab::Credits), &mut s, &info);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        let last = m.items(&s, &info).len() - 1;
        let downs = vec![Role::Down; last];
        go(&mut m, &mut s, &info, &downs);
        assert_eq!((m.row, m.nudge), (last, 0));
        go(&mut m, &mut s, &info, &[Role::Down, Role::Down]);
        assert_eq!(m.row, last);
        assert_eq!(m.nudge, 2 * SCROLL_STEP);
        assert!(!m.follow);
        // Up moves the row again, and the view follows it.
        go(&mut m, &mut s, &info, &[Role::Up]);
        assert_eq!(m.row, last - 1);
        assert!(m.follow);
    }

    #[test]
    fn the_tip_jar_opens_the_page() {
        // Desktop: the browser opens over the running game.
        let (mut m, mut s, info) = setup(Platform::Desktop);
        select(&mut m, &s, &info, Tab::Credits, RowId::TipJar);
        assert_eq!(go(&mut m, &mut s, &info, &[Role::Confirm]), vec![Command::OpenTipPage]);
        // Android: the game has to close first, so it asks.
        let (mut m, mut s, info) = setup(Platform::Android);
        select(&mut m, &s, &info, Tab::Credits, RowId::TipJar);
        go(&mut m, &mut s, &info, &[Role::Confirm]);
        assert_eq!(m.mode, Mode::Dialog { change: Change::TipPage, yes: false });
        let out = go(&mut m, &mut s, &info, &[Role::NextSection, Role::Confirm]);
        assert_eq!(out, vec![Command::Apply(Change::TipPage)]);
        let (_, body, _) = dialog_text(&Change::TipPage, Platform::Android, &info);
        assert!(body.contains(credits::TIP_URL));
    }

    fn item_index(m: &Menu, s: &Settings, info: &Info, id: RowId) -> usize {
        m.items(s, info).iter().position(|&r| r == id).unwrap()
    }

    #[test]
    fn restart_rows_are_marked() {
        let (_, s, info) = setup(Platform::Android);
        let marked: Vec<RowId> = rows(Tab::Music, Platform::Android, &s, &info)
            .into_iter()
            .filter_map(|r| match r {
                Row::Item { id, restarts: true, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(
            marked,
            [
                RowId::MusicFolder(0),
                RowId::MusicFolder(1),
                RowId::AddMusicFolder,
                RowId::Rescan
            ]
        );
    }

    #[test]
    fn android_shows_its_save_folder_and_the_desktop_doesnt() {
        let (_, s, info) = setup(Platform::Android);
        let row = rows(Tab::Data, Platform::Android, &s, &info)
            .into_iter()
            .find_map(|r| match r {
                Row::Item {
                    id: RowId::SaveFolder,
                    label,
                    restarts,
                    ..
                } => Some((label, restarts)),
                _ => None,
            });
        assert_eq!(
            row,
            Some(("Internal storage › Documents › SongSummoner".to_string(), true))
        );
        assert!(!rows(Tab::Data, Platform::Desktop, &s, &info)
            .iter()
            .any(|r| matches!(r, Row::Item { id: RowId::SaveFolder, .. })));
    }

    #[test]
    fn changing_the_save_folder_says_what_happens_to_saves() {
        let (_, _, info) = setup(Platform::Android);
        let (title, body, yes) = dialog_text(&Change::SaveFolder, Platform::Android, &info);
        assert_eq!(title, "Change the save folder?");
        assert!(body.contains("already has saves"));
        assert!(body.contains("closes now"));
        assert_eq!(yes, "Close and pick folder");
    }

    #[test]
    fn a_first_menu_with_no_music_opens_on_the_music_tab() {
        let mut m = Menu::new(Platform::Android, ConfirmButton::South);
        m.start_on_music(true);
        assert_eq!(m.current_tab(), Tab::Music);
        let mut m = Menu::new(Platform::Android, ConfirmButton::South);
        m.start_on_music(false);
        assert_eq!(m.current_tab(), Tab::Game);
    }

    #[test]
    fn dialog_texts_warn_about_the_restart() {
        let (_, _, info) = setup(Platform::Desktop);
        let (title, body, yes) = dialog_text(&Change::RemoveFolder(1), Platform::Desktop, &info);
        assert_eq!(title, "Remove this music folder?");
        assert!(body.contains("412 songs"));
        assert!(body.contains("last in-game save"));
        assert_eq!(yes, "Remove and restart");
        let (_, body, yes) = dialog_text(&Change::GameFile(None), Platform::Android, &info);
        assert!(body.contains("closes now"));
        assert_eq!(yes, "Close and pick file");
    }

    // On Windows only the first folder's songs keep folder-relative IDs
    // (scan_windows::place), so removing it renames every other song.
    #[test]
    fn removing_the_first_of_several_folders_warns_about_troopers() {
        let (_, _, info) = setup(Platform::Desktop);
        assert!(info.folders.len() > 1);
        let (_, body, _) = dialog_text(&Change::RemoveFolder(0), Platform::Desktop, &info);
        assert!(body.contains("troopers"));
        let (_, body, _) = dialog_text(&Change::RemoveFolder(1), Platform::Desktop, &info);
        assert!(!body.contains("troopers"));
        let (_, body, _) = dialog_text(&Change::RemoveFolder(0), Platform::Android, &info);
        assert!(!body.contains("troopers"));
    }

    #[test]
    fn credits_tab_carries_the_notices() {
        let (_, s, info) = setup(Platform::Desktop);
        let all: String = rows(Tab::Credits, Platform::Desktop, &s, &info)
            .into_iter()
            .map(|r| match r {
                Row::Heading(t) | Row::Paragraph(t) => t,
                Row::Item { label, sub, .. } => format!("{label} {}", sub.unwrap_or_default()),
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all.contains(credits::ZACKSLY_CREDIT));
        assert!(all.contains(credits::TOUCHHLE_COPYRIGHT));
        assert!(all.contains(credits::SQUARE_ENIX_COPYRIGHT));
        assert!(all.contains(credits::NOT_AFFILIATED));
        assert!(all.contains(credits::SUPPORT_SQUARE_ENIX));
        assert!(all.contains(credits::SOURCE_URL));
    }

    #[test]
    fn thousands() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(1284), "1,284");
        assert_eq!(with_thousands(1234567), "1,234,567");
    }
}
