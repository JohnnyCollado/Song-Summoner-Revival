/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Virtual cursor state machine.
//!
//! Phase 1: just analog-stick movement + a single "tap/drag" button. Tap-vs-
//! drag distinction, snap, double-tap, etc. come in later phases.
//!
//! Coordinates here are in "window pixels" — the same space the SDL events
//! produce — so the existing `transform_input_coords` in `window.rs` can map
//! them into the guest's iOS coord system when we emit touches.

use std::time::Instant;

/// One-letter user actions the cursor understands. Higher layers map physical
/// controller buttons onto these (so the per-game mapping in
/// [super::mapping] can be swapped without touching the state machine).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CursorAction {
    /// Primary tap/drag button. Short press = tap, long press = drag.
    Tap,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum CursorState {
    /// Cursor is on screen but no touch is being synthesized. The user is
    /// "hovering."
    Idle,
    /// The tap button just went down; we haven't yet decided if this is a
    /// tap or the start of a drag.
    Pressed { began_pos: (f32, f32), began_at: Instant },
    /// Tap button held past the tap window OR cursor moved past the slop
    /// threshold while pressed — emitting TouchesMove every tick.
    Dragging { began_pos: (f32, f32) },
}

/// Configuration for the cursor's movement + gesture-resolution behaviour.
/// Phase 1 keeps this hardcoded; later phases populate it from a per-game
/// TOML file.
#[derive(Copy, Clone, Debug)]
pub struct CursorConfig {
    /// Maximum movement speed at full stick deflection, in window pixels/sec.
    pub max_speed_px_s: f32,
    /// Stick deflection below this is treated as zero (per-axis).
    pub deadzone: f32,
    /// If the user releases the tap button within this many milliseconds AND
    /// moves less than `tap_slop_px`, it's a tap (synthesize Down+Up at the
    /// same point). Otherwise it's a drag (Down on press, Move while held,
    /// Up on release).
    pub tap_max_ms: u128,
    pub tap_slop_px: f32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            max_speed_px_s: 600.0,
            deadzone: 0.18,
            tap_max_ms: 200,
            tap_slop_px: 8.0,
        }
    }
}

pub struct VirtualCursor {
    pub cfg: CursorConfig,
    /// Window-pixel position.
    pos: (f32, f32),
    /// Latest analog stick deflection (post-deadzone), in unit-disk coords.
    stick: (f32, f32),
    state: CursorState,
    /// Bounding rect the cursor is clamped to (window pixels).
    bounds: (f32, f32, f32, f32),
    last_tick: Instant,
    /// Last coord we actually emitted as a touch event — used by the synth
    /// layer to decide whether a Move would be a no-op.
    pub last_emitted: Option<(f32, f32)>,
}

impl VirtualCursor {
    pub fn new(bounds: (f32, f32, f32, f32)) -> Self {
        let (x, y, w, h) = bounds;
        Self {
            cfg: CursorConfig::default(),
            pos: (x + w * 0.5, y + h * 0.5),
            stick: (0.0, 0.0),
            state: CursorState::Idle,
            bounds,
            last_tick: Instant::now(),
            last_emitted: None,
        }
    }

    pub fn pos(&self) -> (f32, f32) {
        self.pos
    }
    pub fn state(&self) -> CursorState {
        self.state
    }

    /// Update the visible/usable bounding rect (window pixels). Call this
    /// whenever the window resizes — keeps the cursor on-screen.
    pub fn set_bounds(&mut self, bounds: (f32, f32, f32, f32)) {
        self.bounds = bounds;
        self.pos = (
            self.pos.0.clamp(bounds.0, bounds.0 + bounds.2),
            self.pos.1.clamp(bounds.1, bounds.1 + bounds.3),
        );
    }

    /// Feed in a raw stick deflection (x and y both in [-1, 1]).
    pub fn set_stick(&mut self, x: f32, y: f32) {
        let dz = self.cfg.deadzone;
        let apply = |v: f32| if v.abs() < dz { 0.0 } else { v };
        self.stick = (apply(x), apply(y));
    }

    /// Tap button pressed (true) or released (false). Returns a `TapResolution`
    /// describing what the synth layer should emit at this exact moment.
    pub fn set_button(&mut self, action: CursorAction, pressed: bool) -> TapResolution {
        match (action, self.state, pressed) {
            (CursorAction::Tap, CursorState::Idle, true) => {
                self.state = CursorState::Pressed {
                    began_pos: self.pos,
                    began_at: Instant::now(),
                };
                TapResolution::None
            }
            (CursorAction::Tap, CursorState::Pressed { began_pos, began_at }, false) => {
                let elapsed_ms = began_at.elapsed().as_millis();
                let dx = self.pos.0 - began_pos.0;
                let dy = self.pos.1 - began_pos.1;
                let moved = (dx * dx + dy * dy).sqrt();
                self.state = CursorState::Idle;
                if elapsed_ms <= self.cfg.tap_max_ms && moved <= self.cfg.tap_slop_px {
                    TapResolution::Tap(self.pos)
                } else {
                    // It was a quick drag-and-release: emit a Down + Up so
                    // the guest sees a meaningful "draggy" event pair.
                    TapResolution::DragRelease(began_pos, self.pos)
                }
            }
            (CursorAction::Tap, CursorState::Dragging { .. }, false) => {
                self.state = CursorState::Idle;
                TapResolution::DragEnd(self.pos)
            }
            _ => TapResolution::None,
        }
    }

    /// Run movement physics + state promotion (Pressed → Dragging when the
    /// hold window expires or the cursor strays past the slop). Should be
    /// called once per event-poll iteration.
    pub fn tick(&mut self) -> TickEvents {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

        // Apply movement.
        let speed = self.cfg.max_speed_px_s;
        self.pos.0 += self.stick.0 * speed * dt;
        self.pos.1 += self.stick.1 * speed * dt;
        let (bx, by, bw, bh) = self.bounds;
        self.pos.0 = self.pos.0.clamp(bx, bx + bw);
        self.pos.1 = self.pos.1.clamp(by, by + bh);

        let mut events = TickEvents::default();

        // Maybe promote a Pressed → Dragging based on time or movement.
        if let CursorState::Pressed { began_pos, began_at } = self.state {
            let elapsed_ms = began_at.elapsed().as_millis();
            let dx = self.pos.0 - began_pos.0;
            let dy = self.pos.1 - began_pos.1;
            let moved = (dx * dx + dy * dy).sqrt();
            if elapsed_ms > self.cfg.tap_max_ms || moved > self.cfg.tap_slop_px {
                self.state = CursorState::Dragging { began_pos };
                events.drag_started = Some(began_pos);
            }
        }
        if let CursorState::Dragging { .. } = self.state {
            // Always report current position to the synth layer; it'll diff
            // against last_emitted before pushing a Move.
            events.drag_position = Some(self.pos);
        }

        events
    }
}

/// What the synth layer should emit for the current `set_button` call.
#[derive(Copy, Clone, Debug)]
pub enum TapResolution {
    /// Nothing — the button state changed but no touch event is implied yet
    /// (e.g. Press transitioning to Pressed; release while Idle).
    None,
    /// User held briefly without moving: emit Down + Up at the same point.
    Tap((f32, f32)),
    /// User released after a Pressed→Dragging promotion: emit Up at `to`.
    DragEnd((f32, f32)),
    /// Pressed and released quickly but moved past slop: emit Down at `from`,
    /// Move + Up at `to`. Treated like a short drag.
    DragRelease((f32, f32), (f32, f32)),
}

/// Per-tick movement events the synth layer turns into touches.
#[derive(Default, Copy, Clone, Debug)]
pub struct TickEvents {
    /// First tick a drag began (after Pressed promotion). Synth emits Down
    /// at this point.
    pub drag_started: Option<(f32, f32)>,
    /// Current cursor position during a drag. Synth diffs against
    /// last_emitted and emits Move if changed.
    pub drag_position: Option<(f32, f32)>,
}
