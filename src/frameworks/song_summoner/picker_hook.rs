/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Host overrides that swap the game's iPod picker for ours.
//!
//! The game drives its picker through six methods of
//! `IPDMediaPickerController` (the base class of the `iPodView2` it
//! creates); see dev-docs/song-summoner-re.md §2 for the contract. We
//! replace exactly those, so the game's own state machine keeps doing the
//! hiding, the confirmation panel and the re-showing.
//!
//! `-[ViewManager start_iPodView]` is replaced too: the original shows a
//! threaded loading screen that only fills caches for the game's old
//! tables, then calls `animationDidStop`. We call that straight away.

use super::picker_view;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{id, msg, msg_super, Override, SEL};
use crate::Environment;

/// For `msg_super!`: the class these implementations are installed on.
const _OBJC_CURRENT_CLASS: &str = "IPDMediaPickerController";

fn init_with_selection_index(env: &mut Environment, this: id, tab: NSInteger) -> id {
    // The game saves the last tab and passes it back; it asserts it's 0–3.
    let tab = tab.clamp(0, 3) as usize;
    picker_view::create(env, this, tab);
    this
}

fn get_view(env: &mut Environment, this: id) -> id {
    picker_view::view_of(env, this)
}

fn set_hidden(env: &mut Environment, this: id, hidden: bool) {
    picker_view::set_hidden(env, this, hidden);
}

fn hidden(env: &mut Environment, this: id) -> bool {
    let view = picker_view::view_of(env, this);
    if view == crate::objc::nil {
        return true;
    }
    msg![env; view isHidden]
}

fn rotate(env: &mut Environment, this: id, angle: u32) {
    // The game is landscape-only; the argument is its current angle. Our
    // view just follows the window's orientation.
    log_dbg!("picker: rotate({:#x})", angle);
    let view = picker_view::view_of(env, this);
    if view != crate::objc::nil {
        picker_view::apply_rotation(env, view);
    }
}

fn selected_tab_index(env: &mut Environment, this: id) -> NSInteger {
    picker_view::tab_of(env, this) as NSInteger
}

fn dealloc(env: &mut Environment, this: id) {
    picker_view::destroy(env, this);
    msg_super![env; this dealloc]
}

fn start_ipod_view(env: &mut Environment, this: id) {
    log!("picker: skipping the game's loading screen");
    () = msg![env; this animationDidStop];
}

pub const OVERRIDES: &[Override] = &[
    Override {
        class: "IPDMediaPickerController",
        selector: "initWithSelectionIndex:",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL, tab: NSInteger| -> id {
            init_with_selection_index(env, this, tab)
        }) as fn(&mut Environment, id, SEL, NSInteger) -> id),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "getView",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL| -> id { get_view(env, this) })
            as fn(&mut Environment, id, SEL) -> id),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "setHidden:",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL, hide: bool| {
            set_hidden(env, this, hide)
        }) as fn(&mut Environment, id, SEL, bool)),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "hidden",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL| -> bool { hidden(env, this) })
            as fn(&mut Environment, id, SEL) -> bool),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "rotate:",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL, angle: u32| rotate(env, this, angle))
            as fn(&mut Environment, id, SEL, u32)),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "selectedTabIndex",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL| -> NSInteger {
            selected_tab_index(env, this)
        }) as fn(&mut Environment, id, SEL) -> NSInteger),
    },
    Override {
        class: "IPDMediaPickerController",
        selector: "dealloc",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL| dealloc(env, this))
            as fn(&mut Environment, id, SEL)),
    },
    Override {
        class: "ViewManager",
        selector: "start_iPodView",
        class_method: false,
        imp: &((|env: &mut Environment, this: id, _: SEL| start_ipod_view(env, this))
            as fn(&mut Environment, id, SEL)),
    },
];
