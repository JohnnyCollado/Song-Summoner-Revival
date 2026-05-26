/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaItem` plus a small built-in "library" of fake songs.
//!
//! touchHLE has no real iPod music library, but apps like Song Summoner are
//! built entirely around iterating it. We expose a handful of synthetic songs
//! here so those apps see something to summon/charge with instead of an empty
//! library state.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::{ns_array, ns_string, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

// Property key strings. Apps pass these to `-[MPMediaItem valueForProperty:]`.
pub const MPMediaItemPropertyPersistentID: &str = "persistentID";
pub const MPMediaItemPropertyTitle: &str = "title";
pub const MPMediaItemPropertyArtist: &str = "artist";
pub const MPMediaItemPropertyAlbumTitle: &str = "albumTitle";
pub const MPMediaItemPropertyAlbumArtist: &str = "albumArtist";
pub const MPMediaItemPropertyGenre: &str = "genre";
pub const MPMediaItemPropertyPlaybackDuration: &str = "playbackDuration";
pub const MPMediaItemPropertyPlayCount: &str = "playCount";
pub const MPMediaItemPropertyArtwork: &str = "artwork";

pub const MPMediaPlaylistPropertyName: &str = "name";
pub const MPMediaPlaylistPropertyPersistentID: &str = "persistentID";

pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMediaItemPropertyPersistentID",
        HostConstant::NSString(MPMediaItemPropertyPersistentID),
    ),
    (
        "_MPMediaItemPropertyTitle",
        HostConstant::NSString(MPMediaItemPropertyTitle),
    ),
    (
        "_MPMediaItemPropertyArtist",
        HostConstant::NSString(MPMediaItemPropertyArtist),
    ),
    (
        "_MPMediaItemPropertyAlbumTitle",
        HostConstant::NSString(MPMediaItemPropertyAlbumTitle),
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
        "_MPMediaItemPropertyPlayCount",
        HostConstant::NSString(MPMediaItemPropertyPlayCount),
    ),
    (
        "_MPMediaItemPropertyArtwork",
        HostConstant::NSString(MPMediaItemPropertyArtwork),
    ),
    (
        "_MPMediaPlaylistPropertyName",
        HostConstant::NSString(MPMediaPlaylistPropertyName),
    ),
    (
        "_MPMediaPlaylistPropertyPersistentID",
        HostConstant::NSString(MPMediaPlaylistPropertyPersistentID),
    ),
];

use super::music_library;
use std::collections::HashMap;

struct MPMediaItemHostObject {
    song_index: usize,
    /// Property values cached on first read, each entry holds a retain. The
    /// game caches references to these (e.g. song titles in cell labels);
    /// without a long-lived retain on our side those strings get released
    /// when the autorelease pool drains and the game blows up later.
    cached_values: HashMap<String, id>,
}
impl HostObject for MPMediaItemHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaItem: MPMediaEntity

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(MPMediaItemHostObject {
            song_index: 0,
            cached_values: HashMap::new(),
        }),
        &mut env.mem,
    )
}

- (())dealloc {
    let cached = std::mem::take(
        &mut env.objc.borrow_mut::<MPMediaItemHostObject>(this).cached_values,
    );
    for (_, v) in cached {
        crate::objc::release(env, v);
    }
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)valueForProperty:(id)property {
    let key = ns_string::to_rust_string(env, property).to_string();
    let idx_for_log = env.objc.borrow::<MPMediaItemHostObject>(this).song_index;
    log!(
        "MPMediaItem {:?} (song_index={}) valueForProperty:'{}'",
        this, idx_for_log, key
    );
    // Return cached value (autoreleased copy of the retain we hold) if any.
    if let Some(&cached) = env
        .objc
        .borrow::<MPMediaItemHostObject>(this)
        .cached_values
        .get(&key)
    {
        if cached == nil { return nil; }
        crate::objc::retain(env, cached);
        return autorelease(env, cached);
    }
    let idx = env.objc.borrow::<MPMediaItemHostObject>(this).song_index;
    let Some(song) = music_library::song(idx) else {
        return nil;
    };
    // Build the value. NSNumber factories return an autoreleased object
    // (retain=1, pool entry pending). ns_string::from_rust_string returns
    // a non-autoreleased object (retain=1, no pool entry). Normalize so all
    // branches end with retain=1 + one pending autorelease — that way the
    // caching logic below can be symmetric.
    let value: id = match key.as_str() {
        MPMediaItemPropertyPersistentID => {
            msg_class![env; NSNumber numberWithUnsignedLongLong:(song.persistent_id)]
        }
        MPMediaItemPropertyTitle | "name" => {
            let v = ns_string::from_rust_string(env, song.title.clone());
            autorelease(env, v)
        }
        MPMediaItemPropertyArtist => {
            let v = ns_string::from_rust_string(env, song.artist.clone());
            autorelease(env, v)
        }
        MPMediaItemPropertyAlbumTitle => {
            let v = ns_string::from_rust_string(env, song.album.clone());
            autorelease(env, v)
        }
        MPMediaItemPropertyAlbumArtist => {
            let v = ns_string::from_rust_string(env, song.artist.clone());
            autorelease(env, v)
        }
        MPMediaItemPropertyGenre => {
            let v = ns_string::from_rust_string(env, song.genre.clone());
            autorelease(env, v)
        }
        MPMediaItemPropertyPlayCount => {
            msg_class![env; NSNumber numberWithUnsignedInt:(song.play_count)]
        }
        MPMediaItemPropertyPlaybackDuration => {
            msg_class![env; NSNumber numberWithDouble:(song.duration_secs)]
        }
        MPMediaItemPropertyArtwork => nil,
        _ => {
            log_dbg!("[(MPMediaItem*){:?} valueForProperty:'{}'] -> nil (unknown)", this, key);
            nil
        }
    };
    // `value` is now uniformly autoreleased (retain=1, pool entry pending).
    // To keep a strong reference in the cache, retain once more (count=2),
    // then return the value as-is — its existing pool entry will balance
    // the caller's auto-release expectation. After the pool drains, the
    // cached entry retains count=1. (Was: previously did an extra
    // `autorelease(env, value)` here which caused a double-release of
    // already-autoreleased NSNumber properties, crashing on stale pointers
    // when the cache was later read.)
    if value != nil {
        crate::objc::retain(env, value);
        env.objc
            .borrow_mut::<MPMediaItemHostObject>(this)
            .cached_values
            .insert(key, value);
        value
    } else {
        env.objc
            .borrow_mut::<MPMediaItemHostObject>(this)
            .cached_values
            .insert(key, nil);
        nil
    }
}

@end

};

/// Build an autoreleased `NSArray*` of the `MPMediaQuery`-exposed subset of
/// the music library (see [music_library::query_song_count]). Each
/// `MPMediaItem` is owned by the array (not also in the autorelease pool),
/// matching the "retained by the Vec" contract of [ns_array::from_vec].
pub fn make_items_array(env: &mut Environment) -> id {
    let count = music_library::query_song_count();
    let mut objs: Vec<id> = Vec::with_capacity(count);
    for i in 0..count {
        objs.push(make_item_owned(env, i));
    }
    let arr = ns_array::from_vec(env, objs);
    autorelease(env, arr)
}

/// Create one `MPMediaItem` for the song at `index`, with the caller taking
/// ownership of the initial retain (no autorelease).
pub fn make_item_owned(env: &mut Environment, index: usize) -> id {
    // Re-use the same MPMediaItem instance for each library song. Song
    // Summoner's pick handling stashes raw pointers to picked items in C++
    // structures we can't see; without a stable identity those pointers
    // dangle the moment our autorelease pool drains the array that held
    // them, and the next msg_send crashes with `orig_class != nil`. Cache
    // by song_index, hand out an extra retain on every call, and never
    // release — items leak but the game stays alive.
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<usize, id>>> = Mutex::new(None);
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(&existing) = map.get(&index) {
            // Bump retain so the caller can release without freeing.
            crate::objc::retain(env, existing);
            return existing;
        }
    }
    let cls = env.objc.get_known_class("MPMediaItem", &mut env.mem);
    let item: id = msg![env; cls alloc];
    env.objc
        .borrow_mut::<MPMediaItemHostObject>(item)
        .song_index = index;
    // Extra retain to keep the item alive for the lifetime of the process —
    // the game's stale pointer needs a valid object to receive msg_sends.
    crate::objc::retain(env, item);
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert(index, item);
    }
    item
}

#[allow(dead_code)]
pub fn song_count() -> NSUInteger {
    music_library::song_count() as NSUInteger
}
