/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal `UITableView`, `UITableViewCell` and `UITableViewController`.
//!
//! Just enough for apps that drive a simple list: one section, rows of a
//! fixed height, cells vended by the data source for the visible rows only,
//! and `tableView:didSelectRowAtIndexPath:` on tap. No cell reuse, headers or
//! editing.

use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_array, NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil,
    objc_classes, release, retain, ClassExports, NSZonePtr, SEL,
};
use std::collections::BTreeMap;

type UITableViewStyle = NSInteger; // 0 = plain, 1 = grouped
type UITableViewCellStyle = NSInteger;
type UITableViewCellSeparatorStyle = NSInteger;
type UITableViewCellAccessoryType = NSInteger;
type UITableViewCellSelectionStyle = NSInteger;
type UITableViewRowAnimation = NSInteger;
type UITableViewScrollPosition = NSInteger;

const DEFAULT_ROW_HEIGHT: f32 = 44.0;
/// A touch that moves further than this (in points) is a scroll, not a tap.
const TAP_SLOP: f32 = 10.0;

#[derive(Default)]
struct UITableViewHostObject {
    superclass: super::ui_scroll_view::UIScrollViewHostObject,
    /// Non-retaining (weak) reference per UIKit semantics.
    data_source: id,
    /// Non-retaining (weak) reference per UIKit semantics.
    delegate: id,
    row_height: f32,
    style: UITableViewStyle,
    /// Row count the data source last reported, `None` before the first
    /// `reloadData`.
    total_rows: Option<NSUInteger>,
    /// Cells for the rows currently in (or next to) the viewport, retained.
    /// Rows scrolled out of view are released, so a long list never holds
    /// more than a screenful of cells.
    cells_by_row: BTreeMap<NSUInteger, id>,
    /// `NSIndexPath*`, retained.
    selected_index_path: id,
    /// Where the current touch started (in the table's own coordinates,
    /// ignoring scrolling), so cells can tell a tap from a scroll.
    touch_began_loc: CGPoint,
    is_dragging: bool,
}
impl_HostObject_with_superclass!(UITableViewHostObject);

#[derive(Default)]
struct UITableViewCellHostObject {
    superclass: super::UIViewHostObject,
    /// `UILabel*`, retained.
    text_label: id,
    /// `UILabel*`, retained.
    detail_text_label: id,
    /// `UIImageView*`, retained.
    image_view: id,
    /// Owning `UITableView*`, weak. Used to find the delegate on tap.
    table_view: id,
    row: NSUInteger,
    accessory_type: UITableViewCellAccessoryType,
    selection_style: UITableViewCellSelectionStyle,
    /// `NSString*`, retained.
    reuse_identifier: id,
}
impl_HostObject_with_superclass!(UITableViewCellHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITableView: UIScrollView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(UITableViewHostObject {
        row_height: DEFAULT_ROW_HEIGHT,
        ..Default::default()
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame style:(UITableViewStyle)style {
    let this: id = msg![env; this initWithFrame:frame];
    env.objc.borrow_mut::<UITableViewHostObject>(this).style = style;
    this
}

- (())dealloc {
    let cells = std::mem::take(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(this).cells_by_row,
    );
    for (_, cell) in cells {
        release(env, cell);
    }
    let selected = env.objc.borrow::<UITableViewHostObject>(this).selected_index_path;
    release(env, selected);
    msg_super![env; this dealloc]
}

- (id)dataSource {
    env.objc.borrow::<UITableViewHostObject>(this).data_source
}
- (())setDataSource:(id)data_source {
    env.objc.borrow_mut::<UITableViewHostObject>(this).data_source = data_source;
    // iOS queries the data source on the next layout pass. touchHLE has no
    // layout pass, so load now or the table would stay blank until the app
    // happens to call reloadData itself.
    if data_source != nil {
        () = msg![env; this reloadData];
    }
}

- (id)delegate {
    env.objc.borrow::<UITableViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UITableViewHostObject>(this).delegate = delegate;
}

- (UITableViewStyle)style {
    env.objc.borrow::<UITableViewHostObject>(this).style
}

- (f32)rowHeight {
    env.objc.borrow::<UITableViewHostObject>(this).row_height
}
- (())setRowHeight:(f32)h {
    env.objc.borrow_mut::<UITableViewHostObject>(this).row_height = h;
}

- (())setSeparatorStyle:(UITableViewCellSeparatorStyle)_s {}
- (())setSeparatorColor:(id)_c {}
- (())setBackgroundView:(id)_v {}
- (())setAllowsSelection:(bool)_v {}
- (())setSectionHeaderHeight:(f32)_h {}
- (())setSectionFooterHeight:(f32)_h {}
- (())setTableHeaderView:(id)_v {}
- (())setTableFooterView:(id)_v {}

// No cell pool: callers always take their "no cell found" path and make a
// fresh one.
- (id)dequeueReusableCellWithIdentifier:(id)_identifier { nil }

- (id)cellForRowAtIndexPath:(id)index_path {
    let row: NSUInteger = msg![env; index_path row];
    env.objc
        .borrow::<UITableViewHostObject>(this)
        .cells_by_row
        .get(&row)
        .copied()
        .unwrap_or(nil)
}

- (id)indexPathForCell:(id)cell {
    let found = env
        .objc
        .borrow::<UITableViewHostObject>(this)
        .cells_by_row
        .iter()
        .find(|&(_, &c)| c == cell)
        .map(|(&row, _)| row);
    match found {
        Some(row) => msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32],
        None => nil,
    }
}

- (NSInteger)numberOfSections { 1 }
- (NSInteger)numberOfRowsInSection:(NSInteger)_section {
    env.objc
        .borrow::<UITableViewHostObject>(this)
        .total_rows
        .unwrap_or(0) as NSInteger
}

- (())deselectRowAtIndexPath:(id)_index_path animated:(bool)_animated {
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(this).selected_index_path,
        nil,
    );
    release(env, old);
}

- (id)indexPathForSelectedRow {
    env.objc.borrow::<UITableViewHostObject>(this).selected_index_path
}

- (id)visibleCells {
    let cells: Vec<id> = env
        .objc
        .borrow::<UITableViewHostObject>(this)
        .cells_by_row
        .values()
        .copied()
        .collect();
    for &cell in &cells {
        retain(env, cell);
    }
    let arr = ns_array::from_vec(env, cells);
    autorelease(env, arr)
}
- (id)indexPathsForVisibleRows {
    let rows: Vec<NSUInteger> = env
        .objc
        .borrow::<UITableViewHostObject>(this)
        .cells_by_row
        .keys()
        .copied()
        .collect();
    let mut paths = Vec::with_capacity(rows.len());
    for row in rows {
        let path: id = msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32];
        retain(env, path);
        paths.push(path);
    }
    let arr = ns_array::from_vec(env, paths);
    autorelease(env, arr)
}
- (id)indexPathForRowAtPoint:(CGPoint)point {
    let &UITableViewHostObject { row_height, total_rows, .. } = env.objc.borrow(this);
    let total = total_rows.unwrap_or(0);
    if row_height <= 0.0 || point.y < 0.0 {
        return nil;
    }
    let row = (point.y / row_height) as NSUInteger;
    if row >= total {
        return nil;
    }
    msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32]
}

- (())scrollToRowAtIndexPath:(id)path
              atScrollPosition:(UITableViewScrollPosition)_pos
                      animated:(bool)_animated {
    let row: NSUInteger = msg![env; path row];
    let row_height = env.objc.borrow::<UITableViewHostObject>(this).row_height;
    let bounds: CGRect = msg![env; this bounds];
    let content_size: CGSize = msg![env; this contentSize];
    let max_y = (content_size.height - bounds.size.height).max(0.0);
    let y = (row as f32 * row_height).min(max_y);
    let offset = CGPoint { x: 0.0, y };
    () = msg![env; this setContentOffset:offset];
}
- (())selectRowAtIndexPath:(id)path
                    animated:(bool)_animated
              scrollPosition:(UITableViewScrollPosition)_pos {
    retain(env, path);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(this).selected_index_path,
        path,
    );
    release(env, old);
}
- (())beginUpdates {}
- (())endUpdates {}
- (())insertRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {
    () = msg![env; this reloadData];
}
- (())deleteRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {
    () = msg![env; this reloadData];
}
- (())reloadRowsAtIndexPaths:(id)_paths withRowAnimation:(UITableViewRowAnimation)_a {
    () = msg![env; this reloadData];
}

// Scrolling moves the bounds, which changes which rows are visible.
- (())setContentOffset:(CGPoint)offset {
    () = msg_super![env; this setContentOffset:offset];
    layout_visible_cells(env, this);
}
- (())setBounds:(CGRect)bounds {
    () = msg_super![env; this setBounds:bounds];
    layout_visible_cells(env, this);
}
- (())setFrame:(CGRect)frame {
    () = msg_super![env; this setFrame:frame];
    layout_visible_cells(env, this);
}

// Touches that land on a cell reach us through the responder chain. We only
// watch them to tell taps from scrolls; UIScrollView does the scrolling.
- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return;
    }
    let loc = touch_location_unscrolled(env, this, touch);
    let host = env.objc.borrow_mut::<UITableViewHostObject>(this);
    host.touch_began_loc = loc;
    host.is_dragging = false;
}
- (())touchesMoved:(id)touches withEvent:(id)event {
    let touch: id = msg![env; touches anyObject];
    if touch != nil {
        let loc = touch_location_unscrolled(env, this, touch);
        let began = env.objc.borrow::<UITableViewHostObject>(this).touch_began_loc;
        let moved = (loc.x - began.x).abs().max((loc.y - began.y).abs());
        if moved > TAP_SLOP {
            env.objc.borrow_mut::<UITableViewHostObject>(this).is_dragging = true;
        }
    }
    () = msg_super![env; this touchesMoved:touches withEvent:event];
}

- (())reloadData {
    let data_source = env.objc.borrow::<UITableViewHostObject>(this).data_source;
    if data_source == nil {
        return;
    }

    let num_sections_sel: SEL = env
        .objc
        .register_host_selector("numberOfSectionsInTableView:".to_string(), &mut env.mem);
    let num_sections: NSInteger =
        if msg![env; data_source respondsToSelector:num_sections_sel] {
            msg![env; data_source numberOfSectionsInTableView:this]
        } else {
            1
        };
    let rows: NSInteger = if num_sections > 0 {
        msg![env; data_source tableView:this numberOfRowsInSection:0i32]
    } else {
        0
    };
    let rows = rows.max(0) as NSUInteger;
    env.objc.borrow_mut::<UITableViewHostObject>(this).total_rows = Some(rows);

    let row_height = env.objc.borrow::<UITableViewHostObject>(this).row_height;
    let bounds: CGRect = msg![env; this bounds];
    let content_size = CGSize {
        width: bounds.size.width,
        height: rows as f32 * row_height,
    };
    () = msg![env; this setContentSize:content_size];

    // The data behind every row may have changed, so drop all cells and let
    // the data source vend fresh ones.
    let old_cells = std::mem::take(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(this).cells_by_row,
    );
    for (_, cell) in old_cells {
        () = msg![env; cell removeFromSuperview];
        release(env, cell);
    }
    layout_visible_cells(env, this);
}

@end

@implementation UITableViewCell: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UITableViewCellHostObject>::default(), &mut env.mem)
}

- (id)initWithStyle:(UITableViewCellStyle)_style
    reuseIdentifier:(id)reuse_identifier {
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 320.0, height: DEFAULT_ROW_HEIGHT },
    };
    let this: id = msg_super![env; this initWithFrame:frame];

    retain(env, reuse_identifier);
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).reuse_identifier =
        reuse_identifier;

    let label_frame = CGRect {
        origin: CGPoint { x: 10.0, y: 0.0 },
        size: CGSize { width: frame.size.width - 20.0, height: frame.size.height },
    };
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:label_frame];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).text_label = label;
    () = msg![env; this addSubview:label];

    this
}

- (id)initWithFrame:(CGRect)frame
    reuseIdentifier:(id)reuse_identifier {
    let this: id = msg![env; this initWithStyle:0i32 reuseIdentifier:reuse_identifier];
    () = msg![env; this setFrame:frame];
    this
}

- (())dealloc {
    let &UITableViewCellHostObject {
        text_label,
        detail_text_label,
        image_view,
        reuse_identifier,
        ..
    } = env.objc.borrow(this);
    release(env, text_label);
    release(env, detail_text_label);
    release(env, image_view);
    release(env, reuse_identifier);
    msg_super![env; this dealloc]
}

- (id)textLabel {
    env.objc.borrow::<UITableViewCellHostObject>(this).text_label
}

- (id)detailTextLabel {
    // Made on first use, since most cells never need one.
    let existing = env.objc.borrow::<UITableViewCellHostObject>(this).detail_text_label;
    if existing != nil {
        return existing;
    }
    let bounds: CGRect = msg![env; this bounds];
    let frame = CGRect {
        origin: CGPoint { x: 10.0, y: bounds.size.height * 0.5 },
        size: CGSize {
            width: bounds.size.width - 20.0,
            height: bounds.size.height * 0.5,
        },
    };
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).detail_text_label = label;
    () = msg![env; this addSubview:label];
    label
}

- (id)imageView {
    let existing = env.objc.borrow::<UITableViewCellHostObject>(this).image_view;
    if existing != nil {
        return existing;
    }
    let bounds: CGRect = msg![env; this bounds];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: bounds.size.height, height: bounds.size.height },
    };
    let iv: id = msg_class![env; UIImageView alloc];
    let iv: id = msg![env; iv initWithFrame:frame];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; iv setBackgroundColor:clear];
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).image_view = iv;
    () = msg![env; this addSubview:iv];
    iv
}

- (id)contentView { this }

- (id)reuseIdentifier {
    env.objc.borrow::<UITableViewCellHostObject>(this).reuse_identifier
}

- (UITableViewCellAccessoryType)accessoryType {
    env.objc.borrow::<UITableViewCellHostObject>(this).accessory_type
}
- (())setAccessoryType:(UITableViewCellAccessoryType)t {
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).accessory_type = t;
}

- (UITableViewCellSelectionStyle)selectionStyle {
    env.objc.borrow::<UITableViewCellHostObject>(this).selection_style
}
- (())setSelectionStyle:(UITableViewCellSelectionStyle)s {
    env.objc.borrow_mut::<UITableViewCellHostObject>(this).selection_style = s;
}

- (())setSelected:(bool)_selected animated:(bool)_animated {}
- (())prepareForReuse {}

- (())touchesEnded:(id)_touches withEvent:(id)_event {
    let &UITableViewCellHostObject { table_view, row, .. } = env.objc.borrow(this);
    if table_view == nil {
        return;
    }
    let was_dragging = std::mem::take(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(table_view).is_dragging,
    );
    if was_dragging {
        return;
    }

    let idx: id = msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32];
    retain(env, idx);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UITableViewHostObject>(table_view).selected_index_path,
        idx,
    );
    release(env, old);

    let delegate: id = env.objc.borrow::<UITableViewHostObject>(table_view).delegate;
    if delegate == nil {
        return;
    }
    let sel: SEL = env.objc.register_host_selector(
        "tableView:didSelectRowAtIndexPath:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        () = msg![env; delegate tableView:table_view didSelectRowAtIndexPath:idx];
    }
}

@end

@implementation UITableViewController: UIViewController

- (id)initWithStyle:(UITableViewStyle)style {
    let this: id = msg_super![env; this initWithNibName:nil bundle:nil];
    let screen: id = msg_class![env; UIScreen mainScreen];
    let frame: CGRect = msg![env; screen applicationFrame];
    let table: id = msg_class![env; UITableView alloc];
    let table: id = msg![env; table initWithFrame:frame style:style];
    () = msg![env; table setDataSource:this];
    () = msg![env; table setDelegate:this];
    () = msg![env; this setView:table];
    release(env, table);
    this
}

// A UITableViewController's view is a UITableView. Real iOS makes it lazily
// in -loadView; the base UIViewController would make a plain UIView, and
// subclasses calling UITableView methods on self.view would then fail.
- (())loadView {
    let screen: id = msg_class![env; UIScreen mainScreen];
    let frame: CGRect = msg![env; screen applicationFrame];
    let table: id = msg_class![env; UITableView alloc];
    let table: id = msg![env; table initWithFrame:frame style:0i32];
    () = msg![env; table setDataSource:this];
    () = msg![env; table setDelegate:this];
    () = msg![env; this setView:table];
    release(env, table);
}

- (id)tableView {
    msg![env; this view]
}

- (())viewWillAppear:(bool)_animated {
    let v: id = msg![env; this view];
    () = msg![env; v reloadData];
}

@end

};

/// A touch's location in the table's coordinates minus the scroll offset,
/// so it stays put while the content scrolls under the finger.
fn touch_location_unscrolled(env: &mut crate::Environment, table: id, touch: id) -> CGPoint {
    let loc: CGPoint = msg![env; touch locationInView:table];
    let offset: CGPoint = msg![env; table contentOffset];
    CGPoint {
        x: loc.x - offset.x,
        y: loc.y - offset.y,
    }
}

/// Make sure exactly the rows in the viewport (plus one row either side)
/// have cells: release the ones that scrolled away and ask the data source
/// for the newly visible ones.
fn layout_visible_cells(env: &mut crate::Environment, table: id) {
    let &UITableViewHostObject {
        data_source,
        row_height,
        total_rows,
        ..
    } = env.objc.borrow(table);
    let total_rows = total_rows.unwrap_or(0);
    if data_source == nil || total_rows == 0 || row_height <= 0.0 {
        return;
    }
    let bounds: CGRect = msg![env; table bounds];

    let top = bounds.origin.y;
    let bottom = bounds.origin.y + bounds.size.height;
    let first = ((top / row_height).floor() as i64 - 1).max(0) as NSUInteger;
    let end = (((bottom / row_height).ceil() as i64) + 1).max(0) as NSUInteger;
    let end = end.min(total_rows);
    let first = first.min(end);

    let stale: Vec<(NSUInteger, id)> = env
        .objc
        .borrow::<UITableViewHostObject>(table)
        .cells_by_row
        .iter()
        .filter(|&(&row, _)| row < first || row >= end)
        .map(|(&row, &cell)| (row, cell))
        .collect();
    for (row, cell) in stale {
        env.objc
            .borrow_mut::<UITableViewHostObject>(table)
            .cells_by_row
            .remove(&row);
        () = msg![env; cell removeFromSuperview];
        release(env, cell);
    }

    for row in first..end {
        if env
            .objc
            .borrow::<UITableViewHostObject>(table)
            .cells_by_row
            .contains_key(&row)
        {
            continue;
        }
        let idx: id = msg_class![env; NSIndexPath indexPathForRow:row inSection:0u32];
        let cell: id = msg![env; data_source tableView:table cellForRowAtIndexPath:idx];
        if cell == nil {
            continue;
        }
        retain(env, cell);
        let frame = CGRect {
            origin: CGPoint { x: 0.0, y: row as f32 * row_height },
            size: CGSize { width: bounds.size.width, height: row_height },
        };
        () = msg![env; cell setFrame:frame];
        {
            let host = env.objc.borrow_mut::<UITableViewCellHostObject>(cell);
            host.table_view = table;
            host.row = row;
        }
        () = msg![env; table addSubview:cell];
        env.objc
            .borrow_mut::<UITableViewHostObject>(table)
            .cells_by_row
            .insert(row, cell);
    }
}
