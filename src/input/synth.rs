/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Converts [super::cursor::VirtualCursor] state into [crate::window::Event]
//! touch events. The window event loop drains the returned Vec straight into
//! its `event_queue`, so the rest of touchHLE sees the cursor as ordinary
//! single-finger touches.
//!
//! Coordinates: the cursor stores window-pixel positions. We call into the
//! caller-supplied transform closure here to convert to whatever coord
//! system the downstream pipeline expects (today: the iOS portrait coords
//! that [crate::window::Window::transform_input_coords] produces).

use super::cursor::{TapResolution, TickEvents, VirtualCursor};
use crate::window::{Event, FingerId};
use std::collections::HashMap;

/// Pure function. Translate a tap-button state change into 0–2 Touch events.
pub fn events_for_tap<F>(
    cursor: &mut VirtualCursor,
    resolution: TapResolution,
    mut to_guest: F,
) -> Vec<Event>
where
    F: FnMut((f32, f32)) -> (f32, f32),
{
    let mut out = Vec::new();
    match resolution {
        TapResolution::None => {}
        TapResolution::Tap(p) => {
            let g = to_guest(p);
            out.push(Event::TouchesDown(HashMap::from([(
                FingerId::VirtualCursor,
                g,
            )])));
            out.push(Event::TouchesUp(HashMap::from([(
                FingerId::VirtualCursor,
                g,
            )])));
            cursor.last_emitted = None;
        }
        TapResolution::DragEnd(p) => {
            let g = to_guest(p);
            out.push(Event::TouchesUp(HashMap::from([(
                FingerId::VirtualCursor,
                g,
            )])));
            cursor.last_emitted = None;
        }
        TapResolution::DragRelease(from, to) => {
            let g_from = to_guest(from);
            let g_to = to_guest(to);
            out.push(Event::TouchesDown(HashMap::from([(
                FingerId::VirtualCursor,
                g_from,
            )])));
            if g_from != g_to {
                out.push(Event::TouchesMove(HashMap::from([(
                    FingerId::VirtualCursor,
                    g_to,
                )])));
            }
            out.push(Event::TouchesUp(HashMap::from([(
                FingerId::VirtualCursor,
                g_to,
            )])));
            cursor.last_emitted = None;
        }
    }
    out
}

/// Pure function. Translate the per-tick movement events from
/// [VirtualCursor::tick] into 0–2 touch events. Diff against
/// `cursor.last_emitted` so a still cursor doesn't spam Move events.
pub fn events_for_tick<F>(
    cursor: &mut VirtualCursor,
    tick: TickEvents,
    mut to_guest: F,
) -> Vec<Event>
where
    F: FnMut((f32, f32)) -> (f32, f32),
{
    let mut out = Vec::new();
    if let Some(start) = tick.drag_started {
        let g = to_guest(start);
        out.push(Event::TouchesDown(HashMap::from([(
            FingerId::VirtualCursor,
            g,
        )])));
        cursor.last_emitted = Some(g);
    }
    if let Some(pos) = tick.drag_position {
        let g = to_guest(pos);
        // Skip if we already started this tick (Down covers it) or if the
        // position is unchanged from the last emission.
        let same = matches!(tick.drag_started, Some(_)) || cursor.last_emitted == Some(g);
        if !same {
            out.push(Event::TouchesMove(HashMap::from([(
                FingerId::VirtualCursor,
                g,
            )])));
            cursor.last_emitted = Some(g);
        }
    }
    out
}
