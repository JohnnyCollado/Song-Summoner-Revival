/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The library the engine is using right now.
//!
//! The desktop scanner runs on its own thread and swaps in a fresh library
//! when it's done, so readers take an [Arc] snapshot and never block on the
//! scan. The generation number lets the picker notice a swap and rebuild.

use super::index::{self, Library};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Shared {
    library: Option<Arc<Library>>,
    generation: u64,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    library: None,
    generation: 0,
});
static SCANNING: AtomicBool = AtomicBool::new(false);
static SCAN_PROGRESS: AtomicUsize = AtomicUsize::new(0);

/// The current library. The first call reads `index.tsv`; a missing or
/// corrupt index gives an empty library, which makes Song Summoner hide its
/// iPod option.
pub fn current() -> Arc<Library> {
    let mut shared = SHARED.lock().unwrap();
    if shared.library.is_none() {
        shared.library = Some(Arc::new(load_from_disk()));
    }
    shared.library.clone().unwrap()
}

/// Bumped every time [publish] swaps the library.
pub fn generation() -> u64 {
    SHARED.lock().unwrap().generation
}

pub fn publish(library: Library) {
    log!("media: library updated: {} songs", library.len());
    let mut shared = SHARED.lock().unwrap();
    shared.library = Some(Arc::new(library));
    shared.generation += 1;
}

pub fn load_from_disk() -> Library {
    let path = super::index_path();
    let start = Instant::now();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            log!("media: no library index at {} ({})", path.display(), e);
            return Library::default();
        }
    };
    match index::parse(&text) {
        Ok(parsed) => {
            let library = Library::new(parsed.songs);
            log!(
                "media: index loaded: {} songs, {} bad rows skipped, {} ms",
                library.len(),
                parsed.skipped,
                start.elapsed().as_millis()
            );
            library
        }
        Err(e) => {
            log!("media: ignoring unreadable index {}: {}", path.display(), e);
            Library::default()
        }
    }
}

/// Whether a background scan is running. The picker shows its loading
/// overlay meanwhile.
pub fn is_scanning() -> bool {
    SCANNING.load(Ordering::Relaxed)
}
pub fn set_scanning(scanning: bool) {
    SCANNING.store(scanning, Ordering::Relaxed);
    if scanning {
        SCAN_PROGRESS.store(0, Ordering::Relaxed);
    }
}
/// Songs read so far by the running scan.
pub fn scan_progress() -> usize {
    SCAN_PROGRESS.load(Ordering::Relaxed)
}
pub fn set_scan_progress(songs: usize) {
    SCAN_PROGRESS.store(songs, Ordering::Relaxed);
}
