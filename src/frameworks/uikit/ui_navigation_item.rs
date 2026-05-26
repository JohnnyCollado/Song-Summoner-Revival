/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `UINavigationItem` — the metadata struct a `UIViewController`
//! contributes to a `UINavigationBar`. We don't draw a nav bar; this is just
//! enough storage so apps that configure title/back/left/right buttons can
//! set/get those without crashing.

use crate::objc::{
    id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

#[derive(Default)]
struct UINavigationItemHostObject {
    /// `NSString*`, retained.
    title: id,
    /// `NSString*`, retained.
    prompt: id,
    /// `UIView*`, retained.
    title_view: id,
    /// `UIBarButtonItem*`, retained.
    left_bar_button_item: id,
    /// `UIBarButtonItem*`, retained.
    right_bar_button_item: id,
    /// `UIBarButtonItem*`, retained.
    back_bar_button_item: id,
    hides_back_button: bool,
}
impl HostObject for UINavigationItemHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UINavigationItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<UINavigationItemHostObject>::default(),
        &mut env.mem,
    )
}

- (id)init { this }
- (id)initWithTitle:(id)title {
    retain(env, title);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).title = title;
    this
}

- (())dealloc {
    let &UINavigationItemHostObject {
        title, prompt, title_view, left_bar_button_item, right_bar_button_item,
        back_bar_button_item, ..
    } = env.objc.borrow(this);
    release(env, title);
    release(env, prompt);
    release(env, title_view);
    release(env, left_bar_button_item);
    release(env, right_bar_button_item);
    release(env, back_bar_button_item);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)title { env.objc.borrow::<UINavigationItemHostObject>(this).title }
- (())setTitle:(id)title {
    let copied: id = if title == nil { nil } else { msg![env; title copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).title,
        copied,
    );
    release(env, old);
}

- (id)prompt { env.objc.borrow::<UINavigationItemHostObject>(this).prompt }
- (())setPrompt:(id)prompt {
    let copied: id = if prompt == nil { nil } else { msg![env; prompt copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).prompt,
        copied,
    );
    release(env, old);
}

- (id)titleView { env.objc.borrow::<UINavigationItemHostObject>(this).title_view }
- (())setTitleView:(id)v {
    retain(env, v);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).title_view,
        v,
    );
    release(env, old);
}

- (id)leftBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).left_bar_button_item
}
- (())setLeftBarButtonItem:(id)item {
    retain(env, item);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).left_bar_button_item,
        item,
    );
    release(env, old);
}
- (())setLeftBarButtonItem:(id)item animated:(bool)_animated {
    msg![env; this setLeftBarButtonItem:item]
}

- (id)rightBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).right_bar_button_item
}
- (())setRightBarButtonItem:(id)item {
    retain(env, item);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).right_bar_button_item,
        item,
    );
    release(env, old);
}
- (())setRightBarButtonItem:(id)item animated:(bool)_animated {
    msg![env; this setRightBarButtonItem:item]
}

- (id)backBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).back_bar_button_item
}
- (())setBackBarButtonItem:(id)item {
    retain(env, item);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UINavigationItemHostObject>(this).back_bar_button_item,
        item,
    );
    release(env, old);
}

- (bool)hidesBackButton {
    env.objc.borrow::<UINavigationItemHostObject>(this).hides_back_button
}
- (())setHidesBackButton:(bool)hidden {
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).hides_back_button = hidden;
}
- (())setHidesBackButton:(bool)hidden animated:(bool)_animated {
    msg![env; this setHidesBackButton:hidden]
}

@end

};
