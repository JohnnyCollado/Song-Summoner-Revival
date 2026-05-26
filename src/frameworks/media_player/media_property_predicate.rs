/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `MPMediaPropertyPredicate`. Apps use these to filter
//! `MPMediaQuery` results to a single song / album / artist. We store the
//! property + value but never actually filter — every query already exposes
//! the full library list.

use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

type MPMediaPredicateComparison = NSInteger;

#[derive(Default)]
struct MPMediaPropertyPredicateHostObject {
    value: id,
    property: id,
    comparison: MPMediaPredicateComparison,
}
impl HostObject for MPMediaPropertyPredicateHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaPredicate: NSObject
// MPMediaPredicate is an abstract base; subclasses provide their own
// host-object allocator. Letting it inherit NSObject's `allocWithZone:` is
// fine since we never instantiate MPMediaPredicate directly.
@end

@implementation MPMediaPropertyPredicate: MPMediaPredicate

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<MPMediaPropertyPredicateHostObject>::default(),
        &mut env.mem,
    )
}

+ (id)predicateWithValue:(id)value forProperty:(id)property {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithValue:value forProperty:property comparisonType:0i32];
    autorelease(env, new)
}

+ (id)predicateWithValue:(id)value
             forProperty:(id)property
          comparisonType:(MPMediaPredicateComparison)comp {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithValue:value forProperty:property comparisonType:comp];
    autorelease(env, new)
}

- (id)initWithValue:(id)value
        forProperty:(id)property
     comparisonType:(MPMediaPredicateComparison)comp {
    retain(env, value);
    retain(env, property);
    let host = env.objc.borrow_mut::<MPMediaPropertyPredicateHostObject>(this);
    host.value = value;
    host.property = property;
    host.comparison = comp;
    this
}

- (())dealloc {
    let &MPMediaPropertyPredicateHostObject { value, property, .. } =
        env.objc.borrow(this);
    release(env, value);
    release(env, property);
    env.objc.dealloc_object(this, &mut env.mem);
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
