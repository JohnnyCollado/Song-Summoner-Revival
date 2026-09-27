/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaPlaylist`.
//!
//! Each folder in the user's music folder is a playlist. The name and songs
//! live in the `MPMediaItemCollection` host object, see
//! `media_query.rs` and `media_item_collection.rs`.

use crate::objc::{objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaPlaylist: MPMediaItemCollection
@end

};
