/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The music folders, and opening a song's file.
//!
//! `library/source.txt` names the music folders, one per line: paths on the
//! desktop, Storage Access Framework tree URIs on Android. An older file
//! with a single line is simply one folder.
//!
//! On the desktop a song in the first folder has a locator relative to it
//! (so songs indexed before there could be several folders keep their
//! locators, and so their persistent IDs); songs in the other folders have
//! their full path. On Android every locator is a `content://` document
//! URI, which only Java can open: we call the Kotlin helper
//! `MusicFiles.openFd` over JNI and wrap the file descriptor it hands back.
//! That opens no activity, so it's safe while touchHLE is running (see the
//! focus rule in CLAUDE.md).

use super::index::Song;
use crate::Environment;
use std::fs::File;
use std::io;
#[cfg(not(target_os = "android"))]
use std::path::{Path, PathBuf};

pub fn open(env: &mut Environment, song: &Song) -> io::Result<File> {
    open_locator(env, &song.locator)
}

/// The folders in a `source.txt`: one per line, blanks and repeats
/// dropped.
pub fn parse_roots(text: &str) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if !line.is_empty() && !roots.iter().any(|r| r == line) {
            roots.push(line.to_string());
        }
    }
    roots
}

/// The music folders, first one first. Empty if none were chosen.
pub fn roots() -> Vec<String> {
    std::fs::read_to_string(super::source_path())
        .map(|text| parse_roots(&text))
        .unwrap_or_default()
}

pub fn write_roots(roots: &[String]) -> io::Result<()> {
    std::fs::create_dir_all(super::library_dir())?;
    let mut text = roots.join("\n");
    text.push('\n');
    std::fs::write(super::source_path(), text)
}

/// Where a desktop locator points: a full path as is, anything else under
/// the first folder (with no way out of it).
#[cfg(not(target_os = "android"))]
pub fn locator_path(roots: &[String], locator: &str) -> Option<PathBuf> {
    if Path::new(locator).is_absolute() {
        return Some(PathBuf::from(locator));
    }
    let mut path = PathBuf::from(roots.first()?);
    for part in locator.split('/').filter(|p| !p.is_empty() && *p != "..") {
        path.push(part);
    }
    Some(path)
}

#[cfg(not(target_os = "android"))]
fn open_locator(_env: &mut Environment, locator: &str) -> io::Result<File> {
    let path = locator_path(&roots(), locator)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no music folder"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_folder_per_line_and_the_old_single_line_file() {
        assert_eq!(parse_roots(r"D:\Music"), [r"D:\Music"]);
        assert_eq!(parse_roots("D:\\Music\n"), [r"D:\Music"]);
        assert_eq!(
            parse_roots("D:\\Music\r\n\r\nE:\\OSTs\nD:\\Music\n"),
            [r"D:\Music", r"E:\OSTs"]
        );
        assert!(parse_roots("").is_empty());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn locators_resolve_under_the_first_folder_or_as_full_paths() {
        let first = std::env::temp_dir().join("music-a");
        let other = std::env::temp_dir().join("music-b").join("x.mp3");
        let roots = vec![first.to_string_lossy().into_owned()];
        assert_eq!(
            locator_path(&roots, "Rock/song.mp3"),
            Some(first.join("Rock").join("song.mp3"))
        );
        // No climbing out of the folder.
        assert_eq!(locator_path(&roots, "../x.mp3"), Some(first.join("x.mp3")));
        let full = other.to_string_lossy().into_owned();
        assert_eq!(locator_path(&roots, &full), Some(other.clone()));
        // A full path works even with no folders; a relative one doesn't.
        assert_eq!(locator_path(&[], &full), Some(other));
        assert_eq!(locator_path(&[], "a.mp3"), None);
    }
}
