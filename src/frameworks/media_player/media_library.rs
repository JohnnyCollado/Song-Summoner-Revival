/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaLibrary`.
//!
//! Apps sometimes check whether a media library exists at all (rather than
//! only inspecting `MPMediaQuery items`). Returning `nil` from
//! `+defaultMediaLibrary` made Song Summoner short-circuit into its "no
//! tunes" branch, so we hand back a singleton placeholder instance — its
//! `items` method still defers to `MPMediaQuery`.

use crate::frameworks::foundation::NSInteger;
use crate::objc::{id, objc_classes, ClassExports, HostObject};
use crate::Environment;

struct MPMediaLibraryHostObject;
impl HostObject for MPMediaLibraryHostObject {}

#[derive(Default)]
pub struct State {
    default_library: Option<id>,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.media_player.media_library
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaLibrary: NSObject

+ (id)defaultMediaLibrary {
    if let Some(existing) = State::get(env).default_library {
        return existing;
    }
    let new: id = env.objc.alloc_static_object(
        this,
        Box::new(MPMediaLibraryHostObject),
        &mut env.mem,
    );
    State::get(env).default_library = Some(new);
    new
}

+ (NSInteger)authorizationStatus {
    // 3 == MPMediaLibraryAuthorizationStatusAuthorized
    3
}

+ (())requestAuthorization:(id)_handler {
    // No-op: we always claim authorization above.
}

- (())beginGeneratingLibraryChangeNotifications {}
- (())endGeneratingLibraryChangeNotifications {}

@end

};
