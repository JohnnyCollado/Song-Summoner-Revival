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

/// How the focus marker marks its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusShape {
    /// Corner brackets just outside it (buttons and rows).
    Brackets,
    /// The diamond inscribed in it, traced from inside (a battle map tile,
    /// which is a 2:1 diamond).
    Diamond,
}

/// Where the focus marker is (x, y, width, height) and its shape.
pub type FocusMarker = ((f32, f32, f32, f32), FocusShape);

/// Present the the latest frame (e.g. the app's splash screen or rendering
/// output), provided as a texture bound to `GL_TEXTURE_2D`, by drawing it on
/// the window. It may be rotated, scaled and/or letterboxed as necessary. The
/// virtual cursor is also drawn if it should be currently visible, and so are
/// the focus marker (see [crate::window::Window::set_focus_marker]) and any
/// debug lines (see [crate::window::Window::set_debug_lines]).
///
/// The provided context must be current.
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    focus_marker: Option<FocusMarker>,
    debug_lines: &[((f32, f32), (f32, f32))],
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
    if let Some((x, y, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);

        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
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

    // Display the focus marker: thin corner brackets a few pixels outside
    // the rectangle. Nothing is drawn over the button itself, and the sides
    // are left open, so it doesn't fight the game's own art (a full box and
    // tint looked heavy on the busier screens).
    if let Some(((x, y, w, h), FocusShape::Brackets)) = focus_marker {
        let (vx, vy, vw, vh) = viewport;
        let (x, y) = (x - vx as f32, y - vy as f32);

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);

        // Premultiplied alpha, like the cursor above.
        let quad = |gles: &mut dyn GLES, x: f32, y: f32, w: f32, h: f32| {
            let (x0, x1) = (x / (vw as f32 / 2.0) - 1.0, (x + w) / (vw as f32 / 2.0) - 1.0);
            let (y0, y1) = (1.0 - y / (vh as f32 / 2.0), 1.0 - (y + h) / (vh as f32 / 2.0));
            let quad: [f32; 12] = [x0, y0, x1, y0, x0, y1, x1, y0, x0, y1, x1, y1];
            gles.VertexPointer(2, gles11::FLOAT, 0, quad.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        };
        // Thickness scales with the window, about 1.5 points of the app.
        let t = (vw.min(vh) as f32 / 213.0).max(1.5);
        let brackets = focus_brackets((x, y, w, h), t, t / 2.0);
        // Two tones, so it reads on both the game's light-blue panels and
        // its pale skies: a dark border all round, then a gold core. (Gold
        // because the game's UI is mostly blue; a blue marker vanished.)
        let a = 0.8;
        gles.Color4f(0.0, 0.0, 0.0, a);
        for bracket in brackets {
            let (bx, by, bw, bh) = bracket_border(bracket, t);
            quad(gles, bx, by, bw, bh);
        }
        gles.Color4f(1.0, 0.82, 0.25, 1.0);
        for (bx, by, bw, bh) in brackets {
            quad(gles, bx, by, bw, bh);
        }
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
    }

    // A tile's marker: the same two tones, tracing the diamond just inside
    // its edges (the dark border reaching half a line past them), so it
    // lies on the map like the game's own tile highlights.
    if let Some(((x, y, w, h), FocusShape::Diamond)) = focus_marker {
        let (vx, vy, vw, vh) = viewport;
        let rect = (x - vx as f32, y - vy as f32, w, h);

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);

        let draw = |gles: &mut dyn GLES, tris: [(f32, f32); 24]| {
            let mut vertices = [0.0f32; 48];
            for (i, (x, y)) in tris.into_iter().enumerate() {
                vertices[2 * i] = x / (vw as f32 / 2.0) - 1.0;
                vertices[2 * i + 1] = 1.0 - y / (vh as f32 / 2.0);
            }
            gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLES, 0, 24);
        };
        // As thick as the brackets.
        let t = (vw.min(vh) as f32 / 213.0).max(1.5);
        // Premultiplied alpha.
        let a = 0.8;
        gles.Color4f(0.0, 0.0, 0.0, a);
        draw(gles, diamond_ring(diamond_grow(rect, t / 2.0), 2.0 * t));
        gles.Color4f(1.0, 0.82, 0.25, 1.0);
        draw(gles, diamond_ring(rect, t));
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
    }

    // Debug lines: thin magenta, drawn last so they sit over everything.
    if !debug_lines.is_empty() {
        let (vx, vy, vw, vh) = viewport;
        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        // Premultiplied alpha.
        let a = 0.85;
        gles.Color4f(a, 0.2 * a, a, a);
        let t = (vw.min(vh) as f32 / 320.0).max(1.0);
        let mut vertices = Vec::with_capacity(debug_lines.len() * 12);
        for &((x0, y0), (x1, y1)) in debug_lines {
            let from = (x0 - vx as f32, y0 - vy as f32);
            let to = (x1 - vx as f32, y1 - vy as f32);
            let Some(tris) = line_triangles(from, to, t) else {
                continue;
            };
            for (x, y) in tris {
                vertices.push(x / (vw as f32 / 2.0) - 1.0);
                vertices.push(1.0 - y / (vh as f32 / 2.0));
            }
        }
        gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, (vertices.len() / 2) as _);
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
    }
}

/// Two triangles covering a line from `from` to `to`, `t` thick, or `None`
/// for a line of no length.
fn line_triangles(from: (f32, f32), to: (f32, f32), t: f32) -> Option<[(f32, f32); 6]> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return None;
    }
    // Half the thickness, across the line.
    let (nx, ny) = (-dy / len * t / 2.0, dx / len * t / 2.0);
    let a = (from.0 + nx, from.1 + ny);
    let b = (from.0 - nx, from.1 - ny);
    let c = (to.0 + nx, to.1 + ny);
    let d = (to.0 - nx, to.1 - ny);
    Some([a, b, c, b, c, d])
}

/// The diamond inscribed in `rect` (x, y, w, h), grown so each edge moves
/// `d` outward (inward if negative), as the box it's inscribed in.
fn diamond_grow((x, y, w, h): (f32, f32, f32, f32), d: f32) -> (f32, f32, f32, f32) {
    let (a, b) = (w / 2.0, h / 2.0);
    let (cx, cy) = (x + a, y + b);
    // The edges are a·b / √(a² + b²) from the middle; scaling the diamond
    // scales that, so scale it by (that + d) / that.
    let k = 1.0 + d * (a * a + b * b).sqrt() / (a * b);
    let (a, b) = ((a * k).max(0.0), (b * k).max(0.0));
    (cx - a, cy - b, 2.0 * a, 2.0 * b)
}

/// Triangles filling a band `t` wide just inside the edges of the diamond
/// inscribed in `rect`: one quad per edge, meeting in mitred corners.
fn diamond_ring(rect: (f32, f32, f32, f32), t: f32) -> [(f32, f32); 24] {
    // Top, right, bottom, left.
    let corners = |(x, y, w, h): (f32, f32, f32, f32)| {
        [
            (x + w / 2.0, y),
            (x + w, y + h / 2.0),
            (x + w / 2.0, y + h),
            (x, y + h / 2.0),
        ]
    };
    let outer = corners(rect);
    let inner = corners(diamond_grow(rect, -t));
    let mut tris = [(0.0, 0.0); 24];
    for i in 0..4 {
        let j = (i + 1) % 4;
        tris[6 * i..6 * i + 6]
            .copy_from_slice(&[outer[i], outer[j], inner[i], outer[j], inner[j], inner[i]]);
    }
    tris
}

/// The focus marker's corner brackets around `rect` (x, y, w, h), as quads
/// in the same form: two arms per corner, `t` thick, `pad` outside the
/// rectangle. The arms reach about a third of the shorter side in, and never
/// meet, so the middle of each side stays open.
fn focus_brackets(
    (x, y, w, h): (f32, f32, f32, f32),
    t: f32,
    pad: f32,
) -> [(f32, f32, f32, f32); 8] {
    let (ox, oy) = (x - pad - t, y - pad - t);
    let (ow, oh) = (w + 2.0 * (pad + t), h + 2.0 * (pad + t));
    let len = (w.min(h) * 0.3 + pad + t)
        .min(ow / 2.0 - t)
        .min(oh / 2.0 - t);
    let (right, bottom) = (ox + ow, oy + oh);
    [
        // Top left.
        (ox, oy, len, t),
        (ox, oy, t, len),
        // Top right.
        (right - len, oy, len, t),
        (right - t, oy, t, len),
        // Bottom left.
        (ox, bottom - t, len, t),
        (ox, bottom - len, t, len),
        // Bottom right.
        (right - len, bottom - t, len, t),
        (right - t, bottom - len, t, len),
    ]
}

/// The dark border behind one of [focus_brackets]' quads, `t` being their
/// thickness: half of it again on every side.
fn bracket_border((x, y, w, h): (f32, f32, f32, f32), t: f32) -> (f32, f32, f32, f32) {
    let b = t / 2.0;
    (x - b, y - b, w + 2.0 * b, h + 2.0 * b)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Rect = (f32, f32, f32, f32);

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    fn contains(outer: Rect, p: (f32, f32)) -> bool {
        p.0 >= outer.0 && p.0 <= outer.0 + outer.2 && p.1 >= outer.1 && p.1 <= outer.1 + outer.3
    }

    #[test]
    fn brackets_stay_clear_of_the_button() {
        // A 160×54 button (the title menu's), 2 px lines 2 px out.
        let button = (80.0, 131.0, 160.0, 54.0);
        let outer = (76.0, 127.0, 168.0, 62.0);
        for quad in focus_brackets(button, 2.0, 2.0) {
            assert!(!overlaps(quad, button), "{quad:?} covers the button");
            assert!(contains(outer, (quad.0, quad.1)));
            assert!(contains(outer, (quad.0 + quad.2, quad.1 + quad.3)));
        }
    }

    #[test]
    fn brackets_mark_only_the_corners() {
        let button = (80.0, 131.0, 160.0, 54.0);
        let quads = focus_brackets(button, 2.0, 2.0);
        // Each corner of the outer box is covered...
        for corner in [(76.5, 127.5), (243.5, 127.5), (76.5, 188.5), (243.5, 188.5)] {
            assert!(quads.iter().any(|&q| contains(q, corner)), "{corner:?}");
        }
        // ...but not the middle of any side.
        for middle in [(160.0, 128.0), (160.0, 188.0), (77.0, 158.0), (243.0, 158.0)] {
            assert!(!quads.iter().any(|&q| contains(q, middle)), "{middle:?}");
        }
    }

    #[test]
    fn bracket_borders_hug_the_button() {
        // With its dark border, the marker reaches at most 3 t outside the
        // button, so it doesn't look oversized on small buttons.
        let (t, pad) = (2.0, 1.0);
        let button = (80.0, 131.0, 160.0, 54.0);
        for quad in focus_brackets(button, t, pad) {
            let (x, y, w, h) = bracket_border(quad, t);
            assert!(x >= 80.0 - 3.0 * t && y >= 131.0 - 3.0 * t);
            assert!(x + w <= 240.0 + 3.0 * t && y + h <= 185.0 + 3.0 * t);
            // The border surrounds the core on every side.
            assert!(x < quad.0 && y < quad.1);
            assert!(x + w > quad.0 + quad.2 && y + h > quad.1 + quad.3);
        }
    }

    #[test]
    fn a_line_is_a_thin_rectangle_along_it() {
        let tris = line_triangles((10.0, 20.0), (30.0, 20.0), 2.0).unwrap();
        for (x, y) in tris {
            assert!(x == 10.0 || x == 30.0, "{x}");
            assert!(y == 19.0 || y == 21.0, "{y}");
        }
        // Both ends, both sides.
        for corner in [(10.0, 19.0), (10.0, 21.0), (30.0, 19.0), (30.0, 21.0)] {
            assert!(tris.contains(&corner), "{corner:?}");
        }
    }

    #[test]
    fn a_diagonal_line_keeps_its_thickness() {
        let tris = line_triangles((0.0, 0.0), (30.0, 40.0), 2.0).unwrap();
        // Every vertex is 1 (half the thickness) off the line through the
        // ends, and within its length.
        for (x, y) in tris {
            let off = (4.0 * x - 3.0 * y).abs() / 5.0;
            assert!((off - 1.0).abs() < 1e-4, "{off}");
            let along = (3.0 * x + 4.0 * y) / 5.0;
            assert!((-1e-4..=50.0 + 1e-4).contains(&along), "{along}");
        }
    }

    #[test]
    fn a_zero_length_line_draws_nothing() {
        assert!(line_triangles((5.0, 5.0), (5.0, 5.0), 2.0).is_none());
    }

    #[test]
    fn brackets_on_a_small_button_dont_meet() {
        // A 48×48 icon: arms shorter than half of each side.
        let quads = focus_brackets((430.0, 298.0, 48.0, 48.0), 2.0, 2.0);
        for (x, y, w, h) in quads {
            assert!(w.max(h) < (48.0 + 8.0) / 2.0, "{:?}", (x, y, w, h));
        }
    }

    // A battle tile at 130% zoom: a 62.4×31.2 diamond in its bounding box.
    const TILE: Rect = (208.8, 144.4, 62.4, 31.2);

    /// How far `p` is inside the diamond inscribed in `rect` (negative:
    /// outside), measured square to its edges.
    fn inside_diamond((x, y, w, h): Rect, p: (f32, f32)) -> f32 {
        let (a, b) = (w / 2.0, h / 2.0);
        let (dx, dy) = ((p.0 - x - a).abs(), (p.1 - y - b).abs());
        // The edge is dx/a + dy/b = 1; scale to a distance.
        (1.0 - dx / a - dy / b) * a * b / (a * a + b * b).sqrt()
    }

    #[test]
    fn a_diamond_ring_traces_the_tile_from_inside() {
        let t = 2.0;
        let tris = diamond_ring(TILE, t);
        // Its outer corners are the tile's: the middles of the box's sides.
        for corner in [(240.0, 144.4), (271.2, 160.0), (240.0, 175.6), (208.8, 160.0)] {
            assert!(
                tris.iter()
                    .any(|&p| (p.0 - corner.0).abs() < 1e-3 && (p.1 - corner.1).abs() < 1e-3),
                "{corner:?}"
            );
        }
        // Every vertex is on the tile, at most t in from its edge.
        for p in tris {
            let d = inside_diamond(TILE, p);
            assert!((-1e-3..=t + 1e-3).contains(&d), "{p:?} is {d} in");
        }
    }

    #[test]
    fn a_diamond_ring_leaves_the_tile_open() {
        // The inner edge is exactly t in, so the tile's middle (where the
        // unit stands) is left clear.
        let t = 2.0;
        let (x, y, w, h) = diamond_grow(TILE, -t);
        let corner = (x + w, y + h / 2.0);
        assert!((inside_diamond(TILE, corner) - t).abs() < 1e-3);
        assert!(w > 0.0 && h > 0.0);
    }

    #[test]
    fn growing_a_diamond_moves_its_edges_out_evenly() {
        let d = 1.5;
        let (x, y, w, h) = diamond_grow(TILE, d);
        // Same centre and shape (2:1)...
        assert!((x + w / 2.0 - 240.0).abs() < 1e-3 && (y + h / 2.0 - 160.0).abs() < 1e-3);
        assert!((w / h - 2.0).abs() < 1e-3);
        // ...with its corners d outside the tile's edges.
        let top = (x + w / 2.0, y);
        let right = (x + w, y + h / 2.0);
        assert!((inside_diamond(TILE, top) + d).abs() < 1e-3);
        assert!((inside_diamond(TILE, right) + d).abs() < 1e-3);
    }
}
