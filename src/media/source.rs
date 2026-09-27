/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Opening a song's file.
//!
//! On the desktop a song's locator is a path relative to the music folder
//! named in `library/source.txt`. On Android it is a Storage Access
//! Framework `content://` URI, which only Java can open: we call the Kotlin
//! helper `MusicFiles.openFd` over JNI and wrap the file descriptor it hands
//! back. That opens no activity, so it's safe while touchHLE is running
//! (see the focus rule in CLAUDE.md).

use super::index::Song;
use crate::Environment;
use std::fs::File;
use std::io;

pub fn open(env: &mut Environment, song: &Song) -> io::Result<File> {
    open_locator(env, &song.locator)
}

#[cfg(not(target_os = "android"))]
fn open_locator(_env: &mut Environment, locator: &str) -> io::Result<File> {
    let root = std::fs::read_to_string(super::source_path())?;
    let root = std::path::PathBuf::from(root.trim());
    let mut path = root;
    for part in locator.split('/').filter(|p| !p.is_empty() && *p != "..") {
        path.push(part);
    }
    File::open(path)
}

#[cfg(target_os = "android")]
fn open_locator(env: &mut Environment, locator: &str) -> io::Result<File> {
    use std::os::fd::FromRawFd;

    let uri = locator.to_string();
    // ART only accepts JNI calls on the thread's real stack, not on the
    // coroutine stacks guest threads run on.
    let fd = env.on_parent_stack_in_coroutine(move |_, _| android::open_fd(&uri));
    match fd {
        Ok(fd) if fd >= 0 => Ok(unsafe { File::from_raw_fd(fd) }),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("MusicFiles.openFd couldn't open {locator}"),
        )),
        Err(e) => Err(io::Error::other(format!("JNI call failed: {e}"))),
    }
}

#[cfg(target_os = "android")]
mod android {
    use jni::objects::JValue;
    use jni::JNIEnv;

    extern "C" {
        // SDL attaches its main thread to the JVM; this returns that
        // thread's JNIEnv.
        fn SDL_AndroidGetJNIEnv() -> *mut std::ffi::c_void;
    }

    /// `org.touchhle.android.MusicFiles.openFd(uri)`: a detached, readable
    /// file descriptor, or -1.
    pub fn open_fd(uri: &str) -> Result<i32, String> {
        let raw = unsafe { SDL_AndroidGetJNIEnv() };
        if raw.is_null() {
            return Err("no JNIEnv for this thread".to_string());
        }
        let mut env =
            unsafe { JNIEnv::from_raw(raw as *mut jni::sys::JNIEnv) }.map_err(|e| e.to_string())?;
        let result = (|| {
            let juri = env.new_string(uri)?;
            let fd = env
                .call_static_method(
                    "org/touchhle/android/MusicFiles",
                    "openFd",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&juri)],
                )?
                .i()?;
            env.delete_local_ref(juri)?;
            Ok::<i32, jni::errors::Error>(fd)
        })();
        match result {
            Ok(fd) => Ok(fd),
            Err(e) => {
                // Leave no Java exception pending, or the next JNI call SDL
                // makes would abort the process.
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                Err(e.to_string())
            }
        }
    }
}
