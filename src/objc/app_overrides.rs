/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Replacing methods an app defines with host implementations.
//!
//! Some app features can't be emulated by implementing system classes,
//! because the app does the work in its own classes (Song Summoner builds
//! its iPod picker out of UIKit parts touchHLE doesn't have). For those, a
//! feature module lists `(class, selector, host implementation)` entries in
//! an [Override] table, and [ObjC::apply_app_overrides] installs them once
//! the app's classes are registered.
//!
//! Entries for classes the app doesn't define are skipped, so a table only
//! ever affects the app it was written for.

use super::{Class, ClassHostObject, HostIMP, ObjC, IMP};
use crate::mach_o::MachO;
use crate::mem::{ConstPtr, Mem, Ptr};
use std::collections::HashMap;

pub struct Override {
    /// An Objective-C class the app defines.
    pub class: &'static str,
    pub selector: &'static str,
    /// Replace a class method rather than an instance method.
    pub class_method: bool,
    pub imp: &'static dyn HostIMP,
}

/// Every override table. Add new feature tables here.
const TABLES: &[&[Override]] = &[crate::frameworks::song_summoner::OVERRIDES];

impl ObjC {
    /// For use by [crate::dyld], after the app's classes and categories are
    /// registered: install the host overrides for classes this app defines.
    pub fn apply_app_overrides(&mut self, bin: &MachO, mem: &mut Mem) {
        let Some(list) = bin.get_section("__objc_classlist") else {
            return;
        };
        let base: ConstPtr<Class> = Ptr::from_bits(list.addr);
        let mut app_classes: HashMap<String, Class> = HashMap::new();
        for i in 0..(list.size / 4) {
            let class = mem.read(base + i);
            if let Some(host) = self.class_host_object(class) {
                app_classes.insert(host.name.clone(), class);
            }
        }

        for entry in TABLES.iter().flat_map(|table| table.iter()) {
            let Some(&class) = app_classes.get(entry.class) else {
                continue;
            };
            let target = if entry.class_method {
                Self::read_isa(class, mem)
            } else {
                class
            };
            let sel = self.register_host_selector(entry.selector.to_string(), mem);

            // A subclass's own version of the method would hide the
            // replacement, so it goes too. That way an override applies to
            // the whole class tree, whichever subclass the app instantiates.
            let subclasses: Vec<(String, Class)> = app_classes
                .iter()
                .filter(|&(_, &other)| other != class && self.class_is_subclass_of(other, class))
                .map(|(name, &other)| (name.clone(), other))
                .collect();
            for (name, subclass) in subclasses {
                let subclass = if entry.class_method {
                    Self::read_isa(subclass, mem)
                } else {
                    subclass
                };
                if self
                    .borrow_mut::<ClassHostObject>(subclass)
                    .methods
                    .remove(&sel)
                    .is_some()
                {
                    log!(
                        "objc: override of {}[{} {}] also replaces {}'s version",
                        if entry.class_method { "+" } else { "-" },
                        entry.class,
                        entry.selector,
                        name
                    );
                }
            }

            let had_method = self
                .borrow_mut::<ClassHostObject>(target)
                .methods
                .insert(sel, IMP::Host(entry.imp))
                .is_some();
            log!(
                "objc: {}[{} {}] {} by a host implementation",
                if entry.class_method { "+" } else { "-" },
                entry.class,
                entry.selector,
                if had_method { "replaced" } else { "added" }
            );
        }
    }

    /// The class's [ClassHostObject], or `None` for fake/unimplemented
    /// substitutes.
    fn class_host_object(&self, class: Class) -> Option<&ClassHostObject> {
        self.get_host_object(class)?.as_any().downcast_ref()
    }
}
