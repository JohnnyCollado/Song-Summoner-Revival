/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaQuery`.
//!
//! Backed by the synthetic [super::music_library]: every query exposes the
//! same flat list of `MPMediaItem`s. We cache the `items` and `collections`
//! arrays on the host object so apps that re-fetch them inside a tight loop
//! don't repeatedly rebuild a fresh `NSArray` every iteration.

use crate::frameworks::foundation::{ns_array, NSInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr,
};
use super::media_item;

/// Snapshot iPod scene + MainLoop state at the moment `+songsQuery` fires.
/// This is called from the iPod scene render function at 0x17800 in its
/// state==9 branch (the confirmation-panel build path) — so this trigger
/// fires exactly when the panel is being built.
///
/// We log scene-table entry 5's scene pointer + state, plus a 16-word
/// window of the scene struct, plus iPodState/Cancel/MusicID. On pick 1
/// this logs a full snapshot; on pick 2 if this NEVER fires, that
/// confirms the render's state==9 branch was skipped.
fn snapshot_state_for_songs_query(env: &mut crate::Environment) {
    const SCENE_TABLE_BASE: u32 = 0x1134f0;
    const SCENE_INDEX_ADDR: u32 = 0x1136b0;
    const ENTRY5_SCENE_PTR_VM: u32 = SCENE_TABLE_BASE + 5 * 0x1c + 0x18;
    const MAINLOOP_GLOBAL_PTR: u32 = 0xc48d0;

    let idx_ptr: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(SCENE_INDEX_ADDR);
    let idx: u32 = env.mem.read(idx_ptr);
    let sp_ptr: crate::mem::ConstPtr<u32> = crate::mem::Ptr::from_bits(ENTRY5_SCENE_PTR_VM);
    let entry5_scene_ptr: u32 = env.mem.read(sp_ptr);

    let scene_state = if entry5_scene_ptr != 0 {
        let p: crate::mem::ConstPtr<u32> =
            crate::mem::Ptr::from_bits(entry5_scene_ptr + 0xc);
        env.mem.read(p)
    } else {
        u32::MAX
    };
    // MainLoop struct is at MAINLOOP_GLOBAL_PTR directly (no indirection).
    let p_state: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0xc);
    let p_cancel: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x10);
    let p_lo: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x14);
    let p_hi: crate::mem::ConstPtr<u32> =
        crate::mem::Ptr::from_bits(MAINLOOP_GLOBAL_PTR + 0x18);
    let ipod_state: u32 = env.mem.read(p_state);
    let ipod_cancel: u32 = env.mem.read(p_cancel);
    let music_lo: u32 = env.mem.read(p_lo);
    let music_hi: u32 = env.mem.read(p_hi);
    log!(
        "+songsQuery FIRED: scene_idx={} entry5.scene={:#x} scene+0xc={} | iPodState={} iPodCancel={} iPodMusicID={:08x}{:08x}",
        idx, entry5_scene_ptr, scene_state, ipod_state, ipod_cancel, music_hi, music_lo
    );
    if entry5_scene_ptr != 0 {
        let mut fields = [0u32; 16];
        for k in 0..16u32 {
            let p: crate::mem::ConstPtr<u32> =
                crate::mem::Ptr::from_bits(entry5_scene_ptr + k * 4);
            fields[k as usize] = env.mem.read(p);
        }
        log!(
            "+songsQuery scene+0x00..+0x3c = [{:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}]",
            fields[0], fields[1], fields[2], fields[3],
            fields[4], fields[5], fields[6], fields[7],
            fields[8], fields[9], fields[10], fields[11],
            fields[12], fields[13], fields[14], fields[15],
        );
    }
}

#[derive(Default)]
struct MPMediaQueryHostObject {
    /// `NSArray<MPMediaItem*>*`, retained. Built lazily by `-items`.
    cached_items: id,
    /// `NSArray<MPMediaItemCollection*>*`, retained. Built lazily.
    cached_collections: id,
    /// `NSMutableSet<MPMediaPropertyPredicate*>*`, retained. Filters applied
    /// via -addFilterPredicate:. We honour these by post-filtering the
    /// library on `items` (matching on persistentID since that's what
    /// Song Summoner uses to re-find the picked song).
    filter_predicates: id,
}
impl HostObject for MPMediaQueryHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaQuery: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let new = env.objc.alloc_object(
        this,
        Box::<MPMediaQueryHostObject>::default(),
        &mut env.mem,
    );
    log!("MPMediaQuery alloc -> {:?}", new);
    new
}

+ (id)songsQuery {
    log!("MPMediaQuery +songsQuery");
    // INSTRUMENTATION: snapshot scene & MainLoop state at the moment
    // +songsQuery fires. The iPod scene render function at 0x17800 calls
    // this in its state==6 branch (the panel-build path). Logging here
    // catches the EXACT moment that branch entered.
    snapshot_state_for_songs_query(env);
    // Also snapshot the keyWindow UI view tree so we can spot duplicate
    // panel labels/imageviews accumulating across pick cycles.
    crate::frameworks::uikit::ui_view::ui_table_view::snapshot_panel_views(
        env, "songsQuery-fired",
    );
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

+ (id)playlistsQuery {
    log!("MPMediaQuery +playlistsQuery");
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

+ (id)albumsQuery {
    log!("MPMediaQuery +albumsQuery");
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

+ (id)artistsQuery {
    log!("MPMediaQuery +artistsQuery");
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

- (id)init {
    log!("MPMediaQuery {:?} -init", this);
    this
}

- (id)initWithFilterPredicates:(id)preds {
    log!("MPMediaQuery {:?} -initWithFilterPredicates:{:?}", this, preds);
    if preds != nil {
        retain(env, preds);
        env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates = preds;
    }
    this
}

- (())dealloc {
    let &MPMediaQueryHostObject { cached_items, cached_collections, filter_predicates } =
        env.objc.borrow(this);
    release(env, cached_items);
    release(env, cached_collections);
    release(env, filter_predicates);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)items {
    let cached = env.objc.borrow::<MPMediaQueryHostObject>(this).cached_items;
    if cached != nil {
        return cached;
    }
    // If we have filter predicates, honour the persistentID one (the only
    // shape Song Summoner uses post-pick) and return just the matching
    // song. Other predicate properties fall back to no-filter for now.
    let filter_set = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    let mut filtered: Option<Vec<usize>> = None;
    if filter_set != nil {
        let count: crate::frameworks::foundation::NSUInteger =
            msg![env; filter_set count];
        if count > 0 {
            let all_objs: id = msg![env; filter_set allObjects];
            let preds_count: crate::frameworks::foundation::NSUInteger =
                msg![env; all_objs count];
            let mut indices: Vec<usize> = Vec::new();
            let mut applied = false;
            for i in 0..preds_count {
                let pred: id = msg![env; all_objs objectAtIndex:i];
                let value: id = msg![env; pred value];
                let property: id = msg![env; pred property];
                let prop_str = crate::frameworks::foundation::ns_string::to_rust_string(
                    env, property,
                ).to_string();
                if prop_str == super::media_item::MPMediaItemPropertyPersistentID {
                    let pid: u64 = msg![env; value unsignedLongLongValue];
                    let resolved = super::music_library::find_song_index_by_persistent_id(pid);
                    log!(
                        "MPMediaQuery filter PID={:016X} -> library_index={:?}",
                        pid, resolved
                    );
                    if let Some(idx) = resolved {
                        if !indices.contains(&idx) {
                            indices.push(idx);
                        }
                    }
                    applied = true;
                }
            }
            if applied {
                filtered = Some(indices);
            }
        }
    }
    let arr = if let Some(indices) = filtered {
        log!("MPMediaQuery items filtered -> indices={:?}", indices);
        let mut objs: Vec<id> = Vec::with_capacity(indices.len());
        for i in indices {
            objs.push(media_item::make_item_owned(env, i));
        }
        let a = ns_array::from_vec(env, objs);
        autorelease(env, a)
    } else {
        log!("MPMediaQuery items unfiltered (no predicates applied)");
        media_item::make_items_array(env)
    };
    retain(env, arr);
    env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_items = arr;
    arr
}

- (id)collections {
    // Bust the cache when a swap is staged so the picker reflects the
    // user's just-tapped 1309-row choice on its next read. Otherwise the
    // cached pre-swap array would be returned and the pick would fire on
    // the wrong song.
    let swap_active = super::music_library::swapped_first_pid().is_some();
    let cached = env.objc.borrow::<MPMediaQueryHostObject>(this).cached_collections;
    if cached != nil && !swap_active {
        return cached;
    }
    if cached != nil && swap_active {
        // Drop the stale cache so the rebuild below is what gets stored.
        let old = std::mem::replace(
            &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_collections,
            nil,
        );
        release(env, old);
    }
    // Honour filter predicates the same way -items does: if a PID predicate is
    // set, return a single one-item collection for the matching song. Song
    // Summoner relies on this — its post-pick lookup goes through
    // -collections, reads [0].representativeItem.title, and would otherwise
    // see song 0 regardless of which row was tapped.
    let filter_set = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    let mut filtered: Option<Vec<usize>> = None;
    if filter_set != nil {
        let count: crate::frameworks::foundation::NSUInteger =
            msg![env; filter_set count];
        if count > 0 {
            let all_objs: id = msg![env; filter_set allObjects];
            let preds_count: crate::frameworks::foundation::NSUInteger =
                msg![env; all_objs count];
            let mut indices: Vec<usize> = Vec::new();
            let mut applied = false;
            for i in 0..preds_count {
                let pred: id = msg![env; all_objs objectAtIndex:i];
                let value: id = msg![env; pred value];
                let property: id = msg![env; pred property];
                let prop_str = crate::frameworks::foundation::ns_string::to_rust_string(
                    env, property,
                ).to_string();
                if prop_str == super::media_item::MPMediaItemPropertyPersistentID {
                    let pid: u64 = msg![env; value unsignedLongLongValue];
                    let resolved = super::music_library::find_song_index_by_persistent_id(pid);
                    log!(
                        "MPMediaQuery collections filter PID={:016X} -> library_index={:?}",
                        pid, resolved
                    );
                    if let Some(idx) = resolved {
                        if !indices.contains(&idx) {
                            indices.push(idx);
                        }
                    }
                    applied = true;
                }
            }
            if applied {
                filtered = Some(indices);
            }
        }
    }
    if let Some(filtered) = filtered {
        // Post-pick PID-filter path: return single-song collections matching
        // the predicate (game's post-pick lookup expects items[0] to be the
        // picked song).
        log!("MPMediaQuery collections filtered -> indices={:?}", filtered);
        let mut wrapped: Vec<id> = Vec::with_capacity(filtered.len());
        for i in filtered {
            let item_owned = media_item::make_item_owned(env, i);
            let one_item: id = ns_array::from_vec(env, vec![item_owned]);
            let one_item = autorelease(env, one_item);
            let coll = super::media_item_collection::make_collection(env, one_item);
            retain(env, coll);
            wrapped.push(coll);
        }
        let arr = ns_array::from_vec(env, wrapped);
        retain(env, arr);
        env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_collections = arr;
        return arr;
    }

    // Unfiltered (initial picker open). Expose the FULL library as
    // one-item collections — that way the game builds its 1309-row drill
    // table, which we promote to the window for scrollable browsing.
    // Picks from the promoted table get hijacked in ui_table_view's
    // cell-tap hook: it stages the chosen PID via
    // `set_swapped_first_pid`, then forces a reloadData on the visible
    // 5-row IPDSongsTab so the swap takes effect at index 0, then
    // dispatches row-0 select on that table — game picks the staged song.
    let total = super::music_library::song_count();
    let swap_pid = super::music_library::swapped_first_pid();
    let swap_idx = swap_pid.and_then(super::music_library::find_song_index_by_persistent_id);

    let mut indices: Vec<usize> = (0..total).collect();
    if let Some(idx) = swap_idx {
        if !indices.is_empty() {
            indices[0] = idx;
        } else {
            indices.push(idx);
        }
    }
    log!(
        "MPMediaQuery collections unfiltered: {} of {} songs (swap={:?}): first 5 indices={:?}",
        indices.len(), total, swap_idx, &indices[..indices.len().min(5)]
    );
    let mut wrapped: Vec<id> = Vec::with_capacity(indices.len());
    for i in indices {
        let item_owned = media_item::make_item_owned(env, i);
        let one_item: id = ns_array::from_vec(env, vec![item_owned]);
        let one_item = autorelease(env, one_item);
        let coll = super::media_item_collection::make_collection(env, one_item);
        retain(env, coll);
        wrapped.push(coll);
    }
    let arr = ns_array::from_vec(env, wrapped);
    retain(env, arr);
    env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_collections = arr;
    arr
}

- (NSInteger)groupingType { 0 }
- (())setGroupingType:(NSInteger)_t {}

- (id)filterPredicates {
    let preds = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    log!("MPMediaQuery {:?} -filterPredicates -> {:?}", this, preds);
    preds
}
- (())setFilterPredicates:(id)preds {
    log!("MPMediaQuery {:?} -setFilterPredicates:{:?}", this, preds);
    retain(env, preds);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates,
        preds,
    );
    release(env, old);
    // Invalidate item caches so the next read filters.
    let cached_items = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_items,
        nil,
    );
    release(env, cached_items);
    let cached_collections = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_collections,
        nil,
    );
    release(env, cached_collections);
}

- (())addFilterPredicate:(id)predicate {
    log!("MPMediaQuery {:?} -addFilterPredicate:{:?}", this, predicate);
    let existing = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    let set: id = if existing == nil {
        let s: id = msg_class![env; NSMutableSet alloc];
        let s: id = msg![env; s init];
        env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates = s;
        s
    } else {
        existing
    };
    () = msg![env; set addObject:predicate];
    // Invalidate caches so next items reads honour the new predicate.
    let cached_items = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_items,
        nil,
    );
    release(env, cached_items);
    let cached_collections = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).cached_collections,
        nil,
    );
    release(env, cached_collections);
}
- (())removeFilterPredicate:(id)predicate {
    let set = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    if set == nil { return; }
    () = msg![env; set removeObject:predicate];
}

- (id)valueForProperty:(id)_property {
    nil
}

@end

};
