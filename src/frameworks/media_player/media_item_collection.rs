/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaItemCollection`.

use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct MPMediaItemCollectionHostObject {
    /// `NSArray<MPMediaItem*>*`, retained.
    items: id,
}
impl HostObject for MPMediaItemCollectionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaItemCollection: MPMediaEntity

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<MPMediaItemCollectionHostObject>::default(),
        &mut env.mem,
    )
}

+ (id)collectionWithItems:(id)items { // NSArray<MPMediaItem*>*
    let cls = env.objc.get_known_class("MPMediaItemCollection", &mut env.mem);
    let coll: id = msg![env; cls alloc];
    let coll: id = msg![env; coll initWithItems:items];
    autorelease(env, coll)
}

- (id)initWithItems:(id)items { // NSArray<MPMediaItem*>*
    retain(env, items);
    env.objc.borrow_mut::<MPMediaItemCollectionHostObject>(this).items = items;
    this
}

- (())dealloc {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    release(env, items);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)items {
    env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items
}

- (id)representativeItem {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil { return nil; }
    let count: NSUInteger = msg![env; items count];
    if count == 0 { return nil; }
    msg![env; items objectAtIndex:0u32]
}

- (NSUInteger)count {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil { 0 } else { msg![env; items count] }
}

// MPMediaItemCollection inherits a "value for property" lookup that just
// forwards to its representative item — apps use the same property keys
// (MPMediaItemPropertyTitle etc.) on either, and the collection version
// reflects the rep item.
- (id)valueForProperty:(id)property {
    let rep: id = msg![env; this representativeItem];
    if rep == nil { return nil; }
    msg![env; rep valueForProperty:property]
}

@end

};

/// Build an autoreleased `MPMediaItemCollection*` wrapping `items`
/// (an `NSArray<MPMediaItem*>*`).
pub fn make_collection(env: &mut Environment, items: id) -> id {
    let cls = env.objc.get_known_class("MPMediaItemCollection", &mut env.mem);
    let coll: id = msg![env; cls alloc];
    let coll: id = msg![env; coll initWithItems:items];
    autorelease(env, coll)
}
