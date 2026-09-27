/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMusicPlayerController`.
//!
//! One player, shared by `applicationMusicPlayer` and `iPodMusicPlayer`,
//! plays a queue of the user's songs through [crate::media::playback].
//! Song Summoner uses it for the trooper's song in the palace, in battle and
//! on the results screen (`Palace_iPodPlayer_Play` and friends), always with
//! a one-song collection.

use super::media_item;
use super::media_item_collection;
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::{ns_string, NSInteger, NSTimeInterval, NSUInteger};
use crate::media::playback::{Player, PumpResult};
use crate::media::{library, playcount, source};
use crate::objc::{id, msg, msg_class, nil, objc_classes, ClassExports};
use crate::Environment;

pub const MPMusicPlayerControllerNowPlayingItemDidChangeNotification: &str =
    "MPMusicPlayerControllerNowPlayingItemDidChangeNotification";
pub const MPMusicPlayerControllerPlaybackStateDidChangeNotification: &str =
    "MPMusicPlayerControllerPlaybackStateDidChangeNotification";
pub const MPMusicPlayerControllerVolumeDidChangeNotification: &str =
    "MPMusicPlayerControllerVolumeDidChangeNotification";

/// `NSNotificationName` values.
pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMusicPlayerControllerNowPlayingItemDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerNowPlayingItemDidChangeNotification),
    ),
    (
        "_MPMusicPlayerControllerPlaybackStateDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerPlaybackStateDidChangeNotification),
    ),
    (
        "_MPMusicPlayerControllerVolumeDidChangeNotification",
        HostConstant::NSString(MPMusicPlayerControllerVolumeDidChangeNotification),
    ),
];

type MPMusicPlaybackState = NSInteger;
const MPMusicPlaybackStateStopped: MPMusicPlaybackState = 0;
const MPMusicPlaybackStatePlaying: MPMusicPlaybackState = 1;
const MPMusicPlaybackStatePaused: MPMusicPlaybackState = 2;

type MPMusicRepeatMode = NSInteger;
const MPMusicRepeatModeOne: MPMusicRepeatMode = 2;
const MPMusicRepeatModeAll: MPMusicRepeatMode = 3;

type MPMusicShuffleMode = NSInteger;
const MPMusicShuffleModeSongs: MPMusicShuffleMode = 2;
const MPMusicShuffleModeAlbums: MPMusicShuffleMode = 3;

#[derive(Default)]
pub(super) struct State {
    /// The one controller object, retained forever once made.
    controller: Option<id>,
    queue: Vec<u64>,
    index: usize,
    repeat_mode: MPMusicRepeatMode,
    shuffle_mode: MPMusicShuffleMode,
    /// Where the next `-play` starts, set by `-setCurrentPlaybackTime:`
    /// while nothing is loaded.
    start_at: f64,
    /// Whether this play of the current song has been counted yet.
    counted: bool,
    notifications: bool,
    player: Player,
}

fn state(env: &mut Environment) -> &mut State {
    &mut env.framework_state.media_player.music_player
}

fn current_id(env: &mut Environment) -> Option<u64> {
    let st = state(env);
    st.queue.get(st.index).copied()
}

fn post(env: &mut Environment, name: &'static str) {
    if !state(env).notifications {
        return;
    }
    let Some(controller) = state(env).controller else {
        return;
    };
    let name = ns_string::get_static_str(env, name);
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    () = msg![env; center postNotificationName:name object:controller];
}

/// Start the song at the queue position from `start_at`.
fn start_current(env: &mut Environment) {
    let Some(pid) = current_id(env) else {
        return;
    };
    let start_at = std::mem::take(&mut state(env).start_at);
    state(env).counted = false;
    let library = library::current();
    let Some(song) = library.get(pid) else {
        log!("media: song {:016X} isn't in the library, can't play it", pid);
        return;
    };
    let file = match source::open(env, song) {
        Ok(file) => file,
        Err(e) => {
            log!("media: can't open {:?}: {}", song.locator, e);
            return;
        }
    };
    let player = &mut env.framework_state.media_player.music_player.player;
    if let Err(e) = player.start(&mut env.openal_manager, file, pid, start_at) {
        log!("media: can't play {:?}: {}", song.locator, e);
    }
    post(env, MPMusicPlayerControllerNowPlayingItemDidChangeNotification);
    post(env, MPMusicPlayerControllerPlaybackStateDidChangeNotification);
}

fn stop(env: &mut Environment) {
    let player = &mut env.framework_state.media_player.music_player.player;
    let was_loaded = player.is_loaded();
    player.stop(&mut env.openal_manager);
    if was_loaded {
        post(env, MPMusicPlayerControllerPlaybackStateDidChangeNotification);
    }
}

fn shuffle(ids: &mut [u64]) {
    // xorshift, seeded from the clock: good enough to deal songs.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    for i in (1..ids.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ids.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

fn set_queue(env: &mut Environment, mut ids: Vec<u64>) {
    stop(env);
    let shuffle_mode = state(env).shuffle_mode;
    if shuffle_mode == MPMusicShuffleModeSongs || shuffle_mode == MPMusicShuffleModeAlbums {
        shuffle(&mut ids);
    }
    log_dbg!("media: queue set to {} songs", ids.len());
    let st = state(env);
    st.queue = ids;
    st.index = 0;
    st.start_at = 0.0;
}

/// For use by `NSRunLoop` via `media_player::handle_players`: keep the
/// player fed, count plays, and move along the queue.
pub fn handle_players(env: &mut Environment) {
    let music = &mut env.framework_state.media_player.music_player;
    if !music.player.is_loaded() {
        return;
    }
    let pid = music.player.song_id();
    let result = music.player.pump(&mut env.openal_manager);

    let music = &mut env.framework_state.media_player.music_player;
    if !music.counted {
        let duration = library::current().get(pid).map_or(0.0, |s| s.duration_secs());
        let reached_end = result == PumpResult::Finished;
        if reached_end || playcount::should_count(music.player.position(&mut env.openal_manager), duration) {
            music.counted = true;
            playcount::increment(pid);
        }
    }

    if result != PumpResult::Finished {
        return;
    }
    let music = &mut env.framework_state.media_player.music_player;
    match music.repeat_mode {
        MPMusicRepeatModeOne => {}
        MPMusicRepeatModeAll => {
            music.index += 1;
            if music.index >= music.queue.len() {
                music.index = 0;
            }
        }
        _ => {
            music.index += 1;
            if music.index >= music.queue.len() {
                music.index = 0;
                post(env, MPMusicPlayerControllerPlaybackStateDidChangeNotification);
                return;
            }
        }
    }
    start_current(env);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMusicPlayerController: NSObject

+ (id)applicationMusicPlayer {
    if let Some(controller) = state(env).controller {
        return controller;
    }
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    state(env).controller = Some(new);
    new
}

+ (id)iPodMusicPlayer {
    msg![env; this applicationMusicPlayer]
}

- (())setQueueWithItemCollection:(id)collection {
    let ids = media_item_collection::persistent_ids(env, collection);
    set_queue(env, ids);
}

- (())setQueueWithQuery:(id)query {
    let items: id = msg![env; query items];
    let count: NSUInteger = msg![env; items count];
    let mut ids = Vec::with_capacity(count as usize);
    for i in 0..count {
        let item: id = msg![env; items objectAtIndex:i];
        ids.push(media_item::persistent_id_of(env, item));
    }
    ids.retain(|&pid| pid != 0);
    set_queue(env, ids);
}

- (())play {
    let player = &mut env.framework_state.media_player.music_player.player;
    if player.is_loaded() {
        if player.is_paused() {
            player.resume(&mut env.openal_manager);
            post(env, MPMusicPlayerControllerPlaybackStateDidChangeNotification);
        }
        return;
    }
    start_current(env);
}

- (())pause {
    let player = &mut env.framework_state.media_player.music_player.player;
    if player.is_loaded() && !player.is_paused() {
        player.pause(&mut env.openal_manager);
        post(env, MPMusicPlayerControllerPlaybackStateDidChangeNotification);
    }
}

- (())stop {
    stop(env);
    state(env).start_at = 0.0;
}

- (MPMusicPlaybackState)playbackState {
    let player = &state(env).player;
    if !player.is_loaded() {
        MPMusicPlaybackStateStopped
    } else if player.is_paused() {
        MPMusicPlaybackStatePaused
    } else {
        MPMusicPlaybackStatePlaying
    }
}

- (f32)volume {
    state(env).player.volume()
}
- (())setVolume:(f32)volume {
    let player = &mut env.framework_state.media_player.music_player.player;
    player.set_volume(&mut env.openal_manager, volume);
    post(env, MPMusicPlayerControllerVolumeDidChangeNotification);
}

- (NSTimeInterval)currentPlaybackTime {
    let player = &mut env.framework_state.media_player.music_player.player;
    if player.is_loaded() {
        player.position(&mut env.openal_manager)
    } else {
        state(env).start_at
    }
}
- (())setCurrentPlaybackTime:(NSTimeInterval)time {
    let time = time.max(0.0);
    let playing = state(env).player.is_loaded();
    state(env).start_at = time;
    if playing {
        // Restarting at the new position is simplest; the decoder skips
        // ahead from the start of the file.
        start_current(env);
    }
}

- (MPMusicRepeatMode)repeatMode {
    state(env).repeat_mode
}
- (())setRepeatMode:(MPMusicRepeatMode)mode {
    state(env).repeat_mode = mode;
}

- (MPMusicShuffleMode)shuffleMode {
    state(env).shuffle_mode
}
- (())setShuffleMode:(MPMusicShuffleMode)mode {
    state(env).shuffle_mode = mode;
}

- (id)nowPlayingItem {
    match current_id(env) {
        Some(pid) => media_item::item_for_id(env, pid),
        None => nil,
    }
}
- (())setNowPlayingItem:(id)item {
    let pid = media_item::persistent_id_of(env, item);
    let position = state(env).queue.iter().position(|&q| q == pid);
    let index = match position {
        Some(index) => index,
        None if pid != 0 => {
            // Not queued: play it on its own, as iOS does.
            state(env).queue = vec![pid];
            0
        }
        None => return,
    };
    let playing = state(env).player.is_loaded();
    state(env).index = index;
    state(env).start_at = 0.0;
    if playing {
        start_current(env);
    }
}
- (NSUInteger)indexOfNowPlayingItem {
    state(env).index as NSUInteger
}

- (())skipToNextItem {
    let playing = state(env).player.is_loaded();
    let st = state(env);
    st.index += 1;
    st.start_at = 0.0;
    if st.index >= st.queue.len() {
        st.index = 0;
        stop(env);
        return;
    }
    if playing {
        start_current(env);
    }
}
- (())skipToPreviousItem {
    let playing = state(env).player.is_loaded();
    let st = state(env);
    st.index = st.index.saturating_sub(1);
    st.start_at = 0.0;
    if playing {
        start_current(env);
    }
}
- (())skipToBeginning {
    let playing = state(env).player.is_loaded();
    state(env).start_at = 0.0;
    if playing {
        start_current(env);
    }
}

- (())beginGeneratingPlaybackNotifications {
    state(env).notifications = true;
}
- (())endGeneratingPlaybackNotifications {
    state(env).notifications = false;
}

@end

};

