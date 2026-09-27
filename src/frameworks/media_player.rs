/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Media Player framework.
//!
//! The iPod library classes (`MPMediaQuery`, `MPMediaItem` and friends) are
//! backed by the user's own music, read from the library index by
//! [crate::media], and `MPMusicPlayerController` plays it. Only what apps
//! actually call is implemented; anything else logs.

mod media_entity;
pub mod media_item;
pub mod media_item_collection;
mod media_library;
mod media_picker_controller;
mod media_playlist;
mod media_property_predicate;
mod media_query;
mod movie_player;
pub mod music_player;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/MediaPlayer.framework/MediaPlayer",
    aliases: &[],
    class_exports: &[
        movie_player::CLASSES,
        music_player::CLASSES,
        media_entity::CLASSES,
        media_item::CLASSES,
        media_item_collection::CLASSES,
        media_library::CLASSES,
        media_picker_controller::CLASSES,
        media_playlist::CLASSES,
        media_property_predicate::CLASSES,
        media_query::CLASSES,
    ],
    constant_exports: &[
        movie_player::CONSTANTS,
        music_player::CONSTANTS,
        media_item::CONSTANTS,
        media_library::CONSTANTS,
    ],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    movie_player: movie_player::State,
    media_item: media_item::State,
    music_player: music_player::State,
}

/// For use by `NSRunLoop`: check media players' status, send notifications if
/// necessary.
pub fn handle_players(env: &mut crate::Environment) {
    movie_player::handle_players(env);
    music_player::handle_players(env);
}
