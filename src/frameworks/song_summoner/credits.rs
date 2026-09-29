/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The credits and legal notice: one copy of the text, used by the Setup
//! menu's Credits tab and checked against `CREDITS.txt` and the README by
//! the tests below, so the three never drift apart.

/// touchHLE's own copyright line, as `--copyright` prints it.
pub const TOUCHHLE_COPYRIGHT: &str = "touchHLE © 2023–2026 touchHLE project contributors.";
pub const TOUCHHLE_LICENSE: &str = "GPL-3.0 (source files under MPL-2.0)";
pub const TOUCHHLE_URL: &str = "github.com/touchHLE/touchHLE";

/// This fork's source, which GPL-3.0 says we must point to.
pub const SOURCE_URL: &str = "github.com/JohnnyCollado/Song-Summoner-Revival";

/// The credit line Zacksly asks for (res/controller_glyphs/CREDITS.txt).
pub const ZACKSLY_CREDIT: &str =
    "Button Icons and Controls by Zacksly (CC BY 3.0 Licensed | zacksly.itch.io)";

/// The Android Setup button's gear (res/drawable/ic_setup_gear.xml).
pub const MATERIAL_CREDIT: &str =
    "Gear icon: \"Settings\" from Material Icons by Google (Apache License 2.0)";

pub const SQUARE_ENIX_COPYRIGHT: &str =
    "Song Summoner: The Unsung Heroes Encore © 2009 SQUARE ENIX CO., LTD. All rights reserved.";

pub const SQUARE_ENIX_RIGHTS: &str = "The game, its name, characters, story, music and \
    artwork are the property of Square Enix. SQUARE ENIX is a registered trademark of \
    Square Enix Holdings Co., Ltd.";

pub const NOT_AFFILIATED: &str = "Song Summoner Revival is an unofficial, non-commercial \
    fan project. It is not made, approved or endorsed by Square Enix. It contains no part \
    of the game: you must supply your own legally obtained copy.";

pub const SUPPORT_SQUARE_ENIX: &str = "If you enjoy Song Summoner, please support Square \
    Enix by buying their games through their official store and channels.";

/// The developer's tip jar (the user's page, 2026-09-28). Optional, and
/// said so: the game is free.
pub const TIP_TITLE: &str = "Buy me a Taco";
pub const TIP_URL: &str = "buymeacoffee.com/johnnycolli";
/// What the browser opens (`TipPage.URL` on Android is the same).
pub const TIP_LINK: &str = "https://www.buymeacoffee.com/johnnycolli";
pub const TIP_TEXT: &str = "Song Summoner Revival is free, and always will be. It's made by \
    one person in their spare time. If it brings you some joy and you'd like to support my \
    work as a solo developer, a tip is always appreciated but never required. Thank you \
    for playing!";

#[cfg(test)]
mod tests {
    use super::*;

    const CREDITS_TXT: &str = include_str!("../../../CREDITS.txt");
    const README: &str = include_str!("../../../README.md");
    const GLYPH_CREDITS: &str = include_str!("../../../res/controller_glyphs/CREDITS.txt");
    const LICENSES_RS: &str = include_str!("../../licenses.rs");

    /// Collapse runs of whitespace, so line wrapping in the text files
    /// doesn't matter.
    fn flat(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn zackslys_line_is_word_for_word() {
        assert!(flat(GLYPH_CREDITS).contains(ZACKSLY_CREDIT));
        assert!(flat(CREDITS_TXT).contains(ZACKSLY_CREDIT));
    }

    #[test]
    fn touchhles_copyright_matches_licenses_rs() {
        assert!(LICENSES_RS.contains(TOUCHHLE_COPYRIGHT));
        assert!(flat(CREDITS_TXT).contains(TOUCHHLE_COPYRIGHT));
    }

    #[test]
    fn the_square_enix_notice_is_everywhere() {
        for text in [flat(CREDITS_TXT), flat(README)] {
            assert!(text.contains(SQUARE_ENIX_COPYRIGHT));
            assert!(text.contains(&flat(SQUARE_ENIX_RIGHTS)));
            assert!(text.contains(&flat(NOT_AFFILIATED)));
            assert!(text.contains(&flat(SUPPORT_SQUARE_ENIX)));
        }
    }

    #[test]
    fn the_tip_jar_is_the_same_everywhere() {
        for text in [flat(CREDITS_TXT), flat(README)] {
            assert!(text.contains(TIP_URL));
            assert!(text.contains(&flat(TIP_TEXT)));
        }
        // It says plainly that tips are optional.
        assert!(TIP_TEXT.contains("never required"));
    }

    #[test]
    fn the_gear_icon_is_credited() {
        assert!(flat(CREDITS_TXT).contains(MATERIAL_CREDIT));
    }

    #[test]
    fn the_source_link_is_given() {
        assert!(CREDITS_TXT.contains(SOURCE_URL));
        assert!(CREDITS_TXT.contains(TOUCHHLE_URL));
    }
}
