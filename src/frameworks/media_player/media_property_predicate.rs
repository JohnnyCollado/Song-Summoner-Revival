/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaPropertyPredicate`.

use super::media_item::{
    MPMediaItemPropertyAlbumArtist, MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
    MPMediaItemPropertyGenre, MPMediaItemPropertyPersistentID, MPMediaItemPropertyTitle,
};
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::media::index::{Filter, FilterValue, Property};
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

type MPMediaPredicateComparison = NSInteger;
const MPMediaPredicateComparisonEqualTo: MPMediaPredicateComparison = 0;
const MPMediaPredicateComparisonContains: MPMediaPredicateComparison = 1;

struct MPMediaPropertyPredicateHostObject {
    /// Retained.
    value: id,
    /// `NSString*`, retained.
    property: id,
    comparison: MPMediaPredicateComparison,
}
impl HostObject for MPMediaPropertyPredicateHostObject {}

/// The predicate as a [Filter] the library can apply, or `None` for a
/// property we can't filter on (which the caller then ignores).
pub fn to_filter(env: &mut Environment, predicate: id) -> Option<Filter> {
    let &MPMediaPropertyPredicateHostObject {
        value,
        property,
        comparison,
    } = env.objc.borrow(predicate);
    if property == nil || value == nil {
        return None;
    }
    let property_name = ns_string::to_rust_string(env, property).to_string();
    let property = match property_name.as_str() {
        MPMediaItemPropertyPersistentID => Property::PersistentId,
        MPMediaItemPropertyTitle => Property::Title,
        MPMediaItemPropertyArtist => Property::Artist,
        MPMediaItemPropertyAlbumTitle => Property::Album,
        MPMediaItemPropertyAlbumArtist => Property::AlbumArtist,
        MPMediaItemPropertyGenre => Property::Genre,
        other => {
            log!("TODO: MPMediaPropertyPredicate on {:?}, ignored", other);
            return None;
        }
    };
    let string_class = env.objc.get_known_class("NSString", &mut env.mem);
    let value_class = msg![env; value class];
    let value = if env.objc.class_is_subclass_of(value_class, string_class) {
        FilterValue::Text(ns_string::to_rust_string(env, value).to_string())
    } else {
        // An NSNumber, as used for persistent IDs.
        let number: u64 = msg![env; value unsignedLongLongValue];
        FilterValue::Id(number)
    };
    Some(Filter {
        property,
        value,
        contains: comparison == MPMediaPredicateComparisonContains,
    })
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaPredicate: NSObject
@end

@implementation MPMediaPropertyPredicate: MPMediaPredicate

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(MPMediaPropertyPredicateHostObject {
            value: nil,
            property: nil,
            comparison: MPMediaPredicateComparisonEqualTo,
        }),
        &mut env.mem,
    )
}

+ (id)predicateWithValue:(id)value forProperty:(id)property {
    msg![env; this predicateWithValue:value
                          forProperty:property
                       comparisonType:MPMediaPredicateComparisonEqualTo]
}

+ (id)predicateWithValue:(id)value
             forProperty:(id)property // NSString*
          comparisonType:(MPMediaPredicateComparison)comparison {
    let new: id = msg![env; this alloc];
    retain(env, value);
    let property: id = msg![env; property copy];
    *env.objc.borrow_mut::<MPMediaPropertyPredicateHostObject>(new) =
        MPMediaPropertyPredicateHostObject {
            value,
            property,
            comparison,
        };
    autorelease(env, new)
}

- (())dealloc {
    let &MPMediaPropertyPredicateHostObject { value, property, .. } = env.objc.borrow(this);
    release(env, value);
    release(env, property);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)value {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).value
}
- (id)property {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).property
}
- (MPMediaPredicateComparison)comparisonType {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).comparison
}

@end

};

