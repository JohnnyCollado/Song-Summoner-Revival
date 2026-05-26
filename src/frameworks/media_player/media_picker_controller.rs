/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaPickerController`.
//!
//! touchHLE provides a built-in set of fake songs (see [super::media_item]),
//! so when an app presents this picker we synthesize a selection on its behalf
//! and deliver `mediaPicker:didPickMediaItems:` to the delegate. Apps that
//! lack a delegate (or whose delegate doesn't handle the pick callback) still
//! get a `mediaPickerDidCancel:` fallback so they can dismiss the picker.

use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_super, nil, objc_classes, release, retain,
    ClassExports, NSZonePtr, SEL,
};
use crate::Environment;
use super::{media_item, media_item_collection};

type MPMediaType = NSUInteger;
const MPMediaTypeAny: MPMediaType = !0;

/// State for the run-loop-driven cancellation of presented pickers.
#[derive(Default)]
pub struct State {
    /// Pickers that have been presented and whose delegates still need to be
    /// told the picker was cancelled. Each entry holds a retain.
    pending_cancels: Vec<id>,
}

/// Captured MPMediaPickerController delegate (iPodView2 instance in Song
/// Summoner). The picker-swap dispatch in ui_table_view.rs invokes
/// `mediaPicker:didPickMediaNumber:` here directly on pick 2+, bypassing
/// IPDSongsTab's early-exit `didSelectRow` so the confirmation panel rebuilds.
pub static PICKER_DELEGATE: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);
pub static PICKER_CONTROLLER: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

#[derive(Default)]
struct MPMediaPickerControllerHostObject {
    superclass: UIViewControllerHostObject,
    media_types: MPMediaType,
    /// `id<MPMediaPickerControllerDelegate>`, a weak (non-retaining) reference.
    delegate: id,
    allows_picking_multiple_items: bool,
    /// `NSString*`, retained.
    prompt: id,
}
impl_HostObject_with_superclass!(MPMediaPickerControllerHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaPickerController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<MPMediaPickerControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    msg![env; this initWithMediaTypes:MPMediaTypeAny]
}

- (id)initWithMediaTypes:(MPMediaType)media_types {
    log!(
        "[(MPMediaPickerController*){:?} initWithMediaTypes:{:#x}]",
        this, media_types
    );
    env.objc.borrow_mut::<MPMediaPickerControllerHostObject>(this).media_types = media_types;
    this
}

- (())dealloc {
    let prompt = env.objc.borrow::<MPMediaPickerControllerHostObject>(this).prompt;
    release(env, prompt);
    // UIViewController's dealloc frees the underlying object.
    msg_super![env; this dealloc]
}

- (MPMediaType)mediaTypes {
    env.objc.borrow::<MPMediaPickerControllerHostObject>(this).media_types
}

// The delegate is a weak reference, matching UIKit/MediaPlayer semantics.
- (())setDelegate:(id)delegate { // id<MPMediaPickerControllerDelegate>
    log!("[(MPMediaPickerController*){:?} setDelegate:{:?}]", this, delegate);
    env.objc.borrow_mut::<MPMediaPickerControllerHostObject>(this).delegate = delegate;
    // Capture for picker-swap: ui_table_view.rs calls
    // `mediaPicker:didPickMediaNumber:` on this delegate to force the game's
    // confirmation panel to rebuild on pick 2+.
    *PICKER_DELEGATE.lock().unwrap() = Some(delegate.to_bits());
    *PICKER_CONTROLLER.lock().unwrap() = Some(this.to_bits());
}
- (id)delegate {
    env.objc.borrow::<MPMediaPickerControllerHostObject>(this).delegate
}

- (())setAllowsPickingMultipleItems:(bool)value {
    env.objc.borrow_mut::<MPMediaPickerControllerHostObject>(this)
        .allows_picking_multiple_items = value;
}
- (bool)allowsPickingMultipleItems {
    env.objc.borrow::<MPMediaPickerControllerHostObject>(this).allows_picking_multiple_items
}

- (())setPrompt:(id)prompt { // NSString*
    let new_prompt: id = if prompt == nil {
        nil
    } else {
        msg![env; prompt copy]
    };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaPickerControllerHostObject>(this).prompt,
        new_prompt,
    );
    release(env, old);
}
- (id)prompt {
    env.objc.borrow::<MPMediaPickerControllerHostObject>(this).prompt
}

- (())viewDidAppear:(bool)animated {
    log!("[(MPMediaPickerController*){:?} viewDidAppear:{}]", this, animated);
    // There is no media library to browse, so schedule a cancellation to be
    // delivered to the delegate once the app returns to the run loop.
    let already_pending = env
        .framework_state
        .media_player
        .media_picker
        .pending_cancels
        .contains(&this);
    if !already_pending {
        env.framework_state
            .media_player
            .media_picker
            .pending_cancels
            .push(this);
        retain(env, this);
    }
}

@end

};

/// For use by `NSRunLoop` via [super::handle_players]: deliver a synthetic
/// pick to the delegate of any picker that has been presented since the last
/// run loop iteration. If the delegate doesn't implement the pick callback we
/// fall back to `mediaPickerDidCancel:` so the app still dismisses the picker.
pub(super) fn handle_players(env: &mut Environment) {
    let pending = std::mem::take(
        &mut env.framework_state.media_player.media_picker.pending_cancels,
    );
    for picker in pending {
        let delegate: id = env
            .objc
            .borrow::<MPMediaPickerControllerHostObject>(picker)
            .delegate;
        if delegate != nil {
            // Build a one-song collection from our fake library so the app
            // has something concrete to summon with. The item goes into the
            // array owned (not autoreleased) per ns_array::from_vec's contract.
            let item = media_item::make_item_owned(env, 0);
            let items_arr = crate::frameworks::foundation::ns_array::from_vec(env, vec![item]);
            let items_arr =
                crate::objc::autorelease(env, items_arr);
            let collection = media_item_collection::make_collection(env, items_arr);

            let pick_sel: SEL = env.objc.register_host_selector(
                "mediaPicker:didPickMediaItems:".to_string(),
                &mut env.mem,
            );
            if msg![env; delegate respondsToSelector:pick_sel] {
                log!(
                    "Auto-picking first fake song for MPMediaPickerController {:?}",
                    picker
                );
                () = msg![env; delegate mediaPicker:picker didPickMediaItems:collection];
            } else {
                let cancel_sel: SEL = env.objc.register_host_selector(
                    "mediaPickerDidCancel:".to_string(),
                    &mut env.mem,
                );
                if msg![env; delegate respondsToSelector:cancel_sel] {
                    log!(
                        "Delegate has no didPickMediaItems:; cancelling MPMediaPickerController {:?}",
                        picker
                    );
                    () = msg![env; delegate mediaPickerDidCancel:picker];
                } else {
                    log!(
                        "Warning: MPMediaPickerController delegate {:?} implements neither \
                         pick nor cancel; the app may not dismiss the picker.",
                        delegate
                    );
                }
            }
        }
        release(env, picker);
    }
}
