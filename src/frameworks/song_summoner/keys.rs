/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Keyboard controls (Windows and other desktops, and a Bluetooth or USB
//! keyboard on Android).
//!
//! Keys are named by SDL scancode, i.e. by position on a US layout: "W" is
//! the key left of E wherever the player's layout puts the letter W, so
//! WASD stays a cluster on AZERTY too. Every action takes up to two keys,
//! and every action, directions included, can be moved in the Setup menu.
//! F2 (the menu), F11 (fullscreen) and F12 (debugger) are reserved.

use super::pad::{role_name, Role};

/// The actions keys can do, in the Setup menu's order.
pub const KEY_ACTIONS: [Role; 12] = [
    Role::Up,
    Role::Down,
    Role::PrevSection,
    Role::NextSection,
    Role::Confirm,
    Role::Back,
    Role::Info,
    Role::PrevTab,
    Role::NextTab,
    Role::ZoomOut,
    Role::ZoomIn,
    Role::Skip,
];

/// Opens the Setup menu. Fixed.
pub const MENU_KEY: &str = "F2";

/// Keys an action can't have.
pub fn reserved(key: &str) -> bool {
    matches!(key, "F2" | "F11" | "F12")
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyBindError {
    Reserved,
    /// Taking the key would leave another action with no key at all.
    WouldUnbind,
    /// Clearing the slot would leave this action with no key at all.
    LastKey,
}

/// Up to two keys per action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBindings {
    keys: [[Option<String>; 2]; KEY_ACTIONS.len()],
}

impl Default for KeyBindings {
    fn default() -> Self {
        let pair = |a: &str, b: Option<&str>| [Some(a.to_string()), b.map(str::to_string)];
        KeyBindings {
            keys: [
                pair("Up", Some("W")),
                pair("Down", Some("S")),
                pair("Left", Some("A")),
                pair("Right", Some("D")),
                pair("Return", Some("Space")),
                pair("Escape", Some("Backspace")),
                pair("I", Some("Tab")),
                pair("Q", None),
                pair("E", None),
                pair("Z", Some("-")),
                pair("C", Some("=")),
                pair("F", None),
            ],
        }
    }
}

fn index_of(role: Role) -> usize {
    KEY_ACTIONS.iter().position(|&r| r == role).unwrap()
}

impl KeyBindings {
    /// What a key does, if anything.
    pub fn role(&self, key: &str) -> Option<Role> {
        self.keys
            .iter()
            .position(|slots| slots.iter().any(|k| k.as_deref() == Some(key)))
            .map(|i| KEY_ACTIONS[i])
    }

    /// The keys for an action, first slot first.
    pub fn keys(&self, role: Role) -> [Option<&str>; 2] {
        let slots = &self.keys[index_of(role)];
        [slots[0].as_deref(), slots[1].as_deref()]
    }

    /// Put `key` in `role`'s slot `slot` (0 or 1). If another action had
    /// the key, it gets this slot's old key instead.
    pub fn bind(&mut self, role: Role, slot: usize, key: &str) -> Result<(), KeyBindError> {
        if reserved(key) {
            return Err(KeyBindError::Reserved);
        }
        let i = index_of(role);
        let old = self.keys[i][slot].clone();
        let holder = self.keys.iter().enumerate().find_map(|(j, slots)| {
            slots
                .iter()
                .position(|k| k.as_deref() == Some(key))
                .map(|s| (j, s))
        });
        if let Some((j, s)) = holder {
            if (j, s) == (i, slot) {
                return Ok(());
            }
            if j != i && old.is_none() && self.keys[j][1 - s].is_none() {
                return Err(KeyBindError::WouldUnbind);
            }
            self.keys[j][s] = old;
        }
        self.keys[i][slot] = Some(key.to_string());
        self.tidy(i);
        if let Some((j, _)) = holder {
            self.tidy(j);
        }
        Ok(())
    }

    /// Empty `role`'s slot, as long as it keeps another key.
    pub fn clear(&mut self, role: Role, slot: usize) -> Result<(), KeyBindError> {
        let i = index_of(role);
        if self.keys[i][1 - slot].is_none() {
            return Err(KeyBindError::LastKey);
        }
        self.keys[i][slot] = None;
        self.tidy(i);
        Ok(())
    }

    /// Keep a lone key in the first slot.
    fn tidy(&mut self, i: usize) {
        if self.keys[i][0].is_none() {
            self.keys[i].swap(0, 1);
        }
    }

    /// `key.<action>.1` / `.2` lines for the settings file.
    pub fn to_settings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (i, &role) in KEY_ACTIONS.iter().enumerate() {
            for slot in 0..2 {
                out.push((
                    format!("key.{}.{}", role_name(role), slot + 1),
                    self.keys[i][slot].clone().unwrap_or_default(),
                ));
            }
        }
        out
    }

    /// Read the `key.*` settings. Missing actions keep their defaults; a
    /// file that gives one key to two actions, or leaves an action with no
    /// key, falls back to the defaults entirely.
    pub fn from_settings<'a>(get: impl Fn(&str) -> Option<&'a str>) -> (KeyBindings, Vec<String>) {
        let mut out = KeyBindings::default();
        let mut warnings = Vec::new();
        for (i, &role) in KEY_ACTIONS.iter().enumerate() {
            let first = get(&format!("key.{}.1", role_name(role)));
            let second = get(&format!("key.{}.2", role_name(role)));
            if first.is_none() && second.is_none() {
                continue;
            }
            let clean = |k: Option<&str>| {
                k.map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_string)
            };
            let slots = [clean(first), clean(second)];
            if slots.iter().flatten().any(|k| reserved(k)) {
                warnings.push(format!("key.{}: F2, F11 and F12 are reserved", role_name(role)));
                continue;
            }
            out.keys[i] = slots;
            out.tidy(i);
        }
        let mut seen: Vec<&str> = Vec::new();
        let mut bad = out.keys.iter().any(|slots| slots[0].is_none());
        for key in out.keys.iter().flatten().flatten() {
            bad |= seen.contains(&key.as_str());
            seen.push(key);
        }
        if bad {
            warnings.push("key.*: a key is used twice or an action has none, using the defaults".to_string());
            out = KeyBindings::default();
        }
        (out, warnings)
    }
}

/// How a key is labelled on screen.
pub fn key_label(key: &str) -> String {
    match key {
        "Return" => "Enter".to_string(),
        "Escape" => "Esc".to_string(),
        "Backspace" => "Bksp".to_string(),
        "Up" => "↑".to_string(),
        "Down" => "↓".to_string(),
        "Left" => "←".to_string(),
        "Right" => "→".to_string(),
        "Left Shift" => "L Shift".to_string(),
        "Right Shift" => "R Shift".to_string(),
        "Left Ctrl" => "L Ctrl".to_string(),
        "Right Ctrl" => "R Ctrl".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let k = KeyBindings::default();
        assert_eq!(k.role("Up"), Some(Role::Up));
        assert_eq!(k.role("W"), Some(Role::Up));
        assert_eq!(k.role("A"), Some(Role::PrevSection));
        assert_eq!(k.role("D"), Some(Role::NextSection));
        assert_eq!(k.role("Return"), Some(Role::Confirm));
        assert_eq!(k.role("Space"), Some(Role::Confirm));
        assert_eq!(k.role("Escape"), Some(Role::Back));
        assert_eq!(k.role("Backspace"), Some(Role::Back));
        assert_eq!(k.role("Tab"), Some(Role::Info));
        assert_eq!(k.role("Q"), Some(Role::PrevTab));
        assert_eq!(k.role("E"), Some(Role::NextTab));
        assert_eq!(k.role("Z"), Some(Role::ZoomOut));
        assert_eq!(k.role("C"), Some(Role::ZoomIn));
        assert_eq!(k.role("F"), Some(Role::Skip));
        assert_eq!(k.role("F2"), None);
        assert_eq!(k.role("P"), None);
    }

    #[test]
    fn every_action_has_a_key_by_default_and_none_repeat() {
        let k = KeyBindings::default();
        let mut seen = Vec::new();
        for role in KEY_ACTIONS {
            let keys = k.keys(role);
            assert!(keys[0].is_some(), "{role:?}");
            for key in keys.into_iter().flatten() {
                assert!(!seen.contains(&key), "{key} twice");
                seen.push(key);
            }
        }
    }

    #[test]
    fn a_used_key_swaps() {
        let mut k = KeyBindings::default();
        // Put Q (previous tab) on Confirm's first slot: previous tab gets
        // Return instead.
        assert_eq!(k.bind(Role::Confirm, 0, "Q"), Ok(()));
        assert_eq!(k.role("Q"), Some(Role::Confirm));
        assert_eq!(k.role("Return"), Some(Role::PrevTab));
        assert_eq!(k.role("Space"), Some(Role::Confirm));
    }

    #[test]
    fn taking_an_actions_only_key_into_an_empty_slot_is_refused() {
        let mut k = KeyBindings::default();
        // Previous tab has only Q; Skip's second slot is empty.
        assert_eq!(k.bind(Role::Skip, 1, "Q"), Err(KeyBindError::WouldUnbind));
        assert_eq!(k.role("Q"), Some(Role::PrevTab));
    }

    #[test]
    fn a_second_key_can_be_added_and_cleared() {
        let mut k = KeyBindings::default();
        assert_eq!(k.bind(Role::Skip, 1, "K"), Ok(()));
        assert_eq!(k.keys(Role::Skip), [Some("F"), Some("K")]);
        assert_eq!(k.clear(Role::Skip, 0), Ok(()));
        // The lone key moves to the first slot.
        assert_eq!(k.keys(Role::Skip), [Some("K"), None]);
        assert_eq!(k.clear(Role::Skip, 0), Err(KeyBindError::LastKey));
    }

    #[test]
    fn reserved_keys_cant_be_bound() {
        let mut k = KeyBindings::default();
        for key in ["F2", "F11", "F12"] {
            assert_eq!(k.bind(Role::Confirm, 0, key), Err(KeyBindError::Reserved));
        }
        assert_eq!(k, KeyBindings::default());
    }

    #[test]
    fn round_trip_through_settings_lines() {
        let mut k = KeyBindings::default();
        k.bind(Role::Skip, 1, "K").unwrap();
        k.bind(Role::Up, 0, "I").unwrap();
        let lines = k.to_settings();
        let get = |key: &str| {
            lines
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let (again, warnings) = KeyBindings::from_settings(get);
        assert_eq!(again, k);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        // Two actions on the same key.
        let get = |key: &str| match key {
            "key.up.1" => Some("Return"),
            _ => None,
        };
        let (k, warnings) = KeyBindings::from_settings(get);
        assert_eq!(k, KeyBindings::default());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn labels_are_short() {
        assert_eq!(key_label("Return"), "Enter");
        assert_eq!(key_label("Escape"), "Esc");
        assert_eq!(key_label("Up"), "↑");
        assert_eq!(key_label("W"), "W");
    }
}
