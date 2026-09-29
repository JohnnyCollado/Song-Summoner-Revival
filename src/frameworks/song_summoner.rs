/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Song Summoner: The Unsung Heroes Encore.
//!
//! Not a system framework: this is the one place for code that only makes
//! sense for this game, so the generic parts of touchHLE stay app-agnostic.
//! It holds the replacement iPod music picker, which also works with a game
//! controller, and controller input for the game itself. Their host
//! overrides ([OVERRIDES], [INPUT_OVERRIDES]) are installed by
//! `objc::app_overrides` only when the app defines the game's classes, so
//! other apps never see any of it.
//!
//! It also holds the Setup menu (settings, remapping, music folders,
//! credits), which pauses the game while it's open (`setup.rs`).
//!
//! See dev-docs/song-summoner-media-plan.md (design) and
//! dev-docs/song-summoner-re.md (the game's side of the contract).

mod battle;
mod credits;
mod game_input;
mod glyphs;
mod keys;
mod mirror;
mod pad;
mod picker_art;
mod picker_hook;
mod picker_render;
mod picker_view;
mod settings;
mod setup;
mod setup_menu;
mod setup_view;
mod support;

use crate::abi::GuestFunction;
use crate::objc::{id, ClassExports};
use std::collections::HashMap;
use std::rc::Rc;

pub use game_input::OVERRIDES as INPUT_OVERRIDES;
pub use picker_hook::OVERRIDES;

/// Host classes. Registered with UIKit, since the picker view is a `UIView`.
pub const CLASSES: ClassExports = picker_view::CLASSES;

#[derive(Default)]
pub struct State {
    /// The open picker, keyed by the game's `iPodView2` object.
    pickers: HashMap<id, picker_view::Picker>,
    /// The game's picker art, loaded from the app bundle on first use.
    art: Option<Rc<picker_art::Art>>,
    fonts: Option<Rc<picker_render::Fonts>>,
    /// Controller button icons for the picker's prompts.
    glyphs: glyphs::GlyphCache,
    /// Portrait per song, from the game's `IPD_ID2Fighter`. Cleared each
    /// time a picker opens, since a summon in between changes the answer.
    fighters: HashMap<u64, Option<u32>>,
    /// `IPD_ID2Fighter`, once looked up (`Some(None)` if the game lacks it).
    id_to_fighter: Option<Option<GuestFunction>>,
    next_list_uid: u64,
    game_input: game_input::State,
    setup: setup::State,
    setup_mirror: mirror::State,
}

/// The app is quitting: Android copies the last changed saves to the save
/// folder. Called for every app; does nothing for others.
pub fn before_exit(env: &mut crate::Environment) {
    setup::before_exit(env);
}

/// At startup, before the window: the Setup menu's settings that change
/// options (the stick's dead zone), and a save restore if one was asked
/// for. Only called for Song Summoner.
pub fn apply_at_startup(options: &mut crate::options::Options) {
    setup::apply_at_startup(options);
}

/// The Song Summoner IPA in the data folder, where the Setup menu puts it.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub fn managed_game_file() -> std::path::PathBuf {
    crate::paths::user_data_base_path().join(setup::IPA_NAME)
}

/// Whether a file looks like an iPhone app (a zip with `Payload/*.app`).
#[cfg_attr(target_os = "android", allow(dead_code))]
pub fn check_game_file(path: &std::path::Path) -> Result<(), String> {
    setup::check_game_file(path)
}

/// A controller button went down or up (from [crate::window::Event]). The
/// picker on screen gets it if there is one, otherwise the game.
pub fn handle_pad_button(
    env: &mut crate::Environment,
    button: crate::window::PadButton,
    pressed: bool,
    controller_type: u32,
) {
    // The Setup menu and its Select + Start shortcut come first.
    if setup::handle_pad_button(env, button, pressed, controller_type) {
        return;
    }
    let Some(role) = setup::role_for(env, button) else {
        return;
    };
    let family = setup::family_for(env, controller_type);
    handle_role(env, role, pressed, Some(family));
}

/// A key went down or up (desktop).
pub fn handle_key(env: &mut crate::Environment, key: &str, pressed: bool) {
    // The shop's password keyboard takes typing before the key mapping
    // (letters would otherwise be WASD and so on).
    if !setup::is_open(env) && game_input::handle_typing(env, key, pressed) {
        return;
    }
    if let Some(role) = setup::handle_key(env, key, pressed) {
        handle_role(env, role, pressed, None);
    }
}

/// Text input (Android's on-screen keyboard). Returns true if the shop's
/// password keyboard took it.
pub fn handle_text(env: &mut crate::Environment, event: &crate::window::TextInputEvent) -> bool {
    game_input::handle_text(env, event)
}

/// A command from a button or key, with the player's mapping applied: the
/// picker on screen gets it if there is one, otherwise the game. `family`
/// is the pad's, for its button icons (`None` for the keyboard).
fn handle_role(
    env: &mut crate::Environment,
    role: pad::Role,
    pressed: bool,
    family: Option<glyphs::Family>,
) {
    if !picker_view::handle_role(env, role, pressed, family) {
        game_input::handle_role(env, role, pressed);
    }
}

/// F2 or the Android gear button: open the Setup menu (or close it).
pub fn open_setup_menu(env: &mut crate::Environment) {
    setup::open_key(env);
}

/// A touch event. Returns true if the Setup menu took it.
pub fn handle_touch(env: &mut crate::Environment, event: &crate::window::Event) -> bool {
    setup::handle_touch(env, event)
}

/// A finger touched the screen (from [crate::window::Event]).
pub fn touch_used(env: &mut crate::Environment) {
    game_input::touch_used(env);
}
