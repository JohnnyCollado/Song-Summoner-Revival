/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Per-game input mapping (Phase 1 stub — hardcoded for now).
//!
//! Future iterations load this from `touchHLE_input_<bundle-id>.toml` and
//! support remapping every button + analog-stick mode + tile-snap config.
//! For Phase 1, every game gets the same defaults: left stick moves the
//! cursor, A is the tap/drag button.

use super::cursor::CursorAction;
use crate::options::Button;

#[derive(Clone, Debug)]
pub struct ControllerMapping {
    /// Whether the virtual cursor system is active at all. When false, the
    /// legacy `stick_to_touch` / `dpad_to_touch` / `button_to_touch` paths
    /// continue to run instead.
    pub enabled: bool,
    /// Which controller button triggers each cursor action.
    pub tap_button: Button,
}

impl ControllerMapping {
    /// Default mapping: cursor disabled. Phase 1 callers can opt in by
    /// constructing [Self::default_enabled] explicitly (or by setting an env
    /// var — see [Self::from_env]).
    pub fn default_disabled() -> Self {
        Self {
            enabled: false,
            tap_button: Button::A,
        }
    }

    pub fn default_enabled() -> Self {
        Self {
            enabled: true,
            tap_button: Button::A,
        }
    }

    /// Look at `TOUCHHLE_VCURSOR=1` to opt in without recompiling. Lets the
    /// Phase 1 feature ship behind a flag until the per-game TOML loader
    /// lands in Phase 5.
    pub fn from_env() -> Self {
        if std::env::var("TOUCHHLE_VCURSOR").ok().as_deref() == Some("1") {
            Self::default_enabled()
        } else {
            Self::default_disabled()
        }
    }

    /// Translate a physical controller button into a cursor action, if it
    /// maps to one. Returns None for unmapped buttons (caller should fall
    /// through to legacy handling).
    pub fn action_for(&self, button: Button) -> Option<CursorAction> {
        if !self.enabled {
            return None;
        }
        if button == self.tap_button {
            Some(CursorAction::Tap)
        } else {
            None
        }
    }
}
