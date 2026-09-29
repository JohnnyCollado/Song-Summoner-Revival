/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! What the controller buttons mean, the player's own mapping of them, the
//! Select + Start shortcut that opens the Setup menu, and hold-to-repeat.
//!
//! Buttons arrive by position ([PadButton]: SDL's button labels are turned
//! off in `window.rs`, so the bottom face button is `FaceSouth` on every
//! pad). Which of south/east confirms by default is the `--confirm-button=`
//! option; the Setup menu can then move any action to another button
//! ([Bindings]), saved in the settings file.

use super::glyphs::Glyph;
use crate::options::ConfirmButton;
use crate::window::PadButton;

/// A command, from a controller button or a key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Up,
    Down,
    /// Previous/next A–Z section, or a page in lists without sections.
    PrevSection,
    NextSection,
    PrevTab,
    NextTab,
    /// Pick the song, or open the artist/album/playlist.
    Confirm,
    /// ‹ Back out of an artist/album/playlist, or Cancel at the top level.
    Back,
    /// The north face button: a screen's extra panel, like the card
    /// list's status panel. The picker has none.
    Info,
    /// Start: the game's SKIP button, in cutscenes that allow it.
    Skip,
    /// The triggers (L2 / R2): zoom battle's map out and in. The picker has
    /// no use for them.
    ZoomOut,
    ZoomIn,
}

/// The actions the Setup menu lets the player move to another button, in
/// the menu's order. Directions stay on the D-pad (and the left stick,
/// which mirrors it).
pub const REMAPPABLE: [Role; 8] = [
    Role::Confirm,
    Role::Back,
    Role::Info,
    Role::PrevTab,
    Role::NextTab,
    Role::ZoomOut,
    Role::ZoomIn,
    Role::Skip,
];

/// Why [Bindings::bind] refused.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BindError {
    /// The D-pad and Select can't take an action: the D-pad always moves,
    /// and Select is kept for the Setup menu shortcut.
    ReservedButton,
    /// Directions can't be moved off the D-pad.
    FixedRole,
}

/// Which button does each of [REMAPPABLE].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    /// `buttons[i]` does `REMAPPABLE[i]`.
    buttons: [PadButton; REMAPPABLE.len()],
}

impl Bindings {
    /// The stock mapping, with the confirm and back buttons the
    /// `--confirm-button=` option asks for.
    pub fn defaults(confirm: ConfirmButton) -> Bindings {
        let (confirm_button, back_button) = face_buttons(confirm);
        Bindings {
            buttons: [
                confirm_button,
                back_button,
                PadButton::FaceNorth,
                PadButton::LeftShoulder,
                PadButton::RightShoulder,
                PadButton::LeftTrigger,
                PadButton::RightTrigger,
                PadButton::Start,
            ],
        }
    }

    /// What a button does, if anything.
    pub fn role(&self, button: PadButton) -> Option<Role> {
        match button {
            PadButton::DPadUp => return Some(Role::Up),
            PadButton::DPadDown => return Some(Role::Down),
            PadButton::DPadLeft => return Some(Role::PrevSection),
            PadButton::DPadRight => return Some(Role::NextSection),
            _ => (),
        }
        let i = self.buttons.iter().position(|&b| b == button)?;
        Some(REMAPPABLE[i])
    }

    /// The button that does `role`. Directions answer with their D-pad
    /// button.
    pub fn button(&self, role: Role) -> PadButton {
        match role {
            Role::Up => PadButton::DPadUp,
            Role::Down => PadButton::DPadDown,
            Role::PrevSection => PadButton::DPadLeft,
            Role::NextSection => PadButton::DPadRight,
            _ => {
                let i = REMAPPABLE.iter().position(|&r| r == role).unwrap();
                self.buttons[i]
            }
        }
    }

    /// Move `role` to `button`. If another action had that button, it
    /// takes `role`'s old one, so no action is ever left without a button.
    pub fn bind(&mut self, role: Role, button: PadButton) -> Result<(), BindError> {
        if !bindable(button) {
            return Err(BindError::ReservedButton);
        }
        let Some(i) = REMAPPABLE.iter().position(|&r| r == role) else {
            return Err(BindError::FixedRole);
        };
        let old = self.buttons[i];
        if let Some(j) = self.buttons.iter().position(|&b| b == button) {
            self.buttons[j] = old;
        }
        self.buttons[i] = button;
        Ok(())
    }

    /// `(key, value)` lines for the settings file, like
    /// `("pad.confirm", "face_south")`.
    pub fn to_settings(&self) -> Vec<(String, String)> {
        REMAPPABLE
            .iter()
            .zip(self.buttons.iter())
            .map(|(&role, &button)| {
                (
                    format!("pad.{}", role_name(role)),
                    button_name(button).to_string(),
                )
            })
            .collect()
    }

    /// Read the `pad.*` settings. With none saved, the `--confirm-button=`
    /// defaults apply. A bad value keeps that action's default; a mapping
    /// that ends up giving two actions one button is dropped entirely.
    /// Returns the bindings and a warning per problem.
    pub fn from_settings<'a>(
        get: impl Fn(&str) -> Option<&'a str>,
        confirm: ConfirmButton,
    ) -> (Bindings, Vec<String>) {
        let mut bindings = Bindings::defaults(confirm);
        let mut warnings = Vec::new();
        for (i, &role) in REMAPPABLE.iter().enumerate() {
            let key = format!("pad.{}", role_name(role));
            let Some(value) = get(&key) else {
                continue;
            };
            match button_from_name(value).filter(|&b| bindable(b)) {
                Some(button) => bindings.buttons[i] = button,
                None => warnings.push(format!("{key}: unknown button {value:?}")),
            }
        }
        let mut seen = Vec::new();
        if bindings.buttons.iter().any(|b| {
            let repeat = seen.contains(b);
            seen.push(*b);
            repeat
        }) {
            warnings.push("pad.*: two actions share a button, using the defaults".to_string());
            bindings = Bindings::defaults(confirm);
        }
        (bindings, warnings)
    }
}

/// Buttons an action can be moved to.
pub fn bindable(button: PadButton) -> bool {
    !matches!(
        button,
        PadButton::DPadUp
            | PadButton::DPadDown
            | PadButton::DPadLeft
            | PadButton::DPadRight
            | PadButton::Back
    )
}

/// The confirm and back buttons.
fn face_buttons(confirm: ConfirmButton) -> (PadButton, PadButton) {
    match confirm {
        ConfirmButton::South => (PadButton::FaceSouth, PadButton::FaceEast),
        ConfirmButton::East => (PadButton::FaceEast, PadButton::FaceSouth),
    }
}

/// The icon for a button.
pub fn glyph_of(button: PadButton) -> Glyph {
    match button {
        PadButton::FaceSouth => Glyph::FaceSouth,
        PadButton::FaceEast => Glyph::FaceEast,
        PadButton::FaceWest => Glyph::FaceWest,
        PadButton::FaceNorth => Glyph::FaceNorth,
        PadButton::DPadUp => Glyph::DPadUp,
        PadButton::DPadDown => Glyph::DPadDown,
        PadButton::DPadLeft => Glyph::DPadLeft,
        PadButton::DPadRight => Glyph::DPadRight,
        PadButton::LeftShoulder => Glyph::BumperLeft,
        PadButton::RightShoulder => Glyph::BumperRight,
        PadButton::LeftTrigger => Glyph::TriggerLeft,
        PadButton::RightTrigger => Glyph::TriggerRight,
        PadButton::Start => Glyph::Start,
        PadButton::Back => Glyph::Select,
    }
}

/// Every button, for the tests and the settings parser.
pub const ALL_BUTTONS: [PadButton; 14] = [
    PadButton::FaceSouth,
    PadButton::FaceEast,
    PadButton::FaceWest,
    PadButton::FaceNorth,
    PadButton::DPadUp,
    PadButton::DPadDown,
    PadButton::DPadLeft,
    PadButton::DPadRight,
    PadButton::LeftShoulder,
    PadButton::RightShoulder,
    PadButton::LeftTrigger,
    PadButton::RightTrigger,
    PadButton::Start,
    PadButton::Back,
];

/// A button's name in the settings file. Named by position, like the icon
/// files, so it means the same button on every make of pad.
pub fn button_name(button: PadButton) -> &'static str {
    match button {
        PadButton::FaceSouth => "face_south",
        PadButton::FaceEast => "face_east",
        PadButton::FaceWest => "face_west",
        PadButton::FaceNorth => "face_north",
        PadButton::DPadUp => "dpad_up",
        PadButton::DPadDown => "dpad_down",
        PadButton::DPadLeft => "dpad_left",
        PadButton::DPadRight => "dpad_right",
        PadButton::LeftShoulder => "bumper_left",
        PadButton::RightShoulder => "bumper_right",
        PadButton::LeftTrigger => "trigger_left",
        PadButton::RightTrigger => "trigger_right",
        PadButton::Start => "start",
        PadButton::Back => "select",
    }
}

pub fn button_from_name(name: &str) -> Option<PadButton> {
    ALL_BUTTONS
        .iter()
        .copied()
        .find(|&b| button_name(b) == name.trim())
}

/// An action's name in the settings file.
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Up => "up",
        Role::Down => "down",
        Role::PrevSection => "left",
        Role::NextSection => "right",
        Role::PrevTab => "prev_tab",
        Role::NextTab => "next_tab",
        Role::Confirm => "confirm",
        Role::Back => "back",
        Role::Info => "info",
        Role::Skip => "skip",
        Role::ZoomOut => "zoom_out",
        Role::ZoomIn => "zoom_in",
    }
}

/// What the Select + Start shortcut made of a button event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChordOutcome {
    /// Not part of the shortcut: handle the button as usual.
    Pass,
    /// Open the Setup menu.
    OpenMenu,
    /// Part of the shortcut: nothing else should see it.
    Swallow,
}

/// Watches for Select + Start. Select has to go down first: Start on its
/// own already fires on press (it skips cutscenes), so a Start that came
/// first can't be taken back.
#[derive(Default, Debug)]
pub struct Chord {
    select_down: bool,
    /// Start went down as part of the shortcut, so its release is ours too.
    start_taken: bool,
}

impl Chord {
    pub fn feed(&mut self, button: PadButton, pressed: bool) -> ChordOutcome {
        match button {
            PadButton::Back => {
                self.select_down = pressed;
                ChordOutcome::Swallow
            }
            PadButton::Start if pressed && self.select_down => {
                self.start_taken = true;
                ChordOutcome::OpenMenu
            }
            PadButton::Start if !pressed && self.start_taken => {
                self.start_taken = false;
                ChordOutcome::Swallow
            }
            _ => ChordOutcome::Pass,
        }
    }
}

/// Holding a direction repeats it, like a keyboard.
pub fn repeats(role: Role) -> bool {
    matches!(
        role,
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection
    )
}

/// Seconds a direction must be held before it starts repeating, unless the
/// player changed it in the Setup menu.
pub const REPEAT_DELAY: f32 = 0.35;
/// Seconds between repeats after that.
pub const REPEAT_INTERVAL: f32 = 0.07;

/// How many repeats (not counting the press itself) are due after holding
/// for `held_for` seconds, at the stock speed.
#[cfg(test)]
pub fn repeats_due(held_for: f32) -> u32 {
    repeats_due_at(held_for, REPEAT_DELAY, REPEAT_INTERVAL)
}

/// [repeats_due] with a chosen delay and interval.
pub fn repeats_due_at(held_for: f32, delay: f32, interval: f32) -> u32 {
    if held_for < delay {
        0
    } else {
        1 + ((held_for - delay) / interval.max(0.01)) as u32
    }
}

/// The tab `step` places after `tab` (of `count`), wrapping round.
pub fn tab_after_of(tab: usize, step: i32, count: usize) -> usize {
    (tab as i32 + step).rem_euclid(count as i32) as usize
}

/// The picker's tab `step` places after `tab` (of 4), wrapping round.
pub fn tab_after(tab: usize, step: i32) -> usize {
    tab_after_of(tab, step, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock meaning of a button, as before buttons could be moved.
    fn role(button: PadButton, confirm: ConfirmButton) -> Option<Role> {
        Bindings::defaults(confirm).role(button)
    }

    /// The icon for the back button.
    fn back_glyph(confirm: ConfirmButton) -> Glyph {
        glyph_of(Bindings::defaults(confirm).button(Role::Back))
    }

    #[test]
    fn south_confirms_by_default() {
        let c = ConfirmButton::South;
        assert_eq!(role(PadButton::FaceSouth, c), Some(Role::Confirm));
        assert_eq!(role(PadButton::FaceEast, c), Some(Role::Back));
        assert_eq!(back_glyph(c), Glyph::FaceEast);
    }

    #[test]
    fn east_confirm_swaps_confirm_and_back() {
        let c = ConfirmButton::East;
        assert_eq!(role(PadButton::FaceEast, c), Some(Role::Confirm));
        assert_eq!(role(PadButton::FaceSouth, c), Some(Role::Back));
        assert_eq!(back_glyph(c), Glyph::FaceSouth);
    }

    #[test]
    fn directions_and_shoulders() {
        let c = ConfirmButton::South;
        assert_eq!(role(PadButton::DPadUp, c), Some(Role::Up));
        assert_eq!(role(PadButton::DPadDown, c), Some(Role::Down));
        assert_eq!(role(PadButton::DPadLeft, c), Some(Role::PrevSection));
        assert_eq!(role(PadButton::DPadRight, c), Some(Role::NextSection));
        assert_eq!(role(PadButton::LeftShoulder, c), Some(Role::PrevTab));
        assert_eq!(role(PadButton::RightShoulder, c), Some(Role::NextTab));
    }

    #[test]
    fn other_buttons_do_nothing() {
        for button in [PadButton::FaceWest, PadButton::Back] {
            assert_eq!(role(button, ConfirmButton::South), None, "{button:?}");
        }
    }

    #[test]
    fn start_skips_cutscenes() {
        assert_eq!(role(PadButton::Start, ConfirmButton::South), Some(Role::Skip));
        assert!(!repeats(Role::Skip));
    }

    #[test]
    fn north_is_info_whichever_button_confirms() {
        assert_eq!(role(PadButton::FaceNorth, ConfirmButton::South), Some(Role::Info));
        assert_eq!(role(PadButton::FaceNorth, ConfirmButton::East), Some(Role::Info));
        assert!(!repeats(Role::Info));
    }

    #[test]
    fn triggers_zoom_and_dont_repeat() {
        // L2 zooms battle's map out, R2 in; a held trigger is one step.
        for c in [ConfirmButton::South, ConfirmButton::East] {
            assert_eq!(role(PadButton::LeftTrigger, c), Some(Role::ZoomOut));
            assert_eq!(role(PadButton::RightTrigger, c), Some(Role::ZoomIn));
        }
        assert!(!repeats(Role::ZoomOut));
        assert!(!repeats(Role::ZoomIn));
    }

    #[test]
    fn only_directions_repeat() {
        assert!(repeats(Role::Up));
        assert!(repeats(Role::Down));
        assert!(repeats(Role::PrevSection));
        assert!(repeats(Role::NextSection));
        // Repeating these would pick a song or leave the picker by accident.
        assert!(!repeats(Role::Confirm));
        assert!(!repeats(Role::Back));
        assert!(!repeats(Role::PrevTab));
        assert!(!repeats(Role::NextTab));
    }

    #[test]
    fn repeat_waits_then_ticks() {
        assert_eq!(repeats_due(0.0), 0);
        assert_eq!(repeats_due(REPEAT_DELAY - 0.01), 0);
        assert_eq!(repeats_due(REPEAT_DELAY), 1);
        assert_eq!(repeats_due(REPEAT_DELAY + REPEAT_INTERVAL * 0.5), 1);
        assert_eq!(repeats_due(REPEAT_DELAY + REPEAT_INTERVAL * 1.01), 2);
        assert_eq!(repeats_due(REPEAT_DELAY + REPEAT_INTERVAL * 10.01), 11);
    }

    #[test]
    fn a_chosen_scroll_speed_changes_the_timing() {
        // Slower: half a second before repeating, then every 0.1 s.
        assert_eq!(repeats_due_at(0.45, 0.5, 0.1), 0);
        assert_eq!(repeats_due_at(0.5, 0.5, 0.1), 1);
        assert_eq!(repeats_due_at(0.71, 0.5, 0.1), 3);
    }

    #[test]
    fn tabs_wrap_round() {
        assert_eq!(tab_after(0, 1), 1);
        assert_eq!(tab_after(3, 1), 0);
        assert_eq!(tab_after(0, -1), 3);
        assert_eq!(tab_after(2, -1), 1);
        // Any number of tabs, like the Setup menu's.
        assert_eq!(tab_after_of(5, 1, 6), 0);
        assert_eq!(tab_after_of(0, -1, 6), 5);
    }

    // --- Remapping (Setup menu, T2) ---

    #[test]
    fn binding_a_free_button_moves_the_action() {
        let mut b = Bindings::defaults(ConfirmButton::South);
        assert_eq!(b.bind(Role::Info, PadButton::FaceWest), Ok(()));
        assert_eq!(b.role(PadButton::FaceWest), Some(Role::Info));
        // Its old button is free now.
        assert_eq!(b.role(PadButton::FaceNorth), None);
        assert_eq!(b.button(Role::Info), PadButton::FaceWest);
    }

    #[test]
    fn binding_a_used_button_swaps_the_two_actions() {
        let mut b = Bindings::defaults(ConfirmButton::South);
        assert_eq!(b.bind(Role::Confirm, PadButton::FaceEast), Ok(()));
        assert_eq!(b.role(PadButton::FaceEast), Some(Role::Confirm));
        assert_eq!(b.role(PadButton::FaceSouth), Some(Role::Back));
        // Every action still has exactly one button.
        for role in REMAPPABLE {
            let button = b.button(role);
            assert_eq!(b.role(button), Some(role), "{role:?}");
        }
    }

    #[test]
    fn the_d_pad_and_select_cant_be_bound() {
        let mut b = Bindings::defaults(ConfirmButton::South);
        let before = b.clone();
        for button in [
            PadButton::DPadUp,
            PadButton::DPadDown,
            PadButton::DPadLeft,
            PadButton::DPadRight,
            PadButton::Back,
        ] {
            assert_eq!(b.bind(Role::Confirm, button), Err(BindError::ReservedButton));
        }
        assert_eq!(b, before);
    }

    #[test]
    fn directions_stay_on_the_d_pad() {
        let mut b = Bindings::defaults(ConfirmButton::South);
        for role in [Role::Up, Role::Down, Role::PrevSection, Role::NextSection] {
            assert_eq!(b.bind(role, PadButton::FaceWest), Err(BindError::FixedRole));
        }
        assert_eq!(b.role(PadButton::DPadUp), Some(Role::Up));
    }

    #[test]
    fn bindings_round_trip_through_settings_lines() {
        let mut b = Bindings::defaults(ConfirmButton::South);
        b.bind(Role::Skip, PadButton::FaceWest).unwrap();
        b.bind(Role::Confirm, PadButton::FaceEast).unwrap();
        let lines = b.to_settings();
        assert!(lines.contains(&("pad.skip".to_string(), "face_west".to_string())));
        let get = |key: &str| {
            lines
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let (again, warnings) = Bindings::from_settings(get, ConfirmButton::South);
        assert_eq!(again, b);
        assert!(warnings.is_empty());
    }

    #[test]
    fn no_saved_bindings_means_the_confirm_button_option_decides() {
        let (b, warnings) = Bindings::from_settings(|_| None, ConfirmButton::East);
        assert_eq!(b, Bindings::defaults(ConfirmButton::East));
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_bad_button_name_keeps_that_actions_default() {
        let get = |key: &str| match key {
            "pad.info" => Some("banana"),
            "pad.skip" => Some("face_west"),
            _ => None,
        };
        let (b, warnings) = Bindings::from_settings(get, ConfirmButton::South);
        assert_eq!(b.button(Role::Info), PadButton::FaceNorth);
        assert_eq!(b.button(Role::Skip), PadButton::FaceWest);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_saved_mapping_with_a_shared_button_falls_back_to_defaults() {
        let get = |key: &str| match key {
            "pad.info" => Some("face_south"),
            _ => None,
        };
        let (b, warnings) = Bindings::from_settings(get, ConfirmButton::South);
        assert_eq!(b, Bindings::defaults(ConfirmButton::South));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn button_names_round_trip() {
        for button in ALL_BUTTONS {
            assert_eq!(button_from_name(button_name(button)), Some(button));
        }
        assert_eq!(button_from_name("nope"), None);
    }

    #[test]
    fn every_button_has_an_icon() {
        // The menu draws every button with the Zacksly icons, never text.
        let mut seen = Vec::new();
        for button in ALL_BUTTONS {
            let glyph = glyph_of(button);
            assert!(!seen.contains(&glyph), "{button:?} shares an icon");
            seen.push(glyph);
        }
    }

    // --- The Select + Start shortcut (T4) ---

    #[test]
    fn select_then_start_opens_the_menu_and_doesnt_skip() {
        let mut chord = Chord::default();
        assert_eq!(chord.feed(PadButton::Back, true), ChordOutcome::Swallow);
        assert_eq!(chord.feed(PadButton::Start, true), ChordOutcome::OpenMenu);
        // Its release doesn't reach the game either.
        assert_eq!(chord.feed(PadButton::Start, false), ChordOutcome::Swallow);
        assert_eq!(chord.feed(PadButton::Back, false), ChordOutcome::Swallow);
    }

    #[test]
    fn start_alone_still_skips() {
        let mut chord = Chord::default();
        assert_eq!(chord.feed(PadButton::Start, true), ChordOutcome::Pass);
        assert_eq!(chord.feed(PadButton::Start, false), ChordOutcome::Pass);
    }

    #[test]
    fn start_first_then_select_does_nothing_more() {
        let mut chord = Chord::default();
        assert_eq!(chord.feed(PadButton::Start, true), ChordOutcome::Pass);
        assert_eq!(chord.feed(PadButton::Back, true), ChordOutcome::Swallow);
        assert_eq!(chord.feed(PadButton::Start, false), ChordOutcome::Pass);
    }

    #[test]
    fn letting_go_of_select_before_start_still_swallows_starts_release() {
        let mut chord = Chord::default();
        chord.feed(PadButton::Back, true);
        chord.feed(PadButton::Start, true);
        chord.feed(PadButton::Back, false);
        assert_eq!(chord.feed(PadButton::Start, false), ChordOutcome::Swallow);
        // And the next Start on its own is a normal one again.
        assert_eq!(chord.feed(PadButton::Start, true), ChordOutcome::Pass);
    }

    #[test]
    fn other_buttons_pass_through_the_chord() {
        let mut chord = Chord::default();
        chord.feed(PadButton::Back, true);
        assert_eq!(chord.feed(PadButton::FaceSouth, true), ChordOutcome::Pass);
    }
}
