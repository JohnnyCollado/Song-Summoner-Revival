/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGFont`

use super::cg_data_provider;
use super::cg_data_provider::CGDataProviderRef;
use crate::dyld::{export_c_func, FunctionExports};
use crate::font::Font;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::unichar;
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

// Note: on iOS SDK side this type is defined as a pointer to an opaque struct
pub(super) type CGFontRef = CFTypeRef;

type CGFontIndex = u16;
pub(super) type CGGlyph = CGFontIndex;

pub struct CGFontHostObject {
    pub font: Font,
}
impl HostObject for CGFontHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CGFont: NSObject
@end

};

fn CGFontCreateWithDataProvider(env: &mut Environment, provider: CGDataProviderRef) -> CGFontRef {
    let bytes = cg_data_provider::borrow_bytes(env, provider);
    let font = Font::from_vec(bytes.to_vec());
    let host_obj = Box::new(CGFontHostObject { font });
    let class = env.objc.get_known_class("_touchHLE_CGFont", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

pub fn CGFontRetain(env: &mut Environment, font: CGFontRef) -> CGFontRef {
    if !font.is_null() {
        CFRetain(env, font)
    } else {
        font
    }
}
pub fn CGFontRelease(env: &mut Environment, font: CGFontRef) {
    if !font.is_null() {
        CFRelease(env, font);
    }
}

// This is an undocumented API! But some apps still may call it
fn CGFontGetGlyphsForUnichars(
    env: &mut Environment,
    font: CGFontRef,
    chars: ConstPtr<unichar>,
    glyphs: MutPtr<CGGlyph>,
    length: GuestUSize,
) -> bool {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    for i in 0..length {
        let c: unichar = env.mem.read(chars + i);
        let x = font.glyph_id_for_char(c).0;
        env.mem.write(glyphs + i, x);
    }
    true
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGFontCreateWithDataProvider(_)),
    export_c_func!(CGFontRetain(_)),
    export_c_func!(CGFontRelease(_)),
    export_c_func!(CGFontGetGlyphsForUnichars(_, _, _, _)),
];
