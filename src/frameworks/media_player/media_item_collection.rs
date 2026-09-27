/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaItemCollection` (and the storage `MPMediaPlaylist` shares).

use super::media_item::{self, MPMediaPlaylistPropertyName, MPMediaPlaylistPropertyPersistentID};
use crate::frameworks::foundation::{ns_array, ns_string, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
pub(super) struct MPMediaItemCollectionHostObject {
    persistent_ids: Vec<u64>,
    /// `NSArray<MPMediaItem*>*`, retained, made on first `-items`.
    items: id,
    /// Playlists only: the playlist's name.
    name: Option<String>,
}
impl HostObject for MPMediaItemCollectionHostObject {}

/// A new autoreleased collection of `class_name` (`MPMediaItemCollection`
/// or `MPMediaPlaylist`) holding these songs.
pub fn new_collection(
    env: &mut Environment,
    class_name: &str,
    persistent_ids: Vec<u64>,
    name: Option<String>,
) -> id {
    let class = env.objc.get_known_class(class_name, &mut env.mem);
    let collection: id = msg![env; class alloc];
    {
        let host = env
            .objc
            .borrow_mut::<MPMediaItemCollectionHostObject>(collection);
        host.persistent_ids = persistent_ids;
        host.name = name;
    }
    autorelease(env, collection)
}

/// The songs in a collection, in order.
pub fn persistent_ids(env: &mut Environment, collection: id) -> Vec<u64> {
    if collection == nil {
        return Vec::new();
    }
    env.objc
        .borrow::<MPMediaItemCollectionHostObject>(collection)
        .persistent_ids
        .clone()
}

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
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithItems:items];
    autorelease(env, new)
}

- (id)initWithItems:(id)items { // NSArray<MPMediaItem*>*
    let count: NSUInteger = if items == nil { 0 } else { msg![env; items count] };
    let mut ids = Vec::with_capacity(count as usize);
    for i in 0..count {
        let item: id = msg![env; items objectAtIndex:i];
        let pid = media_item::persistent_id_of(env, item);
        if pid != 0 {
            ids.push(pid);
        }
    }
    env.objc
        .borrow_mut::<MPMediaItemCollectionHostObject>(this)
        .persistent_ids = ids;
    this
}

- (())dealloc {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    release(env, items);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)items {
    let cached = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if cached != nil {
        return cached;
    }
    let ids = env
        .objc
        .borrow::<MPMediaItemCollectionHostObject>(this)
        .persistent_ids
        .clone();
    let mut items = Vec::with_capacity(ids.len());
    for pid in ids {
        let item = media_item::item_for_id(env, pid);
        // The array owns one reference to each (immortal) item.
        retain(env, item);
        items.push(item);
    }
    let array = ns_array::from_vec(env, items);
    env.objc
        .borrow_mut::<MPMediaItemCollectionHostObject>(this)
        .items = array;
    array
}

- (NSUInteger)count {
    env.objc
        .borrow::<MPMediaItemCollectionHostObject>(this)
        .persistent_ids
        .len() as NSUInteger
}

- (id)representativeItem {
    let first = env
        .objc
        .borrow::<MPMediaItemCollectionHostObject>(this)
        .persistent_ids
        .first()
        .copied();
    match first {
        Some(pid) => media_item::item_for_id(env, pid),
        None => nil,
    }
}

- (NSUInteger)mediaTypes {
    1 // MPMediaTypeMusic
}

- (id)valueForProperty:(id)property { // NSString*
    if property == nil {
        return nil;
    }
    let key = ns_string::to_rust_string(env, property).to_string();
    let name = env
        .objc
        .borrow::<MPMediaItemCollectionHostObject>(this)
        .name
        .clone();
    if let Some(name) = name {
        if key == MPMediaPlaylistPropertyName {
            let string = ns_string::from_rust_string(env, name);
            return autorelease(env, string);
        }
        if key == MPMediaPlaylistPropertyPersistentID {
            let pid = crate::media::index::persistent_id(&name);
            return msg_class![env; NSNumber numberWithUnsignedLongLong:pid];
        }
    }
    // Anything else describes the collection's songs.
    let item: id = msg![env; this representativeItem];
    if item == nil {
        return nil;
    }
    msg![env; item valueForProperty:property]
}

@end

};
