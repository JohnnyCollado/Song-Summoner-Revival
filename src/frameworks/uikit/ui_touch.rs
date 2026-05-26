/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITouch`.

use super::ui_event;
use crate::frameworks::core_graphics::{CGPoint, CGRect};
use crate::frameworks::foundation::{NSInteger, NSTimeInterval, NSUInteger};
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::window::{Coords, Event, FingerId};
use crate::Environment;
use std::collections::hash_map::{Entry, HashMap};
use std::collections::HashSet;

pub type UITouchPhase = NSInteger;
pub const UITouchPhaseBegan: UITouchPhase = 0;
pub const UITouchPhaseMoved: UITouchPhase = 1;
pub const UITouchPhaseStationary: UITouchPhase = 2;
pub const UITouchPhaseEnded: UITouchPhase = 3;

#[derive(Default)]
pub struct State {
    current_touches: HashMap<FingerId, id>,
}

pub(super) struct UITouchHostObject {
    /// Strong reference to the `UIView`
    pub(super) view: id,
    /// Strong reference to the `UIWindow`, used as a reference for co-ordinate
    /// space conversion
    pub(super) window: id,
    /// Relative to the screen
    location: CGPoint,
    /// Relative to the screen
    previous_location: CGPoint,
    timestamp: NSTimeInterval,
    phase: UITouchPhase,
}
impl HostObject for UITouchHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITouch: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITouchHostObject {
        view: nil,
        window: nil,
        location: CGPoint { x: 0.0, y: 0.0 },
        previous_location: CGPoint { x: 0.0, y: 0.0 },
        timestamp: 0.0,
        phase: UITouchPhaseBegan,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let &mut UITouchHostObject { view, window, .. } = env.objc.borrow_mut(this);
    release(env, view);
    release(env, window);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (CGPoint)locationInView:(id)that_view { // UIView*
    let &UITouchHostObject { location, window, .. } = env.objc.borrow(this);
    let location_in_window: CGPoint = msg![env; window convertPoint:location fromWindow:nil];
    if that_view == nil {
        location_in_window
    } else {
        msg![env; that_view convertPoint:location_in_window fromView:window]
    }
}
- (CGPoint)previousLocationInView:(id)that_view { // UIView*
    let &UITouchHostObject { previous_location, window, .. } = env.objc.borrow(this);
    let location_in_window: CGPoint = msg![env; window convertPoint:previous_location fromWindow:nil];
    if that_view == nil {
        location_in_window
    } else {
        msg![env; that_view convertPoint:location_in_window fromView:window]
    }
}

- (id)view {
    env.objc.borrow::<UITouchHostObject>(this).view
}

- (NSTimeInterval)timestamp {
    env.objc.borrow::<UITouchHostObject>(this).timestamp
}

- (NSUInteger)tapCount {
    1 // TODO: support double-taps etc
}

- (UITouchPhase)phase {
    env.objc.borrow::<UITouchHostObject>(this).phase
}

@end

};

/// [super::handle_events] will forward touch events to this function.
pub fn handle_event(env: &mut Environment, event: Event) {
    // before processing anything, we mark all current touches as stationary
    let current_touches = &env.framework_state.uikit.ui_touch.current_touches;
    for &touch in (*current_touches).values() {
        env.objc.borrow_mut::<UITouchHostObject>(touch).phase = UITouchPhaseStationary;
    }
    match event {
        Event::TouchesDown(map) => handle_touches_down(env, map),
        Event::TouchesMove(map) => handle_touches_move(env, map),
        Event::TouchesUp(map) => handle_touches_up(env, map),
        _ => unreachable!(),
    }
}

fn handle_touches_down(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    // UIKit creates and drains autorelease pools when handling events.
    let pool: id = msg_class![env; NSAutoreleasePool new];


    // Note: if the emulator is heavily lagging, this timestamp is going
    // to be far off from the truth, since it should represent the
    // time when the event actually happened, not the time when the
    // event was dispatched. Maybe we'll need to fix this eventually.
    let timestamp: NSTimeInterval = {
        let process_info = msg_class![env; NSProcessInfo processInfo];
        msg![env; process_info systemUptime]
    };

    let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];

    for (finger_id, coords) in map {
        let current_touches = &mut env.framework_state.uikit.ui_touch.current_touches;

        if current_touches.contains_key(&finger_id) {
            // this seems to happen only on the desktop with a single touch
            assert_eq!(current_touches.len(), 1);
            log!(
                "Warning: New touch {:?} initiated but current touch did not end yet, treating as movement.",
                finger_id
            );
            return handle_touches_move(env, HashMap::from([(finger_id, coords)]));
        }

        log_dbg!("Finger {:?} touch down: {:?}", finger_id, coords);

        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };

        // TODO: is this the correct state of the UITouch and UIEvent during
        //       hit testing?

        let new_touch: id = msg_class![env; UITouch alloc];
        *env.objc.borrow_mut(new_touch) = UITouchHostObject {
            view: nil,
            window: nil,
            location,
            previous_location: location,
            timestamp,
            phase: UITouchPhaseBegan,
        };
        autorelease(env, new_touch);

        let _: () = msg![env; touches addObject:new_touch];

        let _ = &env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .insert(finger_id, new_touch);
        retain(env, new_touch);
    }

    let all_touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
    for &touch in env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .clone()
        .values()
    {
        let _: () = msg![env; all_touches addObject:touch];
    }

    let event = ui_event::new_event(env, all_touches);
    autorelease(env, event);

    // views with existing touches (see isMultipleTouchEnabled check below)
    let views_with_existing_touches: HashSet<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .map(|&touch| env.objc.borrow::<UITouchHostObject>(touch).view)
        .collect();

    // view to set of touches for this view
    let mut view_touches: HashMap<id, id> = HashMap::new();

    let touches_arr: id = msg![env; touches allObjects];
    let touches_count: NSUInteger = msg![env; touches_arr count];
    for i in 0..touches_count {
        let touch: id = msg![env; touches_arr objectAtIndex:i];
        let &UITouchHostObject { location, .. } = env.objc.borrow(touch);

        // Assumes the windows in the list are ordered back-to-front.
        // TODO: this may not be correct once we support windowLevel.
        let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
        let Some((window, location_in_window)) = windows.into_iter().rev().find_map(|window| {
            let location_in_window: CGPoint =
                msg![env; window convertPoint:location fromWindow:nil];
            if msg![env; window pointInside:location_in_window withEvent:event] {
                Some((window, location_in_window))
            } else {
                None
            }
        }) else {
            log!(
                "Couldn't find a window for touch at {:?}, discarding",
                location,
            );
            continue;
        };

        let view: id = msg![env; window hitTest:location_in_window withEvent:event];
        if view == nil {
            log!(
                "Couldn't find a view for touch at {:?} in window {:?}, discarding",
                location_in_window,
                window,
            );
            continue;
        } else {
            log_dbg!(
                "Found view {:?} with frame {:?} for touch at {:?} in window {:?}",
                view,
                {
                    let f: CGRect = msg![env; view frame];
                    f
                },
                location_in_window,
                window,
            );
        }

        let is_multi_touch_enabled: bool = msg![env; view isMultipleTouchEnabled];
        if !is_multi_touch_enabled {
            // When a view has multi-touch disabled, it can only have one active
            // touch at once. So, we can only report a new touch to the view if
            // there are no other touches currently associated with it, and if
            // there are multiple new touches for this view, we can only report
            // one of them.
            let view_has_other_new_touches = view_touches.contains_key(&view);
            let view_has_existing_touches = views_with_existing_touches.contains(&view);
            if view_has_other_new_touches || view_has_existing_touches {
                log!(
                    "Ignoring new touch {:?} for view {:?}, !isMultipleTouchEnabled",
                    touch,
                    view
                );
                // The touch will continue to be tracked until it ends, but the
                // view will be nil, so messages sent to it will be ignored.
                // TODO: Figure out if/how these should be delivered elsewhere
                //       in the responder chain.
                // FIXME: The fact the view is nil might be observed via
                //        touchesForView:nil or allTouches on UIEvent.
                //        This might cause problems. What does the real OS do?
                //        Does this need to be prevented?
                continue;
            }
        }

        // Only create the set after the isMultipleTouchEnabled checks so we
        // won't end up with an empty set.
        if let Entry::Vacant(e) = view_touches.entry(view) {
            let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
            e.insert(touches);
        }
        let touches: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; touches addObject:touch];

        retain(env, view);
        retain(env, window);
        {
            let new_touch = env.objc.borrow_mut::<UITouchHostObject>(touch);
            new_touch.view = view;
            new_touch.window = window;
        }
    }

    for (view, touches) in view_touches {
        log_dbg!(
            "Sending [{:?} touchesBegan:{:?} withEvent:{:?}]",
            view,
            touches,
            event
        );
        let _: () = msg![env; view touchesBegan:touches withEvent:event];
    }

    release(env, pool);
}

fn handle_touches_move(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    let pool: id = msg_class![env; NSAutoreleasePool new];

    let timestamp: NSTimeInterval = {
        let process_info = msg_class![env; NSProcessInfo processInfo];
        msg![env; process_info systemUptime]
    };

    let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];

    // view to set of touches for this view
    let mut view_touches: HashMap<id, id> = HashMap::new();

    for (finger_id, coords) in map {
        let Some(&touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .get(&finger_id)
        else {
            log!(
                "Warning: Finger {:?} touch move event received but no current touch, ignoring.",
                finger_id
            );
            continue;
        };

        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };

        let view = env.objc.borrow::<UITouchHostObject>(touch).view;
        let host_object = env.objc.borrow_mut::<UITouchHostObject>(touch);

        if host_object.location == location {
            continue;
        }

        log_dbg!("Finger {:?} touch move: {:?}", finger_id, coords);

        host_object.previous_location = host_object.location;
        host_object.location = location;
        host_object.timestamp = timestamp;
        assert_eq!(host_object.phase, UITouchPhaseStationary);
        host_object.phase = UITouchPhaseMoved;

        let _: () = msg![env; touches addObject:touch];

        if let Entry::Vacant(e) = view_touches.entry(view) {
            let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
            e.insert(touches);
        }
        let touches: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; touches addObject:touch];
    }

    let all_touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
    for &touch in env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .clone()
        .values()
    {
        let _: () = msg![env; all_touches addObject:touch];
    }

    let event = ui_event::new_event(env, all_touches);
    autorelease(env, event);

    for (view, touches) in view_touches {
        log_dbg!(
            "Sending [{:?} touchesMoved:{:?} withEvent:{:?}]",
            view,
            touches,
            event
        );
        let _: () = msg![env; view touchesMoved:touches withEvent:event];
    }

    release(env, pool);
}

fn handle_touches_up(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    let pool: id = msg_class![env; NSAutoreleasePool new];

    // Capture this touch-up's iOS-canvas coordinates up front so we still
    // have them after the for-loop below consumes `map`. Used by the
    // post-dispatch classifier that decides whether to restore the picker
    // (tap on No) or leave it hidden (tap on Create Trooper / elsewhere).
    let touch_up_xy: Option<(f32, f32)> =
        map.iter().next().map(|(_, c)| (c.0, c.1));

    let timestamp: NSTimeInterval = {
        let process_info = msg_class![env; NSProcessInfo processInfo];
        msg![env; process_info systemUptime]
    };

    let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];

    // We need to construct all touches set _BEFORE_ removing touches!
    // (as removed one are reported as the part of the event)
    let all_touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
    for &touch in env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .clone()
        .values()
    {
        let _: () = msg![env; all_touches addObject:touch];
    }

    // view to set of touches for this view
    let mut view_touches: HashMap<id, id> = HashMap::new();

    for (finger_id, coords) in map {
        let Some(&touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .get(&finger_id)
        else {
            log!(
                "Warning: Finger {:?} touch up event received but no current touch, ignoring.",
                finger_id
            );
            continue;
        };

        log_dbg!("Finger {:?} touch up: {:?}", finger_id, coords);

        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };

        let view = env.objc.borrow::<UITouchHostObject>(touch).view;
        let host_object = env.objc.borrow_mut::<UITouchHostObject>(touch);
        host_object.previous_location = host_object.location;
        host_object.location = location;
        host_object.timestamp = timestamp;
        assert_eq!(host_object.phase, UITouchPhaseStationary);
        host_object.phase = UITouchPhaseEnded;

        let _: () = msg![env; touches addObject:touch];

        if let Entry::Vacant(e) = view_touches.entry(view) {
            let touches: id = msg_class![env; NSMutableSet allocWithZone:(MutVoidPtr::null())];
            e.insert(touches);
        }
        let touches: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; touches addObject:touch];

        let _ = &env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .remove(&finger_id);
        release(env, touch); // only owner now should be the NSSet
    }

    let event = ui_event::new_event(env, all_touches);
    autorelease(env, event);

    // Snapshot the promoted picker table's hidden state and the touch
    // coordinates, then classify the tap by location once dispatch
    // returns. The Create Trooper / No screen is drawn by the game via
    // OpenGL (not UIViews), so the only signal we have is where the user
    // tapped on the simulated 480x320 touchscreen.
    let promoted_table_pre: crate::objc::id = {
        let bits = *crate::frameworks::uikit::ui_view::ui_table_view::PROMOTED_TABLE
            .lock()
            .unwrap();
        match bits {
            Some(b) => crate::objc::id::from_bits(b),
            None => crate::objc::nil,
        }
    };
    let was_hidden_pre: bool = if promoted_table_pre != crate::objc::nil {
        msg![env; promoted_table_pre isHidden]
    } else {
        false
    };

    // Suppress touch dispatch entirely while the confirmation panel is up
    // (was_hidden_pre = our promoted picker is hidden). The game renders
    // the confirmation as cell 0 of IPDSongsTab — if we forwarded the
    // touch normally, the cell would receive a tap and the game's
    // didSelectRow would fire a SECOND time with row=0 (re-picking the
    // wrong song). We only want our own classifier below to act on this
    // touch, picking exactly one of: No / Create Trooper / no-op.
    if !was_hidden_pre {
        for (view, touches) in view_touches {
            log_dbg!(
                "Sending [{:?} touchesEnded:{:?} withEvent:{:?}]",
                view,
                touches,
                event
            );
            let _: () = msg![env; view touchesEnded:touches withEvent:event];
        }
    } else {
        log_dbg!(
            "ui_touch: suppressing dispatch for touch-up while picker is hidden (confirmation up)"
        );
    }

    // Post-dispatch: if the table was hidden before this touch-up and the
    // user tapped *on the No button* on the Create Trooper confirmation
    // panel, restore the picker. Taps on Create Trooper (game progresses
    // to next scene) or anywhere else (user still deciding) leave the
    // picker hidden — the game covers it with its next scene anyway, and
    // restoring on every tap would briefly flash the picker through other
    // game screens.
    //
    // Bounds are in the 480x320 iOS-landscape canvas, calibrated from the
    // confirmation-screen layout (see dev-docs notes / screenshot). Tweak
    // here if the game's UI shifts.
    // Calibrated from user-reported tap coords. The user pressed No at
    // landscape y=238 and 227, both of which fall in what I'd initially
    // estimated as the Create Trooper region. The dividing line between
    // the two buttons is around y=225, not y=245 as the marked screenshot
    // visually suggested. Bias towards No so it wins the boundary --
    // pressing Create Trooper is destructive (game advances past picker),
    // pressing No is recoverable.
    // Hit-boxes calibrated by user-driven diagonal drags (logged via the
    // touch-down + touch-up diagnostics). Measured edges:
    //   Create Trooper: (268..473, 158..195)
    //   No:             (266..471, 211..252)
    // There's a 16px dead-zone between them (y=195..211). Bias the
    // boundary toward No (extend it up into the gap) since Create Trooper
    // dismisses the picker permanently and No is recoverable.
    const CREATE_TROOPER_BOUNDS:   (f32, f32, f32, f32) = (262.0, 152.0, 478.0, 200.0);
    const NO_BUTTON_BOUNDS:        (f32, f32, f32, f32) = (262.0, 200.0, 478.0, 258.0);
    // Bottom-right back-icon — returns user to Soul Master (parent menu).
    // Calibrated from user-reported taps at landscape (471, 312) and (457, 293).
    const BACK_BUTTON_BOUNDS:      (f32, f32, f32, f32) = (440.0, 270.0, 480.0, 320.0);
    fn in_rect(p: (f32, f32), r: (f32, f32, f32, f32)) -> bool {
        p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
    }
    if promoted_table_pre != crate::objc::nil && was_hidden_pre {
        let still_hidden: bool = msg![env; promoted_table_pre isHidden];
        if still_hidden {
            // The user was on the confirmation panel (picker hidden) when
            // this touch landed. Stop any leftover song preview now,
            // regardless of where on the panel they tapped -- the artwork,
            // the dialog, the off-button area, etc. Otherwise a preview
            // starts when the song is picked and never stops (because the
            // game can dismiss the confirmation via paths we don't watch
            // for), leaving music playing for one song while the picker
            // shows a different one.
            crate::frameworks::media_player::music_library::stop_song_preview();
            // touch_up_xy is in iOS-portrait coords (320x480) because the
            // window's frame is in UIKit's native portrait orientation.
            // The visible confirmation panel — and the bounds below — are
            // laid out in the rotated landscape canvas (480x320). Convert
            // before testing.
            // For --landscape-left (-π/2 from portrait, the only orientation
            // this game ships in): landscape_x = portrait_y,
            //                        landscape_y = 320 - portrait_x.
            // Verified against the user's button-to-touch=A,240,165
            // ("center"): landscape (240, 165) -> portrait (155, 240),
            // matching the centre of a 320x480 canvas.
            let landscape = touch_up_xy.map(|(px, py)| (py, 320.0 - px));
            let tap_was_no = landscape
                .map(|p| in_rect(p, NO_BUTTON_BOUNDS))
                .unwrap_or(false);
            let tap_was_create = landscape
                .map(|p| in_rect(p, CREATE_TROOPER_BOUNDS))
                .unwrap_or(false);
            let tap_was_back = landscape
                .map(|p| in_rect(p, BACK_BUTTON_BOUNDS))
                .unwrap_or(false);
            if tap_was_back {
                // Back-icon: tell game to cancel its iPod flow, then tear
                // our picker down entirely. The game's cancel handler
                // exits the iPod view and returns to Soul Master.
                use crate::abi::{CallFromHost, GuestFunction};
                let cancel_fn =
                    GuestFunction::from_addr_and_thumb_flag(0x3244, true);
                let _: () = cancel_fn.call_from_host(env, (1i32,));
                crate::frameworks::uikit::ui_view::ui_table_view::dismiss_promoted_picker(env);
                log!(
                    "ui_touch: tap at {:?} on Back -> cancel + dismiss",
                    touch_up_xy
                );
            } else if tap_was_no {
                // SIMPLE CYCLE: clear all + remount the picker.
                //   1. Tell game to cancel its iPod flow.
                //   2. Reset the iPodView2 animation/state singleton.
                //   3. Aggressively re-mount our 1317-row promoted picker
                //      (re-parent to window, flush cell cache, layout).
                // This gives a consistent "back to picker" experience.
                // Confirmation-panel rebuild on pick 2+ is a separate
                // open issue documented in dev-docs/song-summoner-picker.md.
                use crate::abi::{CallFromHost, GuestFunction};
                let cancel_fn =
                    GuestFunction::from_addr_and_thumb_flag(0x3244, true);
                let _: () = cancel_fn.call_from_host(env, (1i32,));
                crate::frameworks::uikit::ui_view::ui_table_view::invoke_ipodview_reset(env);
                let remounted =
                    crate::frameworks::uikit::ui_view::ui_table_view::remount_promoted_picker(env);
                log!(
                    "ui_touch: tap at {:?} on No -> cancel + reset + remount (remounted={})",
                    touch_up_xy, remounted
                );
            } else if tap_was_create {
                // Game advances to the next scene — remove the picker
                // entirely (table + synthetic tab bar) so it doesn't
                // ghost-render behind the new scene and a future picker
                // session promotes a fresh table. Preview was already
                // stopped above.
                crate::frameworks::uikit::ui_view::ui_table_view::dismiss_promoted_picker(env);
                log!(
                    "ui_touch: tap at {:?} on Create Trooper -> picker dismissed",
                    touch_up_xy
                );
            }
            // Tap anywhere else on the confirmation screen (artwork,
            // dialog text, off-button) -- no-op. Preview keeps playing,
            // picker stays hidden, user can keep deciding.
            if !tap_was_no && !tap_was_create && !tap_was_back {
                log!(
                    "ui_touch: confirmation-panel tap at portrait={:?} landscape={:?} (no button matched)",
                    touch_up_xy, landscape
                );
            }
        }
    }

    release(env, pool);
}
