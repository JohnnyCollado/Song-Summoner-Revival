/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Controller button icons for on-screen prompts ("(A) Choose  (B) Back").
//!
//! The art is Zacksly's button icon packs (CC BY 3.0, credited in
//! res/controller_glyphs/CREDITS.txt and the README), embedded in the binary
//! so the Windows build and the APK need no extra files. Unlike the game's
//! own art, it's ours to ship.
//!
//! Icons are looked up by physical *position*, never by printed letter: "B"
//! is the east button on an Xbox pad but the south one on a Switch pad, so
//! a letter can't say which icon to draw. [Family] picks whose art is used
//! for that position.

use crate::media::artwork::Bitmap;
use std::collections::HashMap;
use std::rc::Rc;

/// Whose button art to draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Xbox,
    PlayStation,
    Nintendo,
}

impl Family {
    #[allow(dead_code)] // For the tests.
    pub const ALL: [Family; 3] = [Family::Xbox, Family::PlayStation, Family::Nintendo];

    /// The family for an `SDL_GameControllerType` value. Pads SDL can't
    /// identify (and the Xbox-layout ones: Luna, Stadia, Shield, virtual)
    /// get Xbox art, since that's the layout SDL's own button names use.
    pub fn from_sdl_type(sdl_type: u32) -> Family {
        match sdl_type {
            // PS3, PS4, PS5
            3 | 4 | 7 => Family::PlayStation,
            // Switch Pro, Joy-Con (L), Joy-Con (R), Joy-Con pair
            5 | 11 | 12 | 13 => Family::Nintendo,
            _ => Family::Xbox,
        }
    }

    fn file_prefix(self) -> &'static str {
        match self {
            Family::Xbox => "xb",
            Family::PlayStation => "ps",
            Family::Nintendo => "ns",
        }
    }
}

/// A physical control on the pad. The picker only shows some of them so
/// far; the rest are for the in-game prompts to come.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    FaceSouth,
    FaceEast,
    FaceWest,
    FaceNorth,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    /// The whole D-pad, for "move" prompts.
    DPadAll,
    BumperLeft,
    BumperRight,
    TriggerLeft,
    TriggerRight,
    Start,
    Select,
    StickLeft,
    StickRight,
}

impl Glyph {
    #[allow(dead_code)] // For the tests.
    pub const ALL: [Glyph; 17] = [
        Glyph::FaceSouth,
        Glyph::FaceEast,
        Glyph::FaceWest,
        Glyph::FaceNorth,
        Glyph::DPadUp,
        Glyph::DPadDown,
        Glyph::DPadLeft,
        Glyph::DPadRight,
        Glyph::DPadAll,
        Glyph::BumperLeft,
        Glyph::BumperRight,
        Glyph::TriggerLeft,
        Glyph::TriggerRight,
        Glyph::Start,
        Glyph::Select,
        Glyph::StickLeft,
        Glyph::StickRight,
    ];

    fn file_suffix(self) -> &'static str {
        match self {
            Glyph::FaceSouth => "face_south",
            Glyph::FaceEast => "face_east",
            Glyph::FaceWest => "face_west",
            Glyph::FaceNorth => "face_north",
            Glyph::DPadUp => "dpad_up",
            Glyph::DPadDown => "dpad_down",
            Glyph::DPadLeft => "dpad_left",
            Glyph::DPadRight => "dpad_right",
            Glyph::DPadAll => "dpad_all",
            Glyph::BumperLeft => "bumper_left",
            Glyph::BumperRight => "bumper_right",
            Glyph::TriggerLeft => "trigger_left",
            Glyph::TriggerRight => "trigger_right",
            Glyph::Start => "start",
            Glyph::Select => "select",
            Glyph::StickLeft => "stick_left",
            Glyph::StickRight => "stick_right",
        }
    }
}

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!(
            "../../../res/controller_glyphs/ctl_", $name, ".png"
        )))),*]
    };
}

/// Every icon, keyed by its file name without `ctl_` and `.png`.
const FILES: &[(&str, &[u8])] = embed![
    "xb_face_south", "xb_face_east", "xb_face_west", "xb_face_north",
    "xb_dpad_up", "xb_dpad_down", "xb_dpad_left", "xb_dpad_right", "xb_dpad_all",
    "xb_bumper_left", "xb_bumper_right", "xb_trigger_left", "xb_trigger_right",
    "xb_start", "xb_select", "xb_stick_left", "xb_stick_right",
    "ps_face_south", "ps_face_east", "ps_face_west", "ps_face_north",
    "ps_dpad_up", "ps_dpad_down", "ps_dpad_left", "ps_dpad_right", "ps_dpad_all",
    "ps_bumper_left", "ps_bumper_right", "ps_trigger_left", "ps_trigger_right",
    "ps_start", "ps_select", "ps_stick_left", "ps_stick_right",
    "ns_face_south", "ns_face_east", "ns_face_west", "ns_face_north",
    "ns_dpad_up", "ns_dpad_down", "ns_dpad_left", "ns_dpad_right", "ns_dpad_all",
    "ns_bumper_left", "ns_bumper_right", "ns_trigger_left", "ns_trigger_right",
    "ns_start", "ns_select", "ns_stick_left", "ns_stick_right",
];

/// The PNG for `family`'s art of `glyph`.
pub fn png(family: Family, glyph: Glyph) -> &'static [u8] {
    let name = format!("{}_{}", family.file_prefix(), glyph.file_suffix());
    FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, bytes)| *bytes)
        .unwrap_or_else(|| panic!("no controller glyph {name}"))
}

/// Decoded icons, shrunk to the sizes the prompts draw them at. The source
/// art is 128×128, far bigger than anything on a 480×320 screen, and
/// shrinking is too slow to repeat every frame.
#[derive(Default)]
pub struct GlyphCache {
    icons: HashMap<(Family, Glyph, u32), Rc<Bitmap>>,
}

impl GlyphCache {
    /// `family`'s `glyph`, `size`×`size` pixels.
    pub fn get(&mut self, family: Family, glyph: Glyph, size: u32) -> Rc<Bitmap> {
        self.icons
            .entry((family, glyph, size))
            .or_insert_with(|| {
                // The art is checked by the tests below, so a failure here
                // would mean a broken build, not a user's broken file.
                let full = Bitmap::decode(png(family, glyph)).expect("bad controller glyph");
                Rc::new(full.scaled(size, size))
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_has_every_glyph() {
        for family in Family::ALL {
            for glyph in Glyph::ALL {
                let bitmap = Bitmap::decode(png(family, glyph))
                    .unwrap_or_else(|| panic!("{family:?} {glyph:?} doesn't decode"));
                assert_eq!((bitmap.width, bitmap.height), (128, 128), "{family:?} {glyph:?}");
            }
        }
    }

    #[test]
    fn families_have_their_own_art() {
        // The point of the families: an Xbox player never sees a Switch
        // icon (their A/B and X/Y positions are swapped).
        for glyph in [Glyph::FaceSouth, Glyph::FaceEast, Glyph::Start] {
            assert_ne!(png(Family::Xbox, glyph), png(Family::Nintendo, glyph));
            assert_ne!(png(Family::Xbox, glyph), png(Family::PlayStation, glyph));
            assert_ne!(png(Family::PlayStation, glyph), png(Family::Nintendo, glyph));
        }
        // And within a family, positions differ.
        assert_ne!(png(Family::Xbox, Glyph::FaceSouth), png(Family::Xbox, Glyph::FaceEast));
    }

    #[test]
    fn family_from_sdl_controller_type() {
        // Values of SDL_GameControllerType.
        assert_eq!(Family::from_sdl_type(0), Family::Xbox); // unknown
        assert_eq!(Family::from_sdl_type(1), Family::Xbox); // Xbox 360
        assert_eq!(Family::from_sdl_type(2), Family::Xbox); // Xbox One
        assert_eq!(Family::from_sdl_type(3), Family::PlayStation);
        assert_eq!(Family::from_sdl_type(4), Family::PlayStation);
        assert_eq!(Family::from_sdl_type(5), Family::Nintendo); // Switch Pro
        assert_eq!(Family::from_sdl_type(6), Family::Xbox); // virtual
        assert_eq!(Family::from_sdl_type(7), Family::PlayStation); // PS5
        assert_eq!(Family::from_sdl_type(8), Family::Xbox); // Luna
        assert_eq!(Family::from_sdl_type(9), Family::Xbox); // Stadia
        assert_eq!(Family::from_sdl_type(10), Family::Xbox); // Shield
        for joycon in 11..=13 {
            assert_eq!(Family::from_sdl_type(joycon), Family::Nintendo);
        }
        // Types newer than this SDL knows about.
        assert_eq!(Family::from_sdl_type(99), Family::Xbox);
    }

    #[test]
    fn cache_shrinks_to_the_asked_size() {
        let mut cache = GlyphCache::default();
        let icon = cache.get(Family::Xbox, Glyph::FaceSouth, 18);
        assert_eq!((icon.width, icon.height), (18, 18));
        // Shrinking keeps the icon visible: some pixels are opaque-ish.
        assert!(icon.pixels.chunks(4).any(|p| p[3] > 128));
        // The same request is served from the cache.
        let again = cache.get(Family::Xbox, Glyph::FaceSouth, 18);
        assert!(Rc::ptr_eq(&icon, &again));
        // Another size is a separate entry.
        let bigger = cache.get(Family::Xbox, Glyph::FaceSouth, 24);
        assert_eq!(bigger.width, 24);
    }
}
