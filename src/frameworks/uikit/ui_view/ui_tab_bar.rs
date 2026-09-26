/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `UITabBar`, `UITabBarItem`, `UITabBarController`.
//!
//! Enough surface for apps that build a custom tab-based browser on top of
//! UIKit (Song Summoner's `IPDMediaPickerController` does this for the 4
//! tabs: Songs / Artists / Albums / Playlists) without us actually rendering
//! the tab bar chrome.

use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil,
    objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

type UITabBarSystemItem = NSInteger;

#[derive(Default)]
struct UITabBarItemHostObject {
    /// `NSString*`, retained.
    title: id,
    /// `UIImage*`, retained.
    image: id,
    /// `NSString*`, retained.
    badge_value: id,
    tag: NSInteger,
    enabled: bool,
}
impl HostObject for UITabBarItemHostObject {}

#[derive(Default)]
struct UITabBarHostObject {
    superclass: super::UIViewHostObject,
    /// `NSArray<UITabBarItem*>*`, retained.
    items: id,
    selected_item: id,
}
impl_HostObject_with_superclass!(UITabBarHostObject);

#[derive(Default)]
struct UITabBarControllerHostObject {
    superclass: crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject,
    /// `NSArray<UIViewController*>*`, retained.
    view_controllers: id,
    selected_view_controller: id,
    selected_index: NSInteger,
    /// `UITabBar*`, retained.
    tab_bar: id,
    /// `id<UITabBarControllerDelegate>`, weak.
    delegate: id,
}
impl_HostObject_with_superclass!(UITabBarControllerHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITabBarItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let mut host = Box::<UITabBarItemHostObject>::default();
    host.enabled = true;
    host.tag = 0;
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)init {
    this
}

- (id)initWithTitle:(id)title image:(id)image tag:(NSInteger)tag {
    retain(env, title);
    retain(env, image);
    let host = env.objc.borrow_mut::<UITabBarItemHostObject>(this);
    host.title = title;
    host.image = image;
    host.tag = tag;
    this
}

- (id)initWithTabBarSystemItem:(UITabBarSystemItem)_sys_item tag:(NSInteger)tag {
    env.objc.borrow_mut::<UITabBarItemHostObject>(this).tag = tag;
    this
}

- (())dealloc {
    let &UITabBarItemHostObject { title, image, badge_value, .. } =
        env.objc.borrow(this);
    release(env, title);
    release(env, image);
    release(env, badge_value);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)title { env.objc.borrow::<UITabBarItemHostObject>(this).title }
- (())setTitle:(id)title {
    retain(env, title);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITabBarItemHostObject>(this).title,
        title,
    );
    release(env, old);
}
- (id)image { env.objc.borrow::<UITabBarItemHostObject>(this).image }
- (())setImage:(id)image {
    retain(env, image);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITabBarItemHostObject>(this).image,
        image,
    );
    release(env, old);
}
- (id)badgeValue { env.objc.borrow::<UITabBarItemHostObject>(this).badge_value }
- (())setBadgeValue:(id)badge {
    retain(env, badge);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITabBarItemHostObject>(this).badge_value,
        badge,
    );
    release(env, old);
}
- (NSInteger)tag { env.objc.borrow::<UITabBarItemHostObject>(this).tag }
- (())setTag:(NSInteger)tag {
    env.objc.borrow_mut::<UITabBarItemHostObject>(this).tag = tag;
}
- (bool)isEnabled { env.objc.borrow::<UITabBarItemHostObject>(this).enabled }
- (())setEnabled:(bool)enabled {
    env.objc.borrow_mut::<UITabBarItemHostObject>(this).enabled = enabled;
}

@end

@implementation UITabBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UITabBarHostObject>::default(), &mut env.mem)
}

- (())dealloc {
    let &UITabBarHostObject { items, .. } = env.objc.borrow(this);
    release(env, items);
    msg_super![env; this dealloc]
}

- (id)items { env.objc.borrow::<UITabBarHostObject>(this).items }
- (())setItems:(id)items {
    retain(env, items);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITabBarHostObject>(this).items,
        items,
    );
    release(env, old);
}
- (())setItems:(id)items animated:(bool)_animated {
    msg![env; this setItems:items]
}
- (id)selectedItem { env.objc.borrow::<UITabBarHostObject>(this).selected_item }
- (())setSelectedItem:(id)item {
    env.objc.borrow_mut::<UITabBarHostObject>(this).selected_item = item;
}
- (())setDelegate:(id)_delegate {}
- (())setBarStyle:(NSInteger)_style {}
- (())setTintColor:(id)_color {}

@end

@implementation UITabBarController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<UITabBarControllerHostObject>::default(),
        &mut env.mem,
    )
}

- (id)init {
    let this: id = msg_super![env; this initWithNibName:nil bundle:nil];
    // Set up an empty UITabBar so accessors don't return nil.
    let tab_bar: id = msg_class![env; UITabBar alloc];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 431.0 },
        size: CGSize { width: 320.0, height: 49.0 },
    };
    let tab_bar: id = msg![env; tab_bar initWithFrame:frame];
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).tab_bar = tab_bar;
    this
}

- (())dealloc {
    let &UITabBarControllerHostObject {
        view_controllers, tab_bar, ..
    } = env.objc.borrow(this);
    release(env, view_controllers);
    release(env, tab_bar);
    msg_super![env; this dealloc]
}

- (id)tabBar {
    env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar
}

- (id)viewControllers {
    env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers
}
- (())setViewControllers:(id)vcs {
    retain(env, vcs);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITabBarControllerHostObject>(this).view_controllers,
        vcs,
    );
    release(env, old);

    // Auto-select the LAST view controller. In iPod-style pickers (and Song
    // Summoner's), this is the "Songs" tab — the one with the full library —
    // while the earlier tabs are Playlists / Artists / Albums with much
    // smaller datasets. Since we don't render the tab bar chrome yet, picking
    // the most useful tab gives a usable picker out of the box.
    if vcs != nil {
        let count: crate::frameworks::foundation::NSUInteger = msg![env; vcs count];
        if count > 0 {
            let idx = (count - 1) as u32;
            let chosen: id = msg![env; vcs objectAtIndex:idx];
            env.objc.borrow_mut::<UITabBarControllerHostObject>(this)
                .selected_view_controller = chosen;
            env.objc.borrow_mut::<UITabBarControllerHostObject>(this)
                .selected_index = idx as NSInteger;
            let chosen_view: id = msg![env; chosen view];
            () = msg![env; this setView:chosen_view];
        }
    }
}
- (())setViewControllers:(id)vcs animated:(bool)_animated {
    msg![env; this setViewControllers:vcs]
}

- (id)selectedViewController {
    env.objc.borrow::<UITabBarControllerHostObject>(this).selected_view_controller
}
- (())setSelectedViewController:(id)vc {
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_view_controller = vc;
    if vc != nil {
        let v: id = msg![env; vc view];
        () = msg![env; this setView:v];
    }
}

- (NSInteger)selectedIndex {
    env.objc.borrow::<UITabBarControllerHostObject>(this).selected_index
}
- (())setSelectedIndex:(NSInteger)idx {
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = idx;
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    if vcs == nil { return; }
    let count: crate::frameworks::foundation::NSUInteger = msg![env; vcs count];
    if (idx as crate::frameworks::foundation::NSUInteger) < count {
        let vc: id = msg![env; vcs objectAtIndex:(idx as u32)];
        env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_view_controller = vc;
        let v: id = msg![env; vc view];
        () = msg![env; this setView:v];
    }
}

- (id)delegate {
    env.objc.borrow::<UITabBarControllerHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).delegate = delegate;
}

- (id)moreNavigationController { nil }
- (id)customizableViewControllers {
    env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers
}
- (())setCustomizableViewControllers:(id)_vcs {}

@end

};

/// Build (and cache, lazily) a UITabBarItem for a UIViewController. Used by
/// the host-side `-[UIViewController tabBarItem]` accessor.
pub fn lazy_tab_bar_item_for_view_controller(
    env: &mut crate::Environment,
    vc: id,
) -> id {
    // Stored on a small ad-hoc host slot in the controller's ivars.
    // Reuses the existing UIViewControllerHostObject's `tab_bar_item` field
    // (added there for this purpose).
    let existing = env
        .objc
        .borrow::<crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject>(vc)
        .tab_bar_item;
    if existing != nil {
        return existing;
    }
    let cls = env.objc.get_known_class("UITabBarItem", &mut env.mem);
    let item: id = msg![env; cls alloc];
    let item: id = msg![env; item init];
    env.objc
        .borrow_mut::<crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject>(vc)
        .tab_bar_item = item;
    item
}
