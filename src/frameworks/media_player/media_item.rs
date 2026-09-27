/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaItem` and `MPMediaItemArtwork`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::cg_image::{self, CGImageRelease};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string;
use crate::media::library;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;
use std::collections::HashMap;

// Property keys, with the values real iPhone OS uses.
pub const MPMediaItemPropertyPersistentID: &str = "persistentID";
pub const MPMediaItemPropertyMediaType: &str = "mediaType";
pub const MPMediaItemPropertyTitle: &str = "title";
pub const MPMediaItemPropertyAlbumTitle: &str = "albumTitle";
pub const MPMediaItemPropertyArtist: &str = "artist";
pub const MPMediaItemPropertyAlbumArtist: &str = "albumArtist";
pub const MPMediaItemPropertyGenre: &str = "genre";
pub const MPMediaItemPropertyPlaybackDuration: &str = "playbackDuration";
pub const MPMediaItemPropertyAlbumTrackNumber: &str = "albumTrackNumber";
pub const MPMediaItemPropertyArtwork: &str = "artwork";
pub const MPMediaItemPropertyPlayCount: &str = "playCount";
pub const MPMediaPlaylistPropertyPersistentID: &str = "playlistPersistentID";
pub const MPMediaPlaylistPropertyName: &str = "name";

pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMediaItemPropertyPersistentID",
        HostConstant::NSString(MPMediaItemPropertyPersistentID),
    ),
    (
        "_MPMediaItemPropertyMediaType",
        HostConstant::NSString(MPMediaItemPropertyMediaType),
    ),
    (
        "_MPMediaItemPropertyTitle",
        HostConstant::NSString(MPMediaItemPropertyTitle),
    ),
    (
        "_MPMediaItemPropertyAlbumTitle",
        HostConstant::NSString(MPMediaItemPropertyAlbumTitle),
    ),
    (
        "_MPMediaItemPropertyArtist",
        HostConstant::NSString(MPMediaItemPropertyArtist),
    ),
    (
        "_MPMediaItemPropertyAlbumArtist",
        HostConstant::NSString(MPMediaItemPropertyAlbumArtist),
    ),
    (
        "_MPMediaItemPropertyGenre",
        HostConstant::NSString(MPMediaItemPropertyGenre),
    ),
    (
        "_MPMediaItemPropertyPlaybackDuration",
        HostConstant::NSString(MPMediaItemPropertyPlaybackDuration),
    ),
    (
        "_MPMediaItemPropertyAlbumTrackNumber",
        HostConstant::NSString(MPMediaItemPropertyAlbumTrackNumber),
    ),
    (
        "_MPMediaItemPropertyArtwork",
        HostConstant::NSString(MPMediaItemPropertyArtwork),
    ),
    (
        "_MPMediaItemPropertyPlayCount",
        HostConstant::NSString(MPMediaItemPropertyPlayCount),
    ),
    (
        "_MPMediaPlaylistPropertyPersistentID",
        HostConstant::NSString(MPMediaPlaylistPropertyPersistentID),
    ),
    (
        "_MPMediaPlaylistPropertyName",
        HostConstant::NSString(MPMediaPlaylistPropertyName),
    ),
];

/// `MPMediaTypeMusic`.
const MEDIA_TYPE_MUSIC: u32 = 1;

#[derive(Default)]
pub(super) struct State {
    /// One `MPMediaItem` per persistent ID, made on first use and never
    /// freed. Song Summoner keeps pointers to items it never retained (the
    /// old implementation crashed on those once an autorelease pool drained
    /// them), and a real iPod library also hands out the same object for
    /// the same song. Each holds one retain from this map.
    items: HashMap<u64, id>,
}

struct MPMediaItemHostObject {
    persistent_id: u64,
    /// Property values handed out so far, each retained. The game stores
    /// some (e.g. titles for its panel) without retaining them, so they
    /// have to outlive the autorelease pool, like the item itself.
    values: HashMap<String, id>,
}
impl HostObject for MPMediaItemHostObject {}

struct MPMediaItemArtworkHostObject {
    persistent_id: u64,
}
impl HostObject for MPMediaItemArtworkHostObject {}

/// The `MPMediaItem` for a song. The caller doesn't own it (it lives
/// forever, see [State::items]).
pub fn item_for_id(env: &mut Environment, persistent_id: u64) -> id {
    if let Some(&item) = env
        .framework_state
        .media_player
        .media_item
        .items
        .get(&persistent_id)
    {
        return item;
    }
    let class = env.objc.get_known_class("MPMediaItem", &mut env.mem);
    let item = env.objc.alloc_object(
        class,
        Box::new(MPMediaItemHostObject {
            persistent_id,
            values: HashMap::new(),
        }),
        &mut env.mem,
    );
    env.framework_state
        .media_player
        .media_item
        .items
        .insert(persistent_id, item);
    item
}

/// The persistent ID of an `MPMediaItem`, or 0 if `item` isn't one of ours.
pub fn persistent_id_of(env: &mut Environment, item: id) -> u64 {
    if item == nil {
        return 0;
    }
    let class = env.objc.get_known_class("MPMediaItem", &mut env.mem);
    let item_class = msg![env; item class];
    if !env.objc.class_is_subclass_of(item_class, class) {
        return 0;
    }
    env.objc.borrow::<MPMediaItemHostObject>(item).persistent_id
}

/// A `UIImage` (autoreleased) of a song's cover art, or nil.
pub fn artwork_image(env: &mut Environment, persistent_id: u64) -> id {
    let Some(bytes) = crate::media::artwork::load_png_bytes(persistent_id) else {
        return nil;
    };
    let Ok(image) = crate::image::Image::from_bytes(&bytes) else {
        log!("media: unreadable art for {:016X}", persistent_id);
        return nil;
    };
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage alloc];
    let ui_image: id = msg![env; ui_image initWithCGImage:cg_image];
    CGImageRelease(env, cg_image);
    autorelease(env, ui_image)
}

fn number_u64(env: &mut Environment, value: u64) -> id {
    msg_class![env; NSNumber numberWithUnsignedLongLong:value]
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaItem: MPMediaEntity

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(MPMediaItemHostObject {
            persistent_id: 0,
            values: HashMap::new(),
        }),
        &mut env.mem,
    )
}

- (())dealloc {
    let values = std::mem::take(
        &mut env.objc.borrow_mut::<MPMediaItemHostObject>(this).values,
    );
    for (_, value) in values {
        release(env, value);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (u64)persistentID {
    env.objc.borrow::<MPMediaItemHostObject>(this).persistent_id
}

- (id)valueForProperty:(id)property { // NSString*
    if property == nil {
        return nil;
    }
    let key = ns_string::to_rust_string(env, property).to_string();
    let pid = env.objc.borrow::<MPMediaItemHostObject>(this).persistent_id;

    // Play counts change while the app runs, so they're never cached.
    if key == MPMediaItemPropertyPlayCount {
        let count = crate::media::playcount::get(pid);
        return msg_class![env; NSNumber numberWithUnsignedInt:count];
    }

    if let Some(&cached) = env
        .objc
        .borrow::<MPMediaItemHostObject>(this)
        .values
        .get(&key)
    {
        return cached;
    }

    let library = library::current();
    let Some(song) = library.get(pid) else {
        log!("media: item {:016X} is no longer in the library", pid);
        return nil;
    };
    let text = |env: &mut Environment, s: &str| {
        let string = ns_string::from_rust_string(env, s.to_string());
        autorelease(env, string)
    };
    let value: id = match key.as_str() {
        MPMediaItemPropertyPersistentID => number_u64(env, pid),
        MPMediaItemPropertyMediaType => {
            msg_class![env; NSNumber numberWithUnsignedInt:MEDIA_TYPE_MUSIC]
        }
        MPMediaItemPropertyTitle => text(env, &song.title),
        MPMediaItemPropertyArtist => text(env, &song.artist),
        MPMediaItemPropertyAlbumTitle => text(env, &song.album),
        MPMediaItemPropertyAlbumArtist => {
            let album_artist = if song.album_artist.is_empty() {
                &song.artist
            } else {
                &song.album_artist
            };
            text(env, album_artist)
        }
        MPMediaItemPropertyGenre => text(env, &song.genre),
        MPMediaItemPropertyPlaybackDuration => {
            let seconds = song.duration_secs();
            msg_class![env; NSNumber numberWithDouble:seconds]
        }
        MPMediaItemPropertyAlbumTrackNumber => {
            let track = song.track;
            msg_class![env; NSNumber numberWithUnsignedInt:track]
        }
        MPMediaItemPropertyArtwork => {
            if !song.has_art {
                return nil;
            }
            let artwork: id = msg_class![env; MPMediaItemArtwork alloc];
            env.objc
                .borrow_mut::<MPMediaItemArtworkHostObject>(artwork)
                .persistent_id = pid;
            autorelease(env, artwork)
        }
        _ => {
            log!("TODO: [(MPMediaItem*){:?} valueForProperty:{:?}] -> nil", this, key);
            nil
        }
    };
    if value != nil {
        retain(env, value);
        env.objc
            .borrow_mut::<MPMediaItemHostObject>(this)
            .values
            .insert(key, value);
    }
    value
}

@end

@implementation MPMediaItemArtwork: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(MPMediaItemArtworkHostObject { persistent_id: 0 }),
        &mut env.mem,
    )
}

// The stored art is at most 256x256; UIKit scales it to whatever size the
// app draws it at.
- (id)imageWithSize:(CGSize)_size {
    let pid = env.objc.borrow::<MPMediaItemArtworkHostObject>(this).persistent_id;
    artwork_image(env, pid)
}

- (CGRect)bounds {
    let pid = env.objc.borrow::<MPMediaItemArtworkHostObject>(this).persistent_id;
    let size = crate::media::artwork::load(pid)
        .map(|b| CGSize {
            width: b.width as f32,
            height: b.height as f32,
        })
        .unwrap_or(CGSize {
            width: 0.0,
            height: 0.0,
        });
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size,
    }
}

- (CGRect)imageCropRect {
    msg![env; this bounds]
}

@end

};
