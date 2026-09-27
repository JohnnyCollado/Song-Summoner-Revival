/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Song Summoner: The Unsung Heroes Encore.
//!
//! Not a system framework: this is the one place for code that only makes
//! sense for this game, so the generic parts of touchHLE stay app-agnostic.
//! It holds the replacement iPod music picker. Its host overrides
//! ([OVERRIDES]) are installed by `objc::app_overrides` only when the app
//! defines the game's classes, so other apps never see any of it.
//!
//! See dev-docs/song-summoner-media-plan.md (design) and
//! dev-docs/song-summoner-re.md (the game's side of the contract).

mod picker_art;
mod picker_hook;
mod picker_render;
mod picker_view;

use crate::abi::GuestFunction;
use crate::objc::{id, ClassExports};
use std::collections::HashMap;
use std::rc::Rc;

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
    /// Portrait per song, from the game's `IPD_ID2Fighter`. Cleared each
    /// time a picker opens, since a summon in between changes the answer.
    fighters: HashMap<u64, Option<u32>>,
    /// `IPD_ID2Fighter`, once looked up (`Some(None)` if the game lacks it).
    id_to_fighter: Option<Option<GuestFunction>>,
    next_list_uid: u64,
}
