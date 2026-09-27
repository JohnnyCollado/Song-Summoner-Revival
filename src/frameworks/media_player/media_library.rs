/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaLibrary`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::objc::{autorelease, id, msg, msg_class, objc_classes, ClassExports};

pub const MPMediaLibraryDidChangeNotification: &str = "MPMediaLibraryDidChangeNotification";

pub const CONSTANTS: ConstantExports = &[(
    "_MPMediaLibraryDidChangeNotification",
    HostConstant::NSString(MPMediaLibraryDidChangeNotification),
)];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaLibrary: NSObject

// The library's contents live in crate::media and are reached through
// MPMediaQuery, so this object carries no state of its own.
+ (id)defaultMediaLibrary {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

- (())beginGeneratingLibraryChangeNotifications {}
- (())endGeneratingLibraryChangeNotifications {}

- (id)lastModifiedDate {
    msg_class![env; NSDate date]
}

@end

};
