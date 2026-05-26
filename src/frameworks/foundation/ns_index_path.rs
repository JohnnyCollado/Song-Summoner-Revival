/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSIndexPath`.
//!
//! Minimal implementation — we only support the (section, row) flavor used by
//! `UITableView`. The full NSIndexPath is a variable-length sequence of
//! indices; storing just two is good enough for table-view code paths.

use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, msg, objc_classes, ClassExports, HostObject, NSZonePtr,
};

#[derive(Default)]
struct NSIndexPathHostObject {
    section: NSUInteger,
    row: NSUInteger,
}
impl HostObject for NSIndexPathHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSIndexPath: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<NSIndexPathHostObject>::default(),
        &mut env.mem,
    )
}

+ (id)indexPathForRow:(NSUInteger)row inSection:(NSUInteger)section {
    let new: id = msg![env; this alloc];
    let host = env.objc.borrow_mut::<NSIndexPathHostObject>(new);
    host.section = section;
    host.row = row;
    autorelease(env, new)
}

- (NSUInteger)section {
    env.objc.borrow::<NSIndexPathHostObject>(this).section
}

- (NSUInteger)row {
    env.objc.borrow::<NSIndexPathHostObject>(this).row
}

- (NSUInteger)length { 2 }

- (NSInteger)indexAtPosition:(NSUInteger)position {
    let host = env.objc.borrow::<NSIndexPathHostObject>(this);
    match position {
        0 => host.section as NSInteger,
        1 => host.row as NSInteger,
        _ => -1,
    }
}

@end

};
