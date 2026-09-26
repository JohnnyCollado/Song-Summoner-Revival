/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIImageView`, including frame-by-frame animation.

use crate::frameworks::core_graphics::cg_image::CGImageRef;
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{NSInteger, NSTimeInterval, NSUInteger};
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_super, nil, objc_classes, release, retain,
    ClassExports, NSZonePtr,
};
use crate::Environment;
use std::time::Instant;

/// Per-framework state for ticking UIImageView animations from the run loop.
#[derive(Default)]
pub struct State {
    /// `UIImageView*` instances currently animating. Each entry holds a retain
    /// so the view can't be deallocated mid-tick.
    animating: Vec<id>,
}

#[derive(Default)]
struct UIImageViewHostObject {
    superclass: super::UIViewHostObject,
    /// Currently-displayed `UIImage*`.
    image: id,
    /// `NSArray<UIImage*>*`, retained. May be nil if no animation is set.
    animation_images: id,
    /// Total duration of one cycle through `animation_images`.
    animation_duration: NSTimeInterval,
    /// 0 or negative = repeat forever; otherwise stop after N complete cycles.
    animation_repeat_count: NSInteger,
    /// Whether `startAnimating` has been called and `stopAnimating` hasn't.
    is_animating: bool,
    /// Wall-clock instant of the last `startAnimating`.
    animation_start: Option<Instant>,
    /// Index of the frame currently shown (so we don't re-issue setImage: if
    /// the frame hasn't changed since the previous tick).
    current_frame: usize,
}
impl_HostObject_with_superclass!(UIImageViewHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIImageView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIImageViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];
    // Not sure if UIImageView does this unconditionally, or only for images
    // with alpha channels.
    () = msg![env; this setOpaque:false];
    // UIImageView, unlike plain UIView, has userInteractionEnabled = NO by
    // default. Without this override, stale UIImageViews (e.g. animation
    // frames left over after a scene transition) silently swallow taps.
    () = msg![env; this setUserInteractionEnabled:false];
    this
}

- (())dealloc {
    let &UIImageViewHostObject {
        superclass: _,
        image,
        animation_images,
        is_animating,
        ..
    } = env.objc.borrow(this);
    if is_animating {
        // Drop the run-loop retain so the view can finish deallocating.
        env.framework_state
            .uikit
            .ui_view
            .ui_image_view
            .animating
            .retain(|&v| v != this);
    }
    release(env, image);
    release(env, animation_images);
    msg_super![env; this dealloc]
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    let key_ns_string = get_static_str(env, "UIImage");
    let image: id = msg![env; coder decodeObjectForKey:key_ns_string];

    () = msg![env; this setImage:image];
    () = msg![env; this setUserInteractionEnabled:false];

    this
}

- (id)initWithImage:(id)image { // UIImage*
    let size: CGSize = msg![env; image size];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size
    };
    let this = msg_super![env; this initWithFrame:frame];
    () = msg![env; this setImage:image];
    // Not sure if UIImageView does this unconditionally, or only for images
    // with alpha channels.
    () = msg![env; this setOpaque:false];
    () = msg![env; this setUserInteractionEnabled:false];
    this
}

- (id)image {
    env.objc.borrow::<UIImageViewHostObject>(this).image
}

- (())setImage:(id)new_image { // UIImage*
    let host_obj = env.objc.borrow_mut::<UIImageViewHostObject>(this);
    let old_image = std::mem::replace(&mut host_obj.image, new_image);
    retain(env, new_image);
    release(env, old_image);

    let layer: id = msg![env; this layer];
    let cg_image: CGImageRef = msg![env; new_image CGImage];
    () = msg![env; layer setContents:cg_image];
}

- (id)animationImages {
    env.objc.borrow::<UIImageViewHostObject>(this).animation_images
}

- (())setAnimationImages:(id)images { // NSArray<UIImage *>*
    let host_obj = env.objc.borrow_mut::<UIImageViewHostObject>(this);
    let old = std::mem::replace(&mut host_obj.animation_images, images);
    host_obj.current_frame = 0;
    retain(env, images);
    release(env, old);
    // Reset to the first frame so the view looks right even before
    // startAnimating is called.
    if images != nil {
        let count: NSUInteger = msg![env; images count];
        if count > 0 {
            let first: id = msg![env; images objectAtIndex:0u32];
            () = msg![env; this setImage:first];
        }
    }
}

- (NSTimeInterval)animationDuration {
    env.objc.borrow::<UIImageViewHostObject>(this).animation_duration
}

- (())setAnimationDuration:(NSTimeInterval)duration {
    env.objc.borrow_mut::<UIImageViewHostObject>(this).animation_duration = duration;
}

- (NSInteger)animationRepeatCount {
    env.objc.borrow::<UIImageViewHostObject>(this).animation_repeat_count
}

- (())setAnimationRepeatCount:(NSInteger)count {
    env.objc.borrow_mut::<UIImageViewHostObject>(this).animation_repeat_count = count;
}

- (bool)isAnimating {
    env.objc.borrow::<UIImageViewHostObject>(this).is_animating
}

- (())startAnimating {
    let host_obj = env.objc.borrow::<UIImageViewHostObject>(this);
    if host_obj.is_animating {
        return;
    }
    if host_obj.animation_images == nil || host_obj.animation_duration <= 0.0 {
        // Nothing to animate or no frame rate set — bail without bookkeeping.
        return;
    }
    let now = Instant::now();
    {
        let host_obj = env.objc.borrow_mut::<UIImageViewHostObject>(this);
        host_obj.is_animating = true;
        host_obj.animation_start = Some(now);
        host_obj.current_frame = 0;
    }
    let already_registered = env
        .framework_state
        .uikit
        .ui_view
        .ui_image_view
        .animating
        .contains(&this);
    if !already_registered {
        env.framework_state
            .uikit
            .ui_view
            .ui_image_view
            .animating
            .push(this);
        retain(env, this);
    }
    log_dbg!("[(UIImageView*){:?} startAnimating]", this);
}

- (())stopAnimating {
    let was_animating = env.objc.borrow::<UIImageViewHostObject>(this).is_animating;
    if !was_animating {
        return;
    }
    env.objc.borrow_mut::<UIImageViewHostObject>(this).is_animating = false;
    let was_registered = env
        .framework_state
        .uikit
        .ui_view
        .ui_image_view
        .animating
        .iter()
        .any(|&v| v == this);
    if was_registered {
        env.framework_state
            .uikit
            .ui_view
            .ui_image_view
            .animating
            .retain(|&v| v != this);
        release(env, this);
    }
    log_dbg!("[(UIImageView*){:?} stopAnimating]", this);
}

@end

};

/// Tick every animating `UIImageView`, swapping its displayed image to the
/// frame at the current point in the animation cycle. Should be called from
/// the main run loop once per iteration. Stops animations that have reached
/// their `animationRepeatCount`.
pub fn handle_animations(env: &mut Environment) {
    let now = Instant::now();
    let views = env
        .framework_state
        .uikit
        .ui_view
        .ui_image_view
        .animating
        .clone();
    for view in views {
        let host = env.objc.borrow::<UIImageViewHostObject>(view);
        let Some(start) = host.animation_start else { continue; };
        let images = host.animation_images;
        let duration = host.animation_duration;
        let repeat = host.animation_repeat_count;
        let prev_frame = host.current_frame;
        if images == nil || duration <= 0.0 {
            continue;
        }
        let count: NSUInteger = msg![env; images count];
        if count == 0 {
            continue;
        }
        let frame_count = count as usize;
        let frame_dur = duration / (frame_count as f64);
        let elapsed = now.duration_since(start).as_secs_f64();
        let total_frames = (elapsed / frame_dur).floor() as i64;
        let completed_cycles = total_frames / (frame_count as i64);

        if repeat > 0 && completed_cycles >= repeat as i64 {
            // Animation finished its scheduled repeats — stop and leave the
            // last frame showing.
            let last_idx = frame_count - 1;
            if prev_frame != last_idx {
                let last: id = msg![env; images objectAtIndex:(last_idx as NSUInteger)];
                () = msg![env; view setImage:last];
                env.objc.borrow_mut::<UIImageViewHostObject>(view).current_frame = last_idx;
            }
            () = msg![env; view stopAnimating];
            continue;
        }

        let current_idx = (total_frames as usize) % frame_count;
        if current_idx != prev_frame {
            let img: id = msg![env; images objectAtIndex:(current_idx as NSUInteger)];
            () = msg![env; view setImage:img];
            env.objc.borrow_mut::<UIImageViewHostObject>(view).current_frame = current_idx;
        }
    }
}
