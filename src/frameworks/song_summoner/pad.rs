/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! What the controller buttons mean in the picker, and hold-to-repeat.
//!
//! Buttons arrive by position ([PadButton]: SDL's button labels are turned
//! off in `window.rs`, so the bottom face button is `FaceSouth` on every
//! pad). Which of south/east confirms is the `--confirm-button=` option.

use super::glyphs::Glyph;
use crate::options::ConfirmButton;
use crate::window::PadButton;

/// A picker command.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
}

pub fn role(button: PadButton, confirm: ConfirmButton) -> Option<Role> {
    let (confirm_button, back_button) = face_buttons(confirm);
    Some(match button {
        b if b == confirm_button => Role::Confirm,
        b if b == back_button => Role::Back,
        PadButton::DPadUp => Role::Up,
        PadButton::DPadDown => Role::Down,
        PadButton::DPadLeft => Role::PrevSection,
        PadButton::DPadRight => Role::NextSection,
        PadButton::LeftShoulder => Role::PrevTab,
        PadButton::RightShoulder => Role::NextTab,
        PadButton::FaceNorth => Role::Info,
        _ => return None,
    })
}

/// The confirm and back buttons.
fn face_buttons(confirm: ConfirmButton) -> (PadButton, PadButton) {
    match confirm {
        ConfirmButton::South => (PadButton::FaceSouth, PadButton::FaceEast),
        ConfirmButton::East => (PadButton::FaceEast, PadButton::FaceSouth),
    }
}

/// The icon to show for the back button.
pub fn back_glyph(confirm: ConfirmButton) -> Glyph {
    match face_buttons(confirm).1 {
        PadButton::FaceSouth => Glyph::FaceSouth,
        _ => Glyph::FaceEast,
    }
}

/// Holding a direction repeats it, like a keyboard.
pub fn repeats(role: Role) -> bool {
    matches!(
        role,
        Role::Up | Role::Down | Role::PrevSection | Role::NextSection
    )
}

/// Seconds a direction must be held before it starts repeating.
pub const REPEAT_DELAY: f32 = 0.35;
/// Seconds between repeats after that.
pub const REPEAT_INTERVAL: f32 = 0.07;

/// How many repeats (not counting the press itself) are due after holding
/// for `held_for` seconds.
pub fn repeats_due(held_for: f32) -> u32 {
    if held_for < REPEAT_DELAY {
        0
    } else {
        1 + ((held_for - REPEAT_DELAY) / REPEAT_INTERVAL) as u32
    }
}

/// The tab `step` places after `tab` (of 4), wrapping round.
pub fn tab_after(tab: usize, step: i32) -> usize {
    (tab as i32 + step).rem_euclid(4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for button in [PadButton::FaceWest, PadButton::Start, PadButton::Back] {
            assert_eq!(role(button, ConfirmButton::South), None, "{button:?}");
        }
    }

    #[test]
    fn north_is_info_whichever_button_confirms() {
        assert_eq!(role(PadButton::FaceNorth, ConfirmButton::South), Some(Role::Info));
        assert_eq!(role(PadButton::FaceNorth, ConfirmButton::East), Some(Role::Info));
        assert!(!repeats(Role::Info));
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
    fn tabs_wrap_round() {
        assert_eq!(tab_after(0, 1), 1);
        assert_eq!(tab_after(3, 1), 0);
        assert_eq!(tab_after(0, -1), 3);
        assert_eq!(tab_after(2, -1), 1);
    }
}
