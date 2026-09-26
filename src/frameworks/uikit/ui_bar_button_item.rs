/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `UIBarItem`, `UIBarButtonItem`. Enough for apps that wire up a
//! Cancel/Done button on a navigation bar (Song Summoner's picker does this).

use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr, SEL,
};

type UIBarButtonItemStyle = NSInteger;
type UIBarButtonSystemItem = NSInteger;

#[derive(Default)]
struct UIBarButtonItemHostObject {
    /// `NSString*`, retained.
    title: id,
    /// `UIImage*`, retained.
    image: id,
    target: id,
    action: Option<SEL>,
    style: UIBarButtonItemStyle,
    tag: NSInteger,
    enabled: bool,
    /// `UIView*` (UIBarButtonItem customView), retained.
    custom_view: id,
    width: f32,
}
impl HostObject for UIBarButtonItemHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIBarItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let mut host = Box::<UIBarButtonItemHostObject>::default();
    host.enabled = true;
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)title { env.objc.borrow::<UIBarButtonItemHostObject>(this).title }
- (())setTitle:(id)title {
    retain(env, title);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).title,
        title,
    );
    release(env, old);
}
- (id)image { env.objc.borrow::<UIBarButtonItemHostObject>(this).image }
- (())setImage:(id)image {
    retain(env, image);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).image,
        image,
    );
    release(env, old);
}
- (NSInteger)tag { env.objc.borrow::<UIBarButtonItemHostObject>(this).tag }
- (())setTag:(NSInteger)tag {
    env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).tag = tag;
}
- (bool)isEnabled { env.objc.borrow::<UIBarButtonItemHostObject>(this).enabled }
- (())setEnabled:(bool)enabled {
    env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).enabled = enabled;
}

- (())dealloc {
    let &UIBarButtonItemHostObject { title, image, custom_view, .. } =
        env.objc.borrow(this);
    release(env, title);
    release(env, image);
    release(env, custom_view);
    env.objc.dealloc_object(this, &mut env.mem);
}

@end

@implementation UIBarButtonItem: UIBarItem

- (id)init {
    this
}
- (id)initWithTitle:(id)title
              style:(UIBarButtonItemStyle)style
             target:(id)target
             action:(SEL)action {
    retain(env, title);
    let host = env.objc.borrow_mut::<UIBarButtonItemHostObject>(this);
    host.title = title;
    host.style = style;
    host.target = target;
    host.action = Some(action);
    this
}
- (id)initWithBarButtonSystemItem:(UIBarButtonSystemItem)_sys
                           target:(id)target
                           action:(SEL)action {
    let host = env.objc.borrow_mut::<UIBarButtonItemHostObject>(this);
    host.target = target;
    host.action = Some(action);
    this
}
- (id)initWithImage:(id)image
              style:(UIBarButtonItemStyle)style
             target:(id)target
             action:(SEL)action {
    retain(env, image);
    let host = env.objc.borrow_mut::<UIBarButtonItemHostObject>(this);
    host.image = image;
    host.style = style;
    host.target = target;
    host.action = Some(action);
    this
}
- (id)initWithCustomView:(id)view {
    retain(env, view);
    env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).custom_view = view;
    this
}

- (id)target { env.objc.borrow::<UIBarButtonItemHostObject>(this).target }
- (())setTarget:(id)target {
    env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).target = target;
}

- (id)customView { env.objc.borrow::<UIBarButtonItemHostObject>(this).custom_view }
- (())setCustomView:(id)view {
    retain(env, view);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).custom_view,
        view,
    );
    release(env, old);
}

- (UIBarButtonItemStyle)style {
    env.objc.borrow::<UIBarButtonItemHostObject>(this).style
}
- (())setStyle:(UIBarButtonItemStyle)style {
    env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).style = style;
}

- (f32)width { env.objc.borrow::<UIBarButtonItemHostObject>(this).width }
- (())setWidth:(f32)w { env.objc.borrow_mut::<UIBarButtonItemHostObject>(this).width = w; }

@end

};

