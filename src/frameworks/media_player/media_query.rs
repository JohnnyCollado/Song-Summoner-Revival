/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaQuery`.
//!
//! Queries run against the library the engine has loaded
//! ([crate::media::library::current]). `-items` is every matching song in
//! title order; `-collections` groups them by the query's grouping type.
//! Song Summoner looks up a picked song with `songsQuery` plus a
//! PersistentID predicate, then reads `collections` → `items`.

use super::media_item;
use super::media_item_collection::new_collection;
use super::media_property_predicate::to_filter;
use crate::frameworks::foundation::{ns_array, NSInteger, NSUInteger};
use crate::media::index::{Filter, Group, Library};
use crate::media::library;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

type MPMediaGrouping = NSInteger;
const MPMediaGroupingTitle: MPMediaGrouping = 0;
const MPMediaGroupingAlbum: MPMediaGrouping = 1;
const MPMediaGroupingArtist: MPMediaGrouping = 2;
const MPMediaGroupingAlbumArtist: MPMediaGrouping = 3;
const MPMediaGroupingGenre: MPMediaGrouping = 5;
const MPMediaGroupingPlaylist: MPMediaGrouping = 6;

struct MPMediaQueryHostObject {
    grouping: MPMediaGrouping,
    /// `NSMutableSet<MPMediaPropertyPredicate*>*`, retained, or nil.
    predicates: id,
    /// `NSArray*`s, retained, cleared whenever the predicates change.
    cached_items: id,
    cached_collections: id,
}
impl HostObject for MPMediaQueryHostObject {}

fn new_query(env: &mut Environment, class: id, grouping: MPMediaGrouping) -> id {
    let query: id = msg![env; class alloc];
    let query: id = msg![env; query init];
    env.objc
        .borrow_mut::<MPMediaQueryHostObject>(query)
        .grouping = grouping;
    autorelease(env, query)
}

/// A new `NSMutableSet` (owned by the caller) with `set`'s objects.
fn mutable_copy_of_set(env: &mut Environment, set: id) -> id {
    let copy: id = msg_class![env; NSMutableSet alloc];
    let copy: id = msg![env; copy init];
    let all: id = msg![env; set allObjects];
    let count: NSUInteger = msg![env; all count];
    for i in 0..count {
        let object: id = msg![env; all objectAtIndex:i];
        () = msg![env; copy addObject:object];
    }
    copy
}

fn clear_caches(env: &mut Environment, query: id) {
    let host = env.objc.borrow_mut::<MPMediaQueryHostObject>(query);
    let items = std::mem::replace(&mut host.cached_items, nil);
    let collections = std::mem::replace(&mut host.cached_collections, nil);
    release(env, items);
    release(env, collections);
}

fn filters(env: &mut Environment, query: id) -> Vec<Filter> {
    let predicates = env.objc.borrow::<MPMediaQueryHostObject>(query).predicates;
    if predicates == nil {
        return Vec::new();
    }
    let all: id = msg![env; predicates allObjects];
    let count: NSUInteger = msg![env; all count];
    let mut out = Vec::new();
    for i in 0..count {
        let predicate: id = msg![env; all objectAtIndex:i];
        if let Some(filter) = to_filter(env, predicate) {
            out.push(filter);
        }
    }
    out
}

/// Keep each group's songs that passed the filters, dropping empty groups.
fn filter_groups(groups: &[Group], matching: &[bool]) -> Vec<(String, Vec<usize>)> {
    groups
        .iter()
        .map(|g| {
            let songs: Vec<usize> = g.songs.iter().copied().filter(|&i| matching[i]).collect();
            (g.name.clone(), songs)
        })
        .filter(|(_, songs)| !songs.is_empty())
        .collect()
}

fn group_by_genre(library: &Library, songs: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for &i in songs {
        let genre = &library.songs[i].genre;
        match groups.iter_mut().find(|(name, _)| name == genre) {
            Some((_, list)) => list.push(i),
            None => groups.push((genre.clone(), vec![i])),
        }
    }
    groups.sort_by(|a, b| {
        let (a, b) = (
            crate::media::index::sort_key(&a.0),
            crate::media::index::sort_key(&b.0),
        );
        crate::media::index::compare_keys(&a, &b)
    });
    groups
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaQuery: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(MPMediaQueryHostObject {
            grouping: MPMediaGroupingTitle,
            predicates: nil,
            cached_items: nil,
            cached_collections: nil,
        }),
        &mut env.mem,
    )
}

+ (id)songsQuery {
    new_query(env, this, MPMediaGroupingTitle)
}
+ (id)albumsQuery {
    new_query(env, this, MPMediaGroupingAlbum)
}
+ (id)artistsQuery {
    new_query(env, this, MPMediaGroupingArtist)
}
+ (id)playlistsQuery {
    new_query(env, this, MPMediaGroupingPlaylist)
}
+ (id)genresQuery {
    new_query(env, this, MPMediaGroupingGenre)
}

- (id)init {
    this
}

- (id)initWithFilterPredicates:(id)predicates { // NSSet*
    if predicates != nil {
        let set = mutable_copy_of_set(env, predicates);
        env.objc
            .borrow_mut::<MPMediaQueryHostObject>(this)
            .predicates = set;
    }
    this
}

- (())dealloc {
    clear_caches(env, this);
    let predicates = env.objc.borrow::<MPMediaQueryHostObject>(this).predicates;
    release(env, predicates);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (MPMediaGrouping)groupingType {
    env.objc.borrow::<MPMediaQueryHostObject>(this).grouping
}
- (())setGroupingType:(MPMediaGrouping)grouping {
    env.objc.borrow_mut::<MPMediaQueryHostObject>(this).grouping = grouping;
    clear_caches(env, this);
}

- (id)filterPredicates {
    env.objc.borrow::<MPMediaQueryHostObject>(this).predicates
}
- (())setFilterPredicates:(id)predicates { // NSSet*
    let set: id = if predicates == nil {
        nil
    } else {
        mutable_copy_of_set(env, predicates)
    };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).predicates,
        set,
    );
    release(env, old);
    clear_caches(env, this);
}
- (())addFilterPredicate:(id)predicate {
    if env.objc.borrow::<MPMediaQueryHostObject>(this).predicates == nil {
        let set: id = msg_class![env; NSMutableSet alloc];
        let set: id = msg![env; set init];
        env.objc
            .borrow_mut::<MPMediaQueryHostObject>(this)
            .predicates = set;
    }
    let set = env.objc.borrow::<MPMediaQueryHostObject>(this).predicates;
    () = msg![env; set addObject:predicate];
    clear_caches(env, this);
}
- (())removeFilterPredicate:(id)predicate {
    let set = env.objc.borrow::<MPMediaQueryHostObject>(this).predicates;
    if set != nil {
        () = msg![env; set removeObject:predicate];
        clear_caches(env, this);
    }
}

- (id)items {
    let cached = env.objc.borrow::<MPMediaQueryHostObject>(this).cached_items;
    if cached != nil {
        return cached;
    }
    let filters = filters(env, this);
    let library = library::current();
    let songs = library.filtered(&filters);
    log_dbg!("media: query {:?} items: {} songs", this, songs.len());
    let mut items = Vec::with_capacity(songs.len());
    for i in songs {
        let item = media_item::item_for_id(env, library.songs[i].id);
        retain(env, item);
        items.push(item);
    }
    let array = ns_array::from_vec(env, items);
    env.objc
        .borrow_mut::<MPMediaQueryHostObject>(this)
        .cached_items = array;
    array
}

- (id)collections {
    let cached = env.objc.borrow::<MPMediaQueryHostObject>(this).cached_collections;
    if cached != nil {
        return cached;
    }
    let filters = filters(env, this);
    let grouping = env.objc.borrow::<MPMediaQueryHostObject>(this).grouping;
    let library = library::current();
    let songs = library.filtered(&filters);
    let mut matching = vec![false; library.len()];
    for &i in &songs {
        matching[i] = true;
    }

    let (class_name, groups): (&str, Vec<(Option<String>, Vec<usize>)>) = match grouping {
        MPMediaGroupingAlbum => (
            "MPMediaItemCollection",
            filter_groups(&library.albums, &matching)
                .into_iter()
                .map(|(_, s)| (None, s))
                .collect(),
        ),
        MPMediaGroupingArtist | MPMediaGroupingAlbumArtist => (
            "MPMediaItemCollection",
            filter_groups(&library.artists, &matching)
                .into_iter()
                .map(|(_, s)| (None, s))
                .collect(),
        ),
        MPMediaGroupingPlaylist => (
            "MPMediaPlaylist",
            filter_groups(&library.playlists, &matching)
                .into_iter()
                .map(|(name, s)| (Some(name), s))
                .collect(),
        ),
        MPMediaGroupingGenre => (
            "MPMediaItemCollection",
            group_by_genre(&library, &songs)
                .into_iter()
                .map(|(_, s)| (None, s))
                .collect(),
        ),
        other => {
            if other != MPMediaGroupingTitle {
                log!("TODO: MPMediaQuery grouping {}, treated as by title", other);
            }
            (
                "MPMediaItemCollection",
                songs.iter().map(|&i| (None, vec![i])).collect(),
            )
        }
    };
    log_dbg!(
        "media: query {:?} collections: {} groups (grouping {})",
        this,
        groups.len(),
        grouping
    );

    let mut collections = Vec::with_capacity(groups.len());
    for (name, songs) in groups {
        let ids = songs.iter().map(|&i| library.songs[i].id).collect();
        let collection = new_collection(env, class_name, ids, name);
        retain(env, collection);
        collections.push(collection);
    }
    let array = ns_array::from_vec(env, collections);
    env.objc
        .borrow_mut::<MPMediaQueryHostObject>(this)
        .cached_collections = array;
    array
}

@end

};
