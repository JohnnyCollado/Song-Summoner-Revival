/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Drawing the Setup menu, the first-run toast and the FPS counter.
//!
//! Reuses the picker's canvas ([Canvas], [Fonts]) and the controller icons
//! ([GlyphCache]). Everything is laid out in the game's 480×320 landscape
//! points and drawn at [SCALE]× so text stays sharp on big windows; the
//! window stretches the bitmap over the game's viewport
//! (`Window::set_overlay`). The same layout answers "what was tapped"
//! ([hit]), so drawing and touch never disagree.

use super::glyphs::{Family, Glyph, GlyphCache};
use super::keys::key_label;
use super::pad;
use super::picker_render::{Canvas, Fonts, Rgb};
use super::setup_menu::{
    dialog_text, Focus, Glyphish, Info, Menu, Mode, Row, Tested, Value,
};
use super::settings::{CursorStyle, Settings};
use crate::media::artwork::Bitmap;

pub const W: i32 = 480;
pub const H: i32 = 320;
/// Pixels drawn per point.
pub const SCALE: i32 = 2;

const PANEL: Rgb = (18, 15, 32);
const LINE: Rgb = (58, 51, 88);
const INK: Rgb = (233, 231, 243);
const DIM: Rgb = (157, 152, 184);
const SEL: Rgb = (59, 44, 115);
const GOLD: Rgb = (226, 189, 82);
const VIOLET: Rgb = (143, 115, 230);
const KEYCAP: Rgb = (233, 231, 243);
const KEYCAP_TEXT: Rgb = (28, 26, 38);

// The panel and its parts, in points.
const PANEL_X: i32 = 20;
const PANEL_Y: i32 = 14;
const PANEL_W: i32 = W - 2 * PANEL_X;
const PANEL_H: i32 = H - 2 * PANEL_Y;
const HEAD_H: i32 = 26;
const FOOT_H: i32 = 24;
const RAIL_W: i32 = 100;
const TAB_H: i32 = 24;
const BODY_Y: i32 = PANEL_Y + HEAD_H;
const BODY_H: i32 = PANEL_H - HEAD_H - FOOT_H;
const PANE_X: i32 = PANEL_X + RAIL_W + 6;
const PANE_W: i32 = PANEL_X + PANEL_W - 8 - PANE_X;
const ROW_PAD: i32 = 6;

const TITLE_SIZE: f32 = 15.0;
const TAB_SIZE: f32 = 12.0;
const LABEL_SIZE: f32 = 11.5;
const SUB_SIZE: f32 = 9.0;
const HEADING_SIZE: f32 = 8.5;
const FOOT_SIZE: f32 = 9.5;
const GLYPH: i32 = 17;

// The restart dialog.
const DIALOG_W: i32 = 320;
const DIALOG_X: i32 = (W - DIALOG_W) / 2;
const BUTTON_H: i32 = 22;

fn line_h(size: f32) -> i32 {
    (size * 1.3).ceil() as i32
}

/// Text width in points.
fn width(fonts: &Fonts, bold: bool, size: f32, text: &str) -> i32 {
    (fonts.width(bold, size * SCALE as f32, text) / SCALE as f32).ceil() as i32
}

/// Greedy word wrap to `max` points.
pub fn wrap(fonts: &Fonts, bold: bool, size: f32, text: &str, max: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && width(fonts, bold, size, &candidate) > max {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where a row sits in the pane's scrolling content.
#[derive(Clone, Debug)]
pub struct Placed {
    pub y: i32,
    pub h: i32,
    /// Index among the tab's items, for items.
    pub item: Option<usize>,
    /// Keyboard rows: each key cell's x and width, right to left order
    /// undone (first slot first), relative to the pane.
    pub key_cells: Vec<(i32, i32)>,
}

fn key_cell_w(fonts: &Fonts, key: &str) -> i32 {
    (width(fonts, true, SUB_SIZE, key) + 10).max(26)
}

/// Lay out `rows` in the pane.
pub fn layout(fonts: &Fonts, rows: &[Row]) -> Vec<Placed> {
    let mut y = 0;
    let mut item = 0;
    let mut out = Vec::new();
    for row in rows {
        let (h, this_item, key_cells) = match row {
            Row::Heading(_) => (line_h(HEADING_SIZE) + 8, None, Vec::new()),
            Row::Paragraph(text) => {
                let lines = wrap(fonts, false, SUB_SIZE, text, PANE_W - 2 * ROW_PAD);
                (lines.len() as i32 * line_h(SUB_SIZE) + 6, None, Vec::new())
            }
            Row::Item { sub, value, .. } => {
                let sub_lines = sub.as_ref().map_or(0, |s| {
                    wrap(fonts, false, SUB_SIZE, s, sub_width(fonts, value)).len() as i32
                });
                let h = line_h(LABEL_SIZE) + sub_lines * line_h(SUB_SIZE) + 8;
                let cells = match value {
                    Value::Keys(keys) => {
                        // Right-aligned, first slot on the left.
                        let widths: Vec<i32> = (0..2)
                            .map(|i| key_cell_w(fonts, keys.get(i).map_or("", String::as_str)))
                            .collect();
                        let right = PANE_W - ROW_PAD;
                        let second_x = right - widths[1];
                        let first_x = second_x - 4 - widths[0];
                        vec![(first_x, widths[0]), (second_x, widths[1])]
                    }
                    _ => Vec::new(),
                };
                let placed = (h, Some(item), cells);
                item += 1;
                placed
            }
        };
        out.push(Placed {
            y,
            h,
            item: this_item,
            key_cells,
        });
        y += h;
    }
    out
}

/// How wide the value on the right of a row is, in points.
fn value_width(fonts: &Fonts, value: &Value) -> i32 {
    match value {
        Value::None => 0,
        Value::Text(t) => width(fonts, true, LABEL_SIZE, t),
        Value::Buttons { buttons, joined } => {
            let n = buttons.len() as i32;
            n * GLYPH + (n - 1).max(0) * if *joined { 12 } else { 3 }
        }
        Value::Keys(keys) => {
            let n = keys.len().max(1) as i32;
            (0..n as usize)
                .map(|i| key_cell_w(fonts, keys.get(i).map_or("", String::as_str)))
                .sum::<i32>()
                + (n - 1) * 4
        }
    }
}

/// Room for a row's label (and its sub line) left of its value.
fn label_width(fonts: &Fonts, value: &Value) -> i32 {
    let v = value_width(fonts, value);
    PANE_W - 2 * ROW_PAD - if v > 0 { v + 10 } else { 0 }
}

fn sub_width(fonts: &Fonts, value: &Value) -> i32 {
    // Keycaps sit in the label line only; a sub line under short values
    // may use the full width.
    match value {
        Value::Keys(_) | Value::None => PANE_W - 2 * ROW_PAD,
        _ => label_width(fonts, value),
    }
}

/// Keep the selected item in view.
fn scroll_to_selection(menu: &mut Menu, placed: &[Placed]) {
    let content_h = placed.last().map_or(0, |p| p.y + p.h);
    let max_scroll = (content_h - BODY_H).max(0);
    // Scrolling past the rows (Up/Down at the ends).
    menu.scroll += std::mem::take(&mut menu.nudge);
    // After a drag, the view stays where the finger left it.
    if menu.focus == Focus::Rows && menu.follow {
        if let Some(p) = placed.iter().find(|p| p.item == Some(menu.row)) {
            // Show the heading above the first item of a group too.
            let index = placed.iter().position(|q| q.y == p.y).unwrap_or(0);
            let top = if index > 0 && placed[index - 1].item.is_none() {
                placed[index - 1].y
            } else {
                p.y
            };
            if top < menu.scroll {
                menu.scroll = top;
            }
            if p.y + p.h > menu.scroll + BODY_H {
                menu.scroll = p.y + p.h - BODY_H;
            }
        }
    }
    menu.scroll = menu.scroll.clamp(0, max_scroll);
}

/// What a tap at (x, y) points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Row(usize, Option<usize>),
    DialogYes,
    DialogNo,
    /// Inside the panel, on nothing in particular.
    Panel,
    Outside,
}

fn dialog_layout(fonts: &Fonts, title: &str, body: &str) -> (i32, i32, Vec<String>) {
    let _ = title;
    let lines = wrap(fonts, false, SUB_SIZE + 1.0, body, DIALOG_W - 28);
    let h = 14 + line_h(TITLE_SIZE - 2.0) + 6 + lines.len() as i32 * line_h(SUB_SIZE + 1.0) + 12
        + BUTTON_H
        + 14;
    ((H - h) / 2, h, lines)
}

/// The dialog's two buttons: (x, width) of Yes, then Cancel. Cancel is on
/// the left and Yes on the right, as in the game's own dialogs, so left
/// and right on the pad always mean the same button.
fn dialog_buttons(fonts: &Fonts, yes: &str) -> [(i32, i32); 2] {
    let cancel_w = width(fonts, true, LABEL_SIZE, "Cancel") + 24;
    let yes_w = width(fonts, true, LABEL_SIZE, yes) + 24;
    let right = DIALOG_X + DIALOG_W - 14;
    let yes_x = right - yes_w;
    let cancel_x = yes_x - 8 - cancel_w;
    [(yes_x, yes_w), (cancel_x, cancel_w)]
}

pub fn hit(fonts: &Fonts, menu: &Menu, settings: &Settings, info: &Info, x: f32, y: f32) -> Hit {
    let (x, y) = (x as i32, y as i32);
    if let Mode::Dialog { change, .. } = &menu.mode {
        let (title, body, yes) = dialog_text(change, menu.platform, info);
        let (top, h, _) = dialog_layout(fonts, &title, &body);
        let button_y = top + h - 14 - BUTTON_H;
        if y >= button_y && y < button_y + BUTTON_H {
            let [(yx, yw), (cx, cw)] = dialog_buttons(fonts, &yes);
            if x >= yx && x < yx + yw {
                return Hit::DialogYes;
            }
            if x >= cx && x < cx + cw {
                return Hit::DialogNo;
            }
        }
        let inside = x >= DIALOG_X && x < DIALOG_X + DIALOG_W && y >= top && y < top + h;
        return if inside { Hit::Panel } else { Hit::Outside };
    }
    let inside = x >= PANEL_X && x < PANEL_X + PANEL_W && y >= PANEL_Y && y < PANEL_Y + PANEL_H;
    if !inside {
        return Hit::Outside;
    }
    if y < BODY_Y || y >= BODY_Y + BODY_H {
        return Hit::Panel;
    }
    if x < PANEL_X + RAIL_W {
        let i = (y - BODY_Y - 4) / TAB_H;
        if i >= 0 && (i as usize) < menu.tabs().len() {
            return Hit::Tab(i as usize);
        }
        return Hit::Panel;
    }
    let rows = menu.rows(settings, info);
    let placed = layout(fonts, &rows);
    let content_y = y - BODY_Y + menu.scroll;
    for p in &placed {
        if content_y >= p.y && content_y < p.y + p.h {
            if let Some(item) = p.item {
                let px = x - PANE_X;
                let slot = p
                    .key_cells
                    .iter()
                    .position(|&(cx, cw)| px >= cx && px < cx + cw);
                let slot = if p.key_cells.is_empty() {
                    None
                } else {
                    Some(slot.unwrap_or(0))
                };
                return Hit::Row(item, slot);
            }
        }
    }
    Hit::Panel
}

/// Scaled drawing in points.
struct Painter<'a> {
    c: Canvas,
    fonts: &'a Fonts,
}

impl Painter<'_> {
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: Rgb, a: f32) {
        self.c.fill_rect(x * SCALE, y * SCALE, w * SCALE, h * SCALE, rgb, a);
    }
    fn rrect(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, fill: Option<(Rgb, f32)>, stroke: Option<Rgb>) {
        self.c.rounded_rect(
            x * SCALE,
            y * SCALE,
            w * SCALE,
            h * SCALE,
            r * SCALE as f32,
            fill,
            stroke,
        );
    }
    fn text(&mut self, bold: bool, size: f32, text: &str, x: i32, y: i32, rgb: Rgb) -> i32 {
        let w = self.c.text(
            self.fonts,
            bold,
            size * SCALE as f32,
            text,
            (x * SCALE) as f32,
            (y * SCALE) as f32,
            rgb,
        );
        (w / SCALE as f32).ceil() as i32
    }
    fn clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.c.set_clip(x * SCALE, y * SCALE, w * SCALE, h * SCALE);
    }
    fn unclip(&mut self) {
        self.c.reset_clip();
    }
    /// Draw an icon `GLYPH` points tall with its top-left at (x, y).
    fn glyph(&mut self, glyphs: &mut GlyphCache, family: Family, glyph: Glyph, x: i32, y: i32) {
        let bitmap = glyphs.get(family, glyph, (GLYPH * SCALE) as u32);
        let dx = (GLYPH * SCALE - bitmap.width as i32) / 2;
        let dy = (GLYPH * SCALE - bitmap.height as i32) / 2;
        self.c.blit(&bitmap, x * SCALE + dx, y * SCALE + dy, 1.0);
    }
    fn keycap(&mut self, label: &str, x: i32, y: i32, w: i32, selected: bool) {
        let h = 16;
        if selected {
            self.rrect(x - 2, y - 2, w + 4, h + 4, 5.0, None, Some(GOLD));
        }
        self.rrect(x, y, w, h, 3.0, Some((KEYCAP, 1.0)), None);
        self.rect(x + 1, y + h - 2, w - 2, 2, (140, 134, 168), 1.0);
        let tw = width(self.fonts, true, SUB_SIZE, label);
        self.text(true, SUB_SIZE, label, x + (w - tw) / 2, y + 2, KEYCAP_TEXT);
    }
    fn footer_hint(&mut self, glyphs: &mut GlyphCache, family: Family, x: &mut i32, y: i32, icons: &[Glyph], keys: &[&str], label: &str) {
        // Drawn right to left: label, then its icons before it.
        let lw = width(self.fonts, false, FOOT_SIZE, label);
        *x -= lw;
        self.text(false, FOOT_SIZE, label, *x, y + 3, DIM);
        *x -= 4;
        for icon in icons.iter().rev() {
            *x -= GLYPH;
            self.glyph(glyphs, family, *icon, *x, y);
            *x -= 1;
        }
        for key in keys.iter().rev() {
            let w = key_cell_w(self.fonts, key);
            *x -= w;
            self.keycap(key, *x, y, w, false);
            *x -= 3;
        }
        *x -= 14;
    }
}

/// Everything the menu drawing needs besides the menu.
pub struct Look {
    pub family: Family,
    /// The keyboard was used more recently than a controller: hints show
    /// keys, not buttons.
    pub keyboard: bool,
}

/// The whole menu, as a (480·SCALE)×(320·SCALE) bitmap.
pub fn render(
    fonts: &Fonts,
    glyphs: &mut GlyphCache,
    menu: &mut Menu,
    settings: &Settings,
    info: &Info,
    look: &Look,
) -> Bitmap {
    let mut p = Painter {
        c: Canvas::new(W * SCALE, H * SCALE),
        fonts,
    };
    let family = look.family;
    // The paused game, dimmed.
    p.rect(0, 0, W, H, (5, 4, 10), 0.6);
    p.rrect(PANEL_X, PANEL_Y, PANEL_W, PANEL_H, 8.0, Some((PANEL, 0.96)), Some(LINE));

    // Header.
    p.text(true, TITLE_SIZE, "SETUP", PANEL_X + 14, PANEL_Y + 5, GOLD);
    let note = match menu.mode {
        Mode::WaitPad(_) | Mode::WaitKey(..) => "Remapping",
        Mode::Tester { .. } => "Button tester",
        _ => "Game paused",
    };
    let nw = width(fonts, false, FOOT_SIZE, note);
    p.text(false, FOOT_SIZE, note, PANEL_X + PANEL_W - 14 - nw, PANEL_Y + 9, DIM);
    p.rect(PANEL_X, BODY_Y - 1, PANEL_W, 1, LINE, 1.0);
    p.rect(PANEL_X, BODY_Y + BODY_H, PANEL_W, 1, LINE, 1.0);
    p.rect(PANEL_X + RAIL_W, BODY_Y, 1, BODY_H, LINE, 1.0);

    // The player's cursor look (Setup > Game > Cursor), shown here too so
    // the menu previews it.
    let cursor = Cursor::of(settings);

    // Tabs.
    for (i, tab) in menu.tabs().iter().enumerate() {
        let y = BODY_Y + 4 + i as i32 * TAB_H;
        let selected = i == menu.tab;
        if selected {
            let strong = menu.focus == Focus::Tabs && menu.mode == Mode::Normal;
            p.rect(PANEL_X + 1, y, RAIL_W - 1, TAB_H, SEL, if strong { 1.0 } else { 0.55 });
            p.rect(PANEL_X + 1, y, 3, TAB_H, if strong { cursor.rgb } else { VIOLET }, 1.0);
            if strong {
                cursor.outline(&mut p, PANEL_X + 1, y, RAIL_W - 1, TAB_H, 0.0);
            }
        }
        let rgb = if selected { INK } else { DIM };
        p.text(selected, TAB_SIZE, tab.label(), PANEL_X + 12, y + 4, rgb);
    }

    // Rows (or the tester).
    p.clip(PANEL_X + RAIL_W + 1, BODY_Y, PANEL_W - RAIL_W - 1, BODY_H);
    if let Mode::Tester { last } = &menu.mode {
        let cx = PANE_X + PANE_W / 2;
        // A button is drawn as its icon, before "→ what it does".
        let (icon, text) = match last {
            Some((Tested::Button(b), does)) => (Some(pad::glyph_of(*b)), format!("→ {does}")),
            Some((Tested::Key(k), does)) => (None, format!("{k} → {does}")),
            None => (None, "…".to_string()),
        };
        let lines = [
            ("Press any button or key", true, LABEL_SIZE + 2.0, INK),
            (text.as_str(), true, TITLE_SIZE, GOLD),
            ("Select, F2 or a tap leaves the tester", false, SUB_SIZE, DIM),
        ];
        let mut y = BODY_Y + BODY_H / 2 - 34;
        for (i, (text, bold, size, rgb)) in lines.into_iter().enumerate() {
            let w = width(fonts, bold, size, text);
            let gap = if i == 1 && icon.is_some() { GLYPH + 6 } else { 0 };
            let x = cx - (w + gap) / 2;
            if let (1, Some(glyph)) = (i, icon) {
                p.glyph(glyphs, family, glyph, x, y + (line_h(size) - GLYPH) / 2);
            }
            p.text(bold, size, text, x + gap, y, rgb);
            y += line_h(size) + 10;
        }
    } else {
        let rows = menu.rows(settings, info);
        let placed = layout(fonts, &rows);
        scroll_to_selection(menu, &placed);
        for (row, place) in rows.iter().zip(placed.iter()) {
            let y = BODY_Y + place.y - menu.scroll;
            if y + place.h < BODY_Y || y > BODY_Y + BODY_H {
                continue;
            }
            draw_row(&mut p, glyphs, family, menu, row, place, y, cursor);
        }
    }
    p.unclip();

    // Footer.
    let foot_y = BODY_Y + BODY_H + 4;
    let mut x = PANEL_X + PANEL_W - 12;
    let kb = look.keyboard;
    let icon = |role| pad::glyph_of(settings.pad.button(role));
    let key_of = |role| {
        settings.keys.keys(role)[0]
            .map(key_label)
            .unwrap_or_default()
    };
    use super::pad::Role;
    let (confirm_icon, back_icon) = (icon(Role::Confirm), icon(Role::Back));
    let (confirm_key, back_key) = (key_of(Role::Confirm), key_of(Role::Back));
    let hint = |p: &mut Painter, glyphs: &mut GlyphCache, x: &mut i32, role_icon: &[Glyph], role_key: &[&str], label: &str| {
        if kb {
            p.footer_hint(glyphs, family, x, foot_y, &[], role_key, label);
        } else {
            p.footer_hint(glyphs, family, x, foot_y, role_icon, &[], label);
        }
    };
    match &menu.mode {
        Mode::WaitPad(_) => {
            p.footer_hint(glyphs, family, &mut x, foot_y, &[Glyph::Select], &[], "Cancel");
        }
        Mode::WaitKey(..) => {
            p.footer_hint(glyphs, family, &mut x, foot_y, &[], &["F2"], "Cancel");
        }
        Mode::Tester { .. } => {}
        Mode::Dialog { .. } => {
            hint(&mut p, glyphs, &mut x, &[back_icon], &[back_key.as_str()], "Cancel");
            hint(&mut p, glyphs, &mut x, &[confirm_icon], &[confirm_key.as_str()], "Choose");
        }
        Mode::Normal => {
            let back = if menu.focus == Focus::Tabs { "Resume game" } else { "Back" };
            hint(&mut p, glyphs, &mut x, &[back_icon], &[back_key.as_str()], back);
            let confirm = if menu.focus == Focus::Tabs { "Open" } else { "Select" };
            hint(&mut p, glyphs, &mut x, &[confirm_icon], &[confirm_key.as_str()], confirm);
            let (l, r) = (icon(Role::PrevTab), icon(Role::NextTab));
            let (lk, rk) = (key_of(Role::PrevTab), key_of(Role::NextTab));
            hint(&mut p, glyphs, &mut x, &[l, r], &[lk.as_str(), rk.as_str()], "Tabs");
        }
    }
    if let Some(flash) = &menu.flash {
        p.text(true, FOOT_SIZE, flash, PANEL_X + 14, foot_y + 3, GOLD);
    }

    // The restart dialog, over everything.
    if let Mode::Dialog { change, yes } = &menu.mode {
        let (title, body, yes_label) = dialog_text(change, menu.platform, info);
        let (top, h, lines) = dialog_layout(fonts, &title, &body);
        p.rect(0, 0, W, H, (5, 4, 10), 0.55);
        p.rrect(DIALOG_X, top, DIALOG_W, h, 7.0, Some(((22, 18, 42), 1.0)), Some(GOLD));
        p.text(true, TITLE_SIZE - 2.0, &title, DIALOG_X + 14, top + 12, GOLD);
        let mut y = top + 14 + line_h(TITLE_SIZE - 2.0) + 6;
        for line in &lines {
            p.text(false, SUB_SIZE + 1.0, line, DIALOG_X + 14, y, (207, 203, 227));
            y += line_h(SUB_SIZE + 1.0);
        }
        let button_y = top + h - 14 - BUTTON_H;
        let [(yx, yw), (cx, cw)] = dialog_buttons(fonts, &yes_label);
        for (label, bx, bw, selected) in [(&yes_label[..], yx, yw, *yes), ("Cancel", cx, cw, !*yes)] {
            let fill = if selected { Some((SEL, 1.0)) } else { None };
            let stroke = if selected { cursor.rgb } else { LINE };
            p.rrect(bx, button_y, bw, BUTTON_H, 5.0, fill, Some(stroke));
            if selected {
                cursor.outline(&mut p, bx, button_y, bw, BUTTON_H, 5.0);
            }
            let tw = width(fonts, true, LABEL_SIZE, label);
            let rgb = if selected { INK } else { DIM };
            p.text(true, LABEL_SIZE, label, bx + (bw - tw) / 2, button_y + 4, rgb);
        }
    }
    p.c.bitmap
}

/// The cursor look, for the menu's own selection.
#[derive(Clone, Copy)]
struct Cursor {
    style: CursorStyle,
    rgb: Rgb,
}

impl Cursor {
    fn of(settings: &Settings) -> Cursor {
        let [r, g, b] = settings.cursor_colour.rgb();
        let byte = |c: f32| (c * 255.0).round() as u8;
        // Gold is the menu's own gold, so the default looks as it did.
        let rgb = match settings.cursor_colour {
            super::settings::CursorColour::Gold => GOLD,
            _ => (byte(r), byte(g), byte(b)),
        };
        Cursor {
            style: settings.cursor_style,
            rgb,
        }
    }

    /// Outlined: a line in the cursor colour round the selection; Bold: two.
    fn outline(self, p: &mut Painter, x: i32, y: i32, w: i32, h: i32, r: f32) {
        let lines = match self.style {
            CursorStyle::Game => 0,
            CursorStyle::Outline => 1,
            CursorStyle::Bold => 2,
        };
        for i in 0..lines {
            p.rrect(x + i, y + i, w - 2 * i, h - 2 * i, (r - i as f32).max(0.0), None, Some(self.rgb));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    p: &mut Painter,
    glyphs: &mut GlyphCache,
    family: Family,
    menu: &Menu,
    row: &Row,
    place: &Placed,
    y: i32,
    cursor: Cursor,
) {
    let fonts = p.fonts;
    match row {
        Row::Heading(text) => {
            p.text(true, HEADING_SIZE, &text.to_uppercase(), PANE_X + ROW_PAD, y + 7, VIOLET);
        }
        Row::Paragraph(text) => {
            let mut ly = y + 2;
            for line in wrap(fonts, false, SUB_SIZE, text, PANE_W - 2 * ROW_PAD) {
                p.text(false, SUB_SIZE, &line, PANE_X + ROW_PAD, ly, DIM);
                ly += line_h(SUB_SIZE);
            }
        }
        Row::Item {
            label,
            sub,
            value,
            restarts,
            ..
        } => {
            let item = place.item.unwrap_or(0);
            let selected = menu.focus == Focus::Rows && menu.row == item;
            if selected {
                p.rrect(PANE_X, y + 1, PANE_W, place.h - 2, 4.0, Some((SEL, 1.0)), None);
                cursor.outline(p, PANE_X, y + 1, PANE_W, place.h - 2, 4.0);
            }
            let waiting = selected && matches!(menu.mode, Mode::WaitPad(_) | Mode::WaitKey(..));
            let max_label = label_width(fonts, value);
            let label = fonts_ellipsize(fonts, label, max_label);
            p.text(false, LABEL_SIZE, &label, PANE_X + ROW_PAD, y + 4, INK);
            if let Some(sub) = sub {
                let mut sy = y + 4 + line_h(LABEL_SIZE);
                for line in wrap(fonts, false, SUB_SIZE, sub, sub_width(fonts, value)) {
                    p.text(false, SUB_SIZE, &line, PANE_X + ROW_PAD, sy, DIM);
                    sy += line_h(SUB_SIZE);
                }
            }
            let right = PANE_X + PANE_W - ROW_PAD;
            let vy = y + 3;
            if waiting {
                let text = if matches!(menu.mode, Mode::WaitKey(..)) {
                    "Press a key…"
                } else {
                    "Press a button…"
                };
                let w = width(fonts, true, LABEL_SIZE, text);
                p.text(true, LABEL_SIZE, text, right - w, vy + 1, GOLD);
                return;
            }
            let mut x = right;
            match value {
                Value::None => (),
                Value::Text(t) => {
                    let w = width(fonts, true, LABEL_SIZE, t);
                    p.text(true, LABEL_SIZE, t, right - w, vy + 1, INK);
                    x = right - w;
                }
                Value::Buttons { buttons, joined } => {
                    for (i, b) in buttons.iter().enumerate().rev() {
                        x -= GLYPH;
                        let glyph = match b {
                            Glyphish::Button(b) => pad::glyph_of(*b),
                            Glyphish::DPadAll => Glyph::DPadAll,
                            Glyphish::StickLeft => Glyph::StickLeft,
                        };
                        p.glyph(glyphs, family, glyph, x, vy);
                        if i > 0 {
                            if *joined {
                                x -= 12;
                                p.text(true, LABEL_SIZE, "+", x + 3, vy + 1, DIM);
                            } else {
                                x -= 3;
                            }
                        }
                    }
                }
                Value::Keys(keys) => {
                    for (slot, &(cx, cw)) in place.key_cells.iter().enumerate() {
                        let label = keys.get(slot).map_or("", String::as_str);
                        let on = selected && menu.key_slot == slot;
                        if label.is_empty() {
                            p.rrect(PANE_X + cx, vy, cw, 16, 3.0, None, Some(if on { GOLD } else { LINE }));
                        } else {
                            p.keycap(label, PANE_X + cx, vy, cw, on);
                        }
                    }
                    x = PANE_X + place.key_cells.first().map_or(0, |c| c.0);
                }
            }
            if *restarts && selected {
                let note = "Restarts the game";
                let w = width(fonts, false, SUB_SIZE, note);
                p.text(false, SUB_SIZE, note, x - 8 - w, vy + 3, GOLD);
            }
        }
    }
}

fn fonts_ellipsize(fonts: &Fonts, text: &str, max: i32) -> String {
    fonts.ellipsize(false, LABEL_SIZE * SCALE as f32, text, (max * SCALE) as f32)
}

/// The "Password" pill over the shop's top menu: the button or key that
/// opens the password entry (the game's only way in is a tap on the
/// shopkeeper, which a controller can't make).
#[derive(Clone, Debug, PartialEq)]
pub enum Pill {
    Button(Family, Glyph),
    Key(String),
}

/// What's drawn over the running game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    /// The first-run toast.
    pub toast: bool,
    /// The FPS counter, rounded.
    pub fps: Option<u32>,
    pub pill: Option<Pill>,
}

const PILL_LABEL: &str = "Password";
const PILL_H: i32 = 26;

/// The pill's rectangle: the bottom-left corner, below the shopkeeper and
/// clear of the shop's menu buttons.
fn pill_rect(fonts: &Fonts, pill: &Pill) -> (i32, i32, i32, i32) {
    let icon_w = match pill {
        Pill::Button(..) => GLYPH,
        Pill::Key(key) => key_cell_w(fonts, key),
    };
    let w = 8 + icon_w + 6 + width(fonts, true, LABEL_SIZE, PILL_LABEL) + 12;
    (8, H - PILL_H - 8, w, PILL_H)
}

/// The first-run toast, the FPS counter and the password pill, over the
/// running game. `None` if there's nothing to show.
pub fn render_hud(fonts: &Fonts, glyphs: &mut GlyphCache, hud: &Hud) -> Option<Bitmap> {
    if *hud == Hud::default() {
        return None;
    }
    let mut p = Painter {
        c: Canvas::new(W * SCALE, H * SCALE),
        fonts,
    };
    if let Some(fps) = hud.fps {
        let text = format!("{fps} FPS");
        let w = width(fonts, true, SUB_SIZE, &text) + 10;
        p.rrect(W - w - 4, 4, w, 15, 4.0, Some(((0, 0, 0), 0.6)), None);
        p.text(true, SUB_SIZE, &text, W - w + 1, 6, INK);
    }
    if let Some(pill) = &hud.pill {
        let (x, y, w, h) = pill_rect(fonts, pill);
        p.rrect(x, y, w, h, (h / 2) as f32, Some(((20, 17, 36), 0.8)), Some(DIM));
        let mut ix = x + 8;
        match pill {
            Pill::Button(family, glyph) => {
                p.glyph(glyphs, *family, *glyph, ix, y + (h - GLYPH) / 2);
                ix += GLYPH;
            }
            Pill::Key(key) => {
                let kw = key_cell_w(fonts, key);
                p.keycap(key, ix, y + (h - 16) / 2, kw, false);
                ix += kw;
            }
        }
        let text_y = y + (h - line_h(LABEL_SIZE)) / 2;
        p.text(true, LABEL_SIZE, PILL_LABEL, ix + 6, text_y, INK);
    }
    if hud.toast {
        let toast = super::setup::REOPEN_TOAST;
        let max = W - 60;
        let lines = wrap(fonts, false, SUB_SIZE + 1.0, toast, max - 24);
        let text_w = lines
            .iter()
            .map(|l| width(fonts, false, SUB_SIZE + 1.0, l))
            .max()
            .unwrap_or(0);
        let w = text_w + 24;
        let h = lines.len() as i32 * line_h(SUB_SIZE + 1.0) + 14;
        let x = (W - w) / 2;
        let y = H - h - 12;
        p.rrect(x, y, w, h, 6.0, Some(((20, 17, 36), 0.95)), Some(GOLD));
        let mut ly = y + 7;
        for line in &lines {
            let lw = width(fonts, false, SUB_SIZE + 1.0, line);
            p.text(false, SUB_SIZE + 1.0, line, x + (w - lw) / 2, ly, INK);
            ly += line_h(SUB_SIZE + 1.0);
        }
    }
    Some(p.c.bitmap)
}

#[cfg(test)]
mod tests {
    use super::super::setup_menu::{tabs, Menu, Platform, Tab};
    use super::*;
    use crate::options::ConfirmButton;

    fn info() -> Info {
        Info {
            game_file: Some((
                "Song Summoner The Unsung Heroes Encore.ipa".into(),
                "/sdcard/SongSummoner".into(),
                213,
            )),
            folders: vec![("Music (Internal storage)".into(), Some(1284))],
            data_dir: "/sdcard/SongSummoner".into(),
            version: "0.2.3".into(),
            fullscreen: false,
            latest_backup: None,
            save_folder: None,
        }
    }

    #[test]
    fn every_label_fits_beside_its_value() {
        let fonts = Fonts::load();
        let settings = Settings::defaults(ConfirmButton::South);
        let info = info();
        for platform in [Platform::Desktop, Platform::Android] {
            for tab in tabs(platform) {
                for row in super::super::setup_menu::rows(tab, platform, &settings, &info) {
                    if let Row::Item { label, value, .. } = &row {
                        // File and folder names may be cut short; nothing
                        // else should need to be.
                        if matches!(tab, Tab::Game | Tab::Music) && label.contains(['.', '(', '/']) {
                            continue;
                        }
                        let room = label_width(&fonts, value);
                        let w = width(&fonts, false, LABEL_SIZE, label);
                        assert!(w <= room, "{tab:?}: {label:?} is {w} wide, room {room}");
                    }
                }
            }
        }
    }

    #[test]
    fn all_tabs_fit_the_rail() {
        let fonts = Fonts::load();
        for tab in tabs(Platform::Desktop) {
            assert!(width(&fonts, true, TAB_SIZE, tab.label()) + 20 < RAIL_W, "{tab:?}");
        }
        assert!(BODY_Y + 4 + tabs(Platform::Desktop).len() as i32 * TAB_H <= BODY_Y + BODY_H);
    }

    #[test]
    fn wrapping_keeps_every_word() {
        let fonts = Fonts::load();
        let text = super::super::credits::NOT_AFFILIATED;
        let lines = wrap(&fonts, false, SUB_SIZE, text, 200);
        assert!(lines.len() > 1);
        assert_eq!(lines.join(" "), text.split_whitespace().collect::<Vec<_>>().join(" "));
        for line in &lines {
            assert!(width(&fonts, false, SUB_SIZE, line) <= 200 || !line.contains(' '));
        }
    }

    #[test]
    fn taps_hit_what_is_drawn() {
        let fonts = Fonts::load();
        let settings = Settings::defaults(ConfirmButton::South);
        let info = info();
        let menu = Menu::new(Platform::Android, ConfirmButton::South);
        // The second tab.
        let y = (BODY_Y + 4 + TAB_H + TAB_H / 2) as f32;
        assert_eq!(hit(&fonts, &menu, &settings, &info, (PANEL_X + 20) as f32, y), Hit::Tab(1));
        // The corner of the screen is outside the panel.
        assert_eq!(hit(&fonts, &menu, &settings, &info, 2.0, 2.0), Hit::Outside);
        // The first row of the pane is the first item (the game file).
        let placed = layout(&fonts, &menu.rows(&settings, &info));
        let first = placed.iter().find(|p| p.item == Some(0)).unwrap();
        let y = (BODY_Y + first.y + first.h / 2) as f32;
        assert_eq!(
            hit(&fonts, &menu, &settings, &info, (PANE_X + 30) as f32, y),
            Hit::Row(0, None)
        );
    }

    #[test]
    fn a_dragged_or_nudged_view_stays_put_within_the_content() {
        let placed: Vec<Placed> = (0..20)
            .map(|i| Placed {
                y: i * 30,
                h: 30,
                item: Some(i as usize),
                key_cells: Vec::new(),
            })
            .collect();
        let max = 20 * 30 - BODY_H;
        let mut menu = Menu::new(Platform::Desktop, ConfirmButton::South);
        menu.focus = Focus::Rows;
        menu.row = 0;
        // Dragged down the list: the selected first row is off screen, and
        // the view stays.
        menu.drag(0, -150);
        scroll_to_selection(&mut menu, &placed);
        assert_eq!(menu.scroll, 150);
        // Dragged past the end: clamped.
        menu.drag(0, -10_000);
        scroll_to_selection(&mut menu, &placed);
        assert_eq!(menu.scroll, max);
        // A nudge is applied once, then cleared.
        menu.scroll = 0;
        menu.nudge = 40;
        scroll_to_selection(&mut menu, &placed);
        assert_eq!((menu.scroll, menu.nudge), (40, 0));
        // Following again brings the selection back.
        menu.follow = true;
        scroll_to_selection(&mut menu, &placed);
        assert_eq!(menu.scroll, 0);
    }

    #[test]
    fn the_password_pill_sits_bottom_left_clear_of_the_shop_menu() {
        let fonts = Fonts::load();
        let mut glyphs = GlyphCache::default();
        for pill in [
            Pill::Button(Family::Xbox, Glyph::FaceNorth),
            Pill::Key("I".into()),
            Pill::Key("Right Shift".into()),
        ] {
            let (x, y, w, h) = pill_rect(&fonts, &pill);
            // On screen, and left of the shop's menu buttons (x 240-400),
            // over the empty corner below the shopkeeper.
            assert!(x >= 4 && x + w < 240, "{pill:?}: {x} {w}");
            assert!(y >= 240 && y + h <= H - 4, "{pill:?}: {y} {h}");
            let hud = Hud {
                toast: false,
                fps: None,
                pill: Some(pill),
            };
            assert!(render_hud(&fonts, &mut glyphs, &hud).is_some());
        }
        // Nothing to show, nothing drawn.
        let empty = Hud::default();
        assert!(render_hud(&fonts, &mut glyphs, &empty).is_none());
    }

    #[test]
    fn the_whole_menu_renders_at_scale() {
        let fonts = Fonts::load();
        let mut glyphs = GlyphCache::default();
        let settings = Settings::defaults(ConfirmButton::South);
        let info = info();
        let mut menu = Menu::new(Platform::Desktop, ConfirmButton::South);
        let look = Look {
            family: Family::Xbox,
            keyboard: false,
        };
        let bitmap = render(&fonts, &mut glyphs, &mut menu, &settings, &info, &look);
        assert_eq!(bitmap.width as i32, W * SCALE);
        assert_eq!(bitmap.height as i32, H * SCALE);
    }
}
