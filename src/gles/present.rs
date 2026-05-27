/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Utilities for presenting frames to the window using an abstract OpenGL ES
//! implementation.

use super::gles11_raw as gles11; // constants and types only
use super::GLES;
use crate::matrix::Matrix;
use crate::window::{CursorSpriteImages, CursorVisual};
use std::time::{Duration, Instant};

pub struct FpsCounter {
    time: std::time::Instant,
    frames: u32,
}
impl FpsCounter {
    pub fn start() -> Self {
        FpsCounter {
            time: Instant::now(),
            frames: 0,
        }
    }

    pub fn count_frame(&mut self, label: std::fmt::Arguments<'_>) {
        self.frames += 1;
        let now = Instant::now();
        let duration = now - self.time;
        if duration >= Duration::from_secs(1) {
            self.time = now;
            echo!(
                "touchHLE: {} FPS: {:.2}",
                label,
                std::mem::take(&mut self.frames) as f32 / duration.as_secs_f32()
            );
        }
    }
}

/// Present the the latest frame (e.g. the app's splash screen or rendering
/// output), provided as a texture bound to `GL_TEXTURE_2D`, by drawing it on
/// the window. It may be rotated, scaled and/or letterboxed as necessary. The
/// virtual cursor is also drawn if it should be currently visible.
///
/// `virtual_cursor_visible_at` is `(x, y, sprite_state)`; `cursor_sprites`
/// holds the decoded PNGs to draw. When the requested sprite is missing
/// from `cursor_sprites`, we fall back to the legacy black-dot draw.
///
/// The provided context must be current.
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, CursorVisual)>,
    cursor_sprites: &CursorSpriteImages,
) {
    // While this is a generic utility, it is closely tied to
    // crate::frameworks::opengles::eagl::present_renderbuffer, which handles
    // backing up and restoring OpenGL ES state that this function might touch,
    // so these need to be updated in tandem.

    use gles11::types::*;

    // Draw the quad
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Clear(gles11::COLOR_BUFFER_BIT | gles11::DEPTH_BUFFER_BIT | gles11::STENCIL_BUFFER_BIT);
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
    let vertices: [f32; 12] = [
        -1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0,
    ];
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    let matrix = Matrix::<4>::from(&rotation_matrix);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();

    // Display virtual cursor
    if let Some((x, y, visual)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.Enable(gles11::BLEND);
        // Sprites are pre-multiplied (see image::from_bytes); use SRC_ALPHA
        // when falling back to the black-dot path (not pre-multiplied) and
        // ONE when drawing a sprite. We re-set this inside each branch.

        if let Some(sprite) = cursor_sprites.for_state(visual) {
            // Sprite path: upload as a texture and draw a textured quad
            // centred at the cursor position. We allocate + delete the
            // texture every frame; tiny sprites make this cheap and it
            // avoids cross-context lifetime issues if the GL context is
            // recreated.
            let (sw, sh) = sprite.dimensions();

            gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
            gles.Color4f(1.0, 1.0, 1.0, 1.0);

            let mut texture: GLuint = 0;
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            gles.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                sw as _,
                sh as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                sprite.pixels().as_ptr() as *const _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::NEAREST as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::NEAREST as _,
            );

            gles.Enable(gles11::TEXTURE_2D);
            gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
            // Top-left UV anchor: the cursor's reported (x, y) is the
            // "hot point" (tip of the hand / centre of the orb). Most
            // pointer art uses the upper-left pixel as the hot point, so
            // draw the sprite from (x, y) extending right + down.
            let sprite_tex_coords: [f32; 12] = [
                0.0, 1.0, // bottom-left
                0.0, 0.0, // top-left
                1.0, 1.0, // bottom-right
                1.0, 1.0, // bottom-right
                0.0, 0.0, // top-left
                1.0, 0.0, // top-right
            ];
            gles.TexCoordPointer(
                2,
                gles11::FLOAT,
                0,
                sprite_tex_coords.as_ptr() as *const GLvoid,
            );
            // Avoid the rotation matrix applied to the underlying frame —
            // the cursor coords are already in screen space.
            gles.MatrixMode(gles11::TEXTURE);
            gles.LoadIdentity();

            // Sprite quad: (x, y) is upper-left, draw sw x sh pixels.
            let sw = sw as f32;
            let sh = sh as f32;
            let mut sprite_verts: [f32; 12] = [
                0.0, sh, // bottom-left
                0.0, 0.0, // top-left
                sw, sh, // bottom-right
                sw, sh, // bottom-right
                0.0, 0.0, // top-left
                sw, 0.0, // top-right
            ];
            for i in (0..sprite_verts.len()).step_by(2) {
                sprite_verts[i] = (sprite_verts[i] + x) / (vw as f32 / 2.0) - 1.0;
                sprite_verts[i + 1] = 1.0 - (sprite_verts[i + 1] + y) / (vh as f32 / 2.0);
            }
            gles.VertexPointer(2, gles11::FLOAT, 0, sprite_verts.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLES, 0, 6);

            gles.DeleteTextures(1, &texture);
            gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
            gles.Disable(gles11::TEXTURE_2D);
        } else {
            // Legacy fallback: small dark dot, alpha based on press state.
            gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
            gles.Disable(gles11::TEXTURE_2D);

            gles.BlendFunc(gles11::SRC_ALPHA, gles11::ONE_MINUS_SRC_ALPHA);
            let pressed = !matches!(visual, CursorVisual::Idle);
            gles.Color4f(0.0, 0.0, 0.0, if pressed { 2.0 / 3.0 } else { 1.0 / 3.0 });

            let radius = 10.0;
            let mut vertices = vertices;
            for i in (0..vertices.len()).step_by(2) {
                vertices[i] = (vertices[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
                vertices[i + 1] = 1.0 - (vertices[i + 1] * radius + y) / (vh as f32 / 2.0);
            }
            gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        }
    }
}
