/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The user's music library, host side.
//!
//! This module knows nothing about Objective-C: it reads the library index
//! that a scanner wrote, opens and decodes song files, and keeps play counts.
//! The guest-facing MediaPlayer classes (`frameworks::media_player`) and the
//! Song Summoner picker (`frameworks::song_summoner`) are built on top of it.
//!
//! Scanning happens outside the engine: in Kotlin before touchHLE starts on
//! Android (`LibraryScanner.kt`), and on a host thread at boot on the desktop
//! ([scan_windows]). Both write the same files to `<user data>/library/`:
//!
//! | File | Contents |
//! |---|---|
//! | `source.txt` | Android: the SAF tree URI. Desktop: the folder path. |
//! | `index.tsv` | The songs, see [index]. |
//! | `art/<id>.png` | Embedded cover art, longest side ≤ 256. |
//! | `playcounts.tsv` | `id  count`, see [playcount]. |

// On Android the library is scanned in Kotlin, so the index writer, PNG
// encoder and library publishing that only the desktop scanner uses are
// never called there.
#![cfg_attr(target_os = "android", allow(dead_code))]

pub mod artwork;
pub mod index;
pub mod library;
pub mod playback;
pub mod playcount;
#[cfg(not(target_os = "android"))]
pub mod scan_windows;
pub mod source;

use std::path::PathBuf;

/// `<user data>/library/`.
pub fn library_dir() -> PathBuf {
    crate::paths::user_data_base_path().join("library")
}
pub fn index_path() -> PathBuf {
    library_dir().join("index.tsv")
}
pub fn source_path() -> PathBuf {
    library_dir().join("source.txt")
}
pub fn art_dir() -> PathBuf {
    library_dir().join("art")
}
pub fn playcounts_path() -> PathBuf {
    library_dir().join("playcounts.tsv")
}

/// File extensions both scanners index. These are the formats touchHLE's
/// Symphonia build can decode (see `symphonia` in Cargo.toml); FLAC and Ogg
/// are left out because there's no decoder for them, and a song that shows
/// up in the picker but never plays is worse than one that isn't listed.
/// Keep in sync with `LibraryScanner.kt`.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "m4a", "aac", "wav", "caf"];
