/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIViewController`.
//!
//! Resources:
//! - [View Controller Programming Guide for iOS (Legacy)](https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/ViewControllerPGforiOSLegacy/BasicViewControllers/BasicViewControllers.html)

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::ns_objc_runtime::NSStringFromClass;
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_application::{
    UIInterfaceOrientation, UIInterfaceOrientationPortrait,
};
use crate::frameworks::uikit::ui_view::set_view_controller;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, todo_objc_setter, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;

pub mod ui_navigation_controller;

#[derive(Default)]
pub struct UIViewControllerHostObject {
    /// The root view.
    /// `UIView*`
    view: id,
    /// Nib name to be used at the load
    /// of the root view, may be nil.
    /// `NSString*`
    nib_name: id,
    /// Bundle to be used for load
    /// of the nib by name, may be nil.
    /// `NSBundle*`
    bundle: id,
    /// The view controller currently presented modally by this one, retained.
    /// `UIViewController*`
    modal_view_controller: id,
    /// Lazy `UITabBarItem*`, retained. Populated on first `-tabBarItem` call.
    pub tab_bar_item: id,
    /// Lazy `UINavigationItem*`, retained. Populated on first `-navigationItem` call.
    pub navigation_item: id,
    /// `NSString*`, retained.
    title: id,
}
impl HostObject for UIViewControllerHostObject {}

type UIModalTransitionStyle = NSInteger;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIViewController: UIResponder

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// TODO: this should be a designated initializer
- (id)initWithNibName:(id)nib_name // NSString *
               bundle:(id)bundle { // NSBundle *
    retain(env, nib_name);
    retain(env, bundle);

    log_dbg!("[(UIViewController*){:?} initWithNibName:{:?} bundle:{:?}]", this, nib_name, bundle);

    env.objc.borrow_mut::<UIViewControllerHostObject>(this).nib_name = nib_name;
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).bundle = bundle;

    this
}

- (id)initWithCoder:(id)coder {
    let key_ns_string = get_static_str(env, "UIView");
    let view: id = msg![env; coder decodeObjectForKey:key_ns_string];

    () = msg![env; this setView:view];

    this
}

- (())dealloc {
    let &UIViewControllerHostObject {
        view, nib_name, bundle, modal_view_controller, tab_bar_item,
        navigation_item, title
    } = env.objc.borrow(this);

    if view != nil {
        set_view_controller(env, view, nil);
    }
    release(env, view);
    release(env, nib_name);
    release(env, bundle);
    release(env, modal_view_controller);
    release(env, tab_bar_item);
    release(env, navigation_item);
    release(env, title);

    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)tabBarItem {
    super::ui_view::ui_tab_bar::lazy_tab_bar_item_for_view_controller(env, this)
}

- (id)navigationItem {
    let existing = env.objc.borrow::<UIViewControllerHostObject>(this).navigation_item;
    if existing != nil {
        return existing;
    }
    let cls = env.objc.get_known_class("UINavigationItem", &mut env.mem);
    let item: id = msg![env; cls alloc];
    let item: id = msg![env; item init];
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).navigation_item = item;
    item
}
- (())setTabBarItem:(id)item {
    retain(env, item);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewControllerHostObject>(this).tab_bar_item,
        item,
    );
    release(env, old);
}

- (id)tabBarController { nil }
- (id)navigationController { nil }
- (id)parentViewController { nil }

- (())loadView {
    let bundle: id = env.objc.borrow::<UIViewControllerHostObject>(this).bundle;
    let bundle: id = if bundle == nil {
        msg_class![env; NSBundle mainBundle]
    } else {
        bundle
    };

    let nib_name: id = get_nib_name(env, this, bundle);
    if nib_name != nil {
        // If we do have nib name, try to load it!
        log_dbg!(
            "Load {:?} view controller's view by nib, using name {}", this, to_rust_string(env, nib_name)
        );

        let nib: id = msg_class![env; UINib nibWithNibName:nib_name bundle:bundle];
        release(env, nib_name);

        // The NIB's File's Owner will be substituted by `this`,
        // implicitly loading the view as well
        let _: id = msg![env; nib instantiateWithOwner:this options:nil];

        let view = env.objc.borrow::<UIViewControllerHostObject>(this).view;
        // Having nil view at this point probably mean that
        // out nib's parsing is wrong.
        // Also we assume here the case of a "detached nib file"
        // TODO: support "integrated nib file"
        assert!(view != nil);

        return;
    };

    // As a last resort, use plain UIVIew for the root view
    let class: Class = msg![env; this class];
    log!("Unable to load {:?} {} view controller's view by nib, using plain UIView", this, env.objc.get_class_name(class).to_string());
    let view: id = msg_class![env; UIView alloc];
    // Docs are saying that "an empty UIView" is created,
    // but testing reveals that frame matches the screen one
    // (at least on the simulator)
    let screen: id = msg_class![env; UIScreen mainScreen];
    let app_frame: CGRect = msg![env; screen applicationFrame];
    let view: id = msg![env; view initWithFrame:app_frame];
    () = msg![env; this setView:view];
}

- (())setView:(id)new_view { // UIView*
    let host_obj = env.objc.borrow_mut::<UIViewControllerHostObject>(this);
    let old_view = std::mem::replace(&mut host_obj.view, new_view);
    if old_view != nil {
        set_view_controller(env, old_view, nil);
    }
    if new_view != nil {
        set_view_controller(env, new_view, this);
    }
    retain(env, new_view);
    release(env, old_view);
}
- (id)view {
    let view = env.objc.borrow_mut::<UIViewControllerHostObject>(this).view;
    if view == nil {
        () = msg![env; this loadView];
        let view = env.objc.borrow_mut::<UIViewControllerHostObject>(this).view;
        () = msg![env; this viewDidLoad];
        view
    } else {
        view
    }
}

// Usually overridden by the application
- (())viewDidLoad {
    log_dbg!("[(UIViewController*){:?} viewDidLoad]", this);
}
- (())viewWillAppear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewWillAppear:{}]", this, animated);
}
- (())viewDidAppear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewDidAppear:{}]", this, animated);
}
- (())viewWillDisappear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewWillDisappear:{}]", this, animated);
}
- (())viewDidDisappear:(bool)animated {
    log_dbg!("[(UIViewController*){:?} viewDidDisappear:{}]", this, animated);
}

- (id)title {
    env.objc.borrow::<UIViewControllerHostObject>(this).title
}
- (())setTitle:(id)title { // NSString *
    let copied: id = if title == nil { nil } else { msg![env; title copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewControllerHostObject>(this).title,
        copied,
    );
    release(env, old);
}
- (())setEditing:(bool)editing {
    todo_objc_setter!(this, editing);
}
- (())setWantsFullScreenLayout:(bool)wants {
    todo_objc_setter!(this, wants);
}
- (())setHidesBottomBarWhenPushed:(bool)hides {
    todo_objc_setter!(this, hides);
}
- (())setModalTransitionStyle:(UIModalTransitionStyle)style {
    todo_objc_setter!(this, style);
}

// Modal view controller presentation. This is a minimal implementation: there
// are no transition animations, and the presented controller's view simply
// covers the key window until it is dismissed.
- (())presentModalViewController:(id)modal_vc // UIViewController*
                        animated:(bool)animated {
    let cls: Class = msg![env; modal_vc class];
    log!(
        "[(UIViewController*){:?} presentModalViewController:{:?}({}) animated:{}]",
        this, modal_vc, env.objc.get_class_name(cls), animated
    );

    let already_presented =
        env.objc.borrow::<UIViewControllerHostObject>(this).modal_view_controller;
    if already_presented != nil {
        // UIKit only allows one modal at a time; dismiss the previous one.
        () = msg![env; this dismissModalViewControllerAnimated:false];
    }

    retain(env, modal_vc);
    env.objc.borrow_mut::<UIViewControllerHostObject>(this).modal_view_controller = modal_vc;

    let view: id = msg![env; modal_vc view];
    () = msg![env; modal_vc viewWillAppear:animated];
    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window != nil && view != nil {
        () = msg![env; window addSubview:view];

        // UIWindow's addSubview: auto-rotates direct-subview views to match
        // the app's interface orientation, but it gates this on
        // shouldAutorotateToInterfaceOrientation: returning true on the
        // attached view-controller. Many iOS 3.x apps (Song Summoner's
        // picker, in particular) don't override that selector on the modal
        // controller class, so the modal lands in portrait while the rest
        // of the app is landscape — UI ends up rotated 90°. Force the
        // landscape transform here so the picker renders the same way the
        // game's main view does.
        if let Some(angle) = env.window.as_ref().and_then(|w| match w.current_rotation() {
            crate::window::DeviceOrientation::LandscapeLeft => {
                Some(-std::f32::consts::FRAC_PI_2)
            }
            crate::window::DeviceOrientation::LandscapeRight => {
                Some(std::f32::consts::FRAC_PI_2)
            }
            crate::window::DeviceOrientation::Portrait => None,
        }) {
            use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
            let transform = CGAffineTransform::make_rotation(angle);
            let window_frame: CGRect = msg![env; window frame];
            () = msg![env; view setTransform:transform];
            () = msg![env; view setFrame:window_frame];
        }
    }
    () = msg![env; modal_vc viewDidAppear:animated];
}
- (id)modalViewController {
    env.objc.borrow::<UIViewControllerHostObject>(this).modal_view_controller
}

- (())dismissModalViewControllerAnimated:(bool)animated {
    let modal_vc =
        env.objc.borrow::<UIViewControllerHostObject>(this).modal_view_controller;
    if modal_vc == nil {
        log_dbg!(
            "[(UIViewController*){:?} dismissModalViewControllerAnimated:{}] (nothing presented)",
            this, animated
        );
        return;
    }
    log_dbg!(
        "[(UIViewController*){:?} dismissModalViewControllerAnimated:{}]",
        this, animated
    );

    () = msg![env; modal_vc viewWillDisappear:animated];
    let view: id = msg![env; modal_vc view];
    () = msg![env; view removeFromSuperview];
    () = msg![env; modal_vc viewDidDisappear:animated];

    env.objc.borrow_mut::<UIViewControllerHostObject>(this).modal_view_controller = nil;
    release(env, modal_vc);
}
- (())dismissMoviePlayerViewControllerAnimated {
    log!("TODO: [(UIViewController*){:?} dismissMoviePlayerViewControllerAnimated]", this); // TODO
}

- (bool)shouldAutorotateToInterfaceOrientation:(UIInterfaceOrientation)interface_orientation {
    interface_orientation == UIInterfaceOrientationPortrait
}

// UIResponder implementation
// From the Apple UIView docs regarding [UIResponder nextResponder]:
// "UIViewController similarly implements the method
// and returns its view’s superview."
// https://developer.apple.com/documentation/uikit/uiresponder/next?language=objc
- (id)nextResponder {
    let view = msg![env; this view];
    let next_responder = msg![env; view superview];
    log_dbg!("[(UIView*){:?} nextResponder] => {:?}", this, next_responder);
    next_responder
}

@end

};

/// A helper function to resolve suitable NIB name for a `view_controller`
/// in the `bundle`. Returns nil if fails.
///
/// Note: It's a responsibility of a caller to release the returned name
/// if not-nil!
fn get_nib_name(env: &mut Environment, view_controller: id, bundle: id) -> id {
    let provider_nib_name: id = env
        .objc
        .borrow::<UIViewControllerHostObject>(view_controller)
        .nib_name;
    if provider_nib_name != nil {
        // TODO: it's not clear how to handle situation when
        // provided nib name do not exist in the bundle.
        // It probably means that our bundle resource loading
        // is faulty, to check
        assert!(check_nib_exists(env, bundle, provider_nib_name));

        retain(env, provider_nib_name);
        return provider_nib_name;
    };

    let class: Class = msg![env; view_controller class];
    let class_name: id = NSStringFromClass(env, class);
    let class_name_str = to_rust_string(env, class_name);

    if let Some(name) = class_name_str.strip_suffix("Controller") {
        let ns_name: id = from_rust_string(env, name.to_string());
        if check_nib_exists(env, bundle, ns_name) {
            release(env, class_name);
            return ns_name;
        }
    }

    if check_nib_exists(env, bundle, class_name) {
        class_name
    } else {
        release(env, class_name);
        nil
    }
}

/// A helper function to check if `nib_name` NIB actually
/// existing in the `bundle`
fn check_nib_exists(env: &mut Environment, bundle: id, nib_name: id) -> bool {
    let type_: id = get_static_str(env, "nib");
    let res: id = msg![env; bundle pathForResource:nib_name ofType:type_];
    res != nil
}
