/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The game's own picker art, read from the user's copy of the app at run
//! time. None of it is stored in the repository or bundled with touchHLE.
//!
//! Every image is optional: another version of the game might lack one, and
//! the picker then draws flat colours in the same layout instead.

use crate::media::artwork::Bitmap;
use crate::Environment;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// `list_fighter%03d.png` exist for 0..FIGHTER_COUNT.
pub const FIGHTER_COUNT: u32 = 90;

pub struct Art {
    /// Row background, and the blue-glow version for a touched row.
    pub cellbg: Option<Bitmap>,
    pub touch_bg: Option<Bitmap>,
    /// 50×50 placeholder for songs without cover art.
    pub noartwork: Option<Bitmap>,
    /// 40×40 tab icons: song, artist, album, playlist.
    pub tab_icons: [Option<Bitmap>; 4],
    /// The spinning cube of the loading screen, 48×48 per frame.
    pub cube: Vec<Bitmap>,
    /// Fighter portraits, loaded when first shown.
    fighters: RefCell<HashMap<u32, Option<Rc<Bitmap>>>>,
    bundle_files: HashMap<String, Vec<u8>>,
}

fn read_bundle_file(env: &mut Environment, name: &str) -> Option<Vec<u8>> {
    let path = env.bundle.bundle_path().join(name);
    env.fs.read(&path).ok()
}

fn load_image(env: &mut Environment, name: &str) -> Option<Bitmap> {
    let bitmap = read_bundle_file(env, name).and_then(|bytes| Bitmap::decode(&bytes));
    if bitmap.is_none() {
        log!("picker: {} isn't in this copy of the game, using flat colours", name);
    }
    bitmap
}

impl Art {
    pub fn load(env: &mut Environment) -> Art {
        let mut cube = Vec::new();
        for frame in 0.. {
            let name = format!("cube_anm_{frame:02}.png");
            let Some(bytes) = read_bundle_file(env, &name) else {
                break;
            };
            if let Some(bitmap) = Bitmap::decode(&bytes) {
                cube.push(bitmap);
            }
        }
        // Portraits are decoded lazily, but reading 90 files from the IPA
        // is quick, and it keeps the environment out of the draw path.
        let mut bundle_files = HashMap::new();
        for n in 0..FIGHTER_COUNT {
            let name = format!("list_fighter{n:03}.png");
            if let Some(bytes) = read_bundle_file(env, &name) {
                bundle_files.insert(name, bytes);
            }
        }
        Art {
            cellbg: load_image(env, "cellbg.png"),
            touch_bg: load_image(env, "touchBG.png"),
            noartwork: load_image(env, "noartwork.png"),
            tab_icons: [
                load_image(env, "song.png"),
                load_image(env, "artist.png"),
                load_image(env, "album.png"),
                load_image(env, "playlist.png"),
            ],
            cube,
            fighters: RefCell::new(HashMap::new()),
            bundle_files,
        }
    }

    /// Portrait `n`, if the game has it.
    pub fn fighter(&self, n: u32) -> Option<Rc<Bitmap>> {
        if let Some(cached) = self.fighters.borrow().get(&n) {
            return cached.clone();
        }
        let bitmap = self
            .bundle_files
            .get(&format!("list_fighter{n:03}.png"))
            .and_then(|bytes| Bitmap::decode(bytes))
            .map(Rc::new);
        self.fighters.borrow_mut().insert(n, bitmap.clone());
        bitmap
    }
}
