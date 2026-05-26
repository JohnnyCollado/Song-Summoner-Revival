/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Virtual cursor input system (Phase 1).
//!
//! See the design doc / commit history for the staged rollout. Phase 1
//! provides analog-stick → cursor movement and a tap/drag button. Future
//! phases add tile snapping, per-game TOML mappings, visible overlay shapes,
//! and double-tap.

pub mod cursor;
pub mod mapping;
pub mod synth;

pub use cursor::{CursorAction, CursorConfig, CursorState, VirtualCursor};
pub use mapping::ControllerMapping;
