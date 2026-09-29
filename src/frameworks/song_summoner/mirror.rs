/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Android: keeping the save folder up to date while the game runs.
//!
//! The Android build runs from the app's own folder and keeps a copy of the
//! saves, settings, play counts, backups and bug reports in a folder the
//! player picked (see `SaveFolder.kt`). `SetupActivity` copies everything
//! before each start; this copies what changed while playing, checked every
//! few seconds and once more as the game quits, through
//! `SaveFolder.openForWrite` / `SaveFolder.delete` over JNI.
//!
//! [allowed] is the same list of files and limits as `SaveFiles` in
//! `SaveFolder.kt`: keep the two in step.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use crate::Environment;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const BUNDLE: &str = super::support::BUNDLE_ID;
/// Names the save folder's tree URI, in the data folder.
pub const POINTER_FILE: &str = "save_folder.txt";

const MAX_SAVE_BYTES: u64 = 16 << 20;
const MAX_TEXT_BYTES: u64 = 4 << 20;
const MAX_ZIP_BYTES: u64 = 128 << 20;
const MAX_DOCUMENTS_DEPTH: usize = 4;
/// How often changes are looked for while playing.
const CHECK_EVERY: Duration = Duration::from_secs(5);

fn documents() -> String {
    format!("touchHLE_sandbox/{BUNDLE}/Documents")
}
fn preferences() -> String {
    format!("touchHLE_sandbox/{BUNDLE}/Library/Preferences")
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c == ':' || c.is_control())
}

fn limit_for(path: &str) -> Option<u64> {
    let parts: Vec<&str> = path.split('/').collect();
    if !parts.iter().all(|p| safe_name(p)) {
        return None;
    }
    let docs = documents();
    let docs: Vec<&str> = docs.split('/').collect();
    let prefs = preferences();
    let prefs: Vec<&str> = prefs.split('/').collect();
    if parts.len() > docs.len()
        && parts.len() <= docs.len() + MAX_DOCUMENTS_DEPTH
        && parts[..docs.len()] == docs[..]
    {
        return Some(MAX_SAVE_BYTES);
    }
    if parts.len() == prefs.len() + 1
        && parts[..prefs.len()] == prefs[..]
        && parts[prefs.len()].ends_with(".plist")
    {
        return Some(MAX_SAVE_BYTES);
    }
    match path {
        "song_summoner_settings.txt" | "library/playcounts.tsv" | "library/source.txt" => {
            return Some(MAX_TEXT_BYTES);
        }
        _ => (),
    }
    match parts[..] {
        ["backups", name] if name.starts_with("saves-") && name.ends_with(".zip") => {
            Some(MAX_ZIP_BYTES)
        }
        ["bug-reports", name] if name.starts_with("bug-report-") && name.ends_with(".zip") => {
            Some(MAX_ZIP_BYTES)
        }
        _ => None,
    }
}

/// Whether a file (relative to the data folder, `/`-separated) belongs in
/// the save folder.
pub fn allowed(path: &str, size: u64) -> bool {
    limit_for(path).is_some_and(|limit| size <= limit)
}

/// A save file: removing it from the data folder removes it from the save
/// folder too (other files there, like backups, are only ever added).
fn is_save(path: &str) -> bool {
    path.starts_with(&format!("{}/", documents())) || path.starts_with(&format!("{}/", preferences()))
}

/// Each allowed file under `base`: (modification time, size).
pub type Snapshot = HashMap<String, (u64, u64)>;

pub fn snapshot(base: &Path) -> Snapshot {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Snapshot) {
        if depth > 10 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(base, &path, depth + 1, out);
                continue;
            }
            let Ok(relative) = path.strip_prefix(base) else {
                continue;
            };
            let relative: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let relative = relative.join("/");
            if !allowed(&relative, meta.len()) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            out.insert(relative, (mtime, meta.len()));
        }
    }
    let mut out = Snapshot::new();
    walk(base, base, 0, &mut out);
    out
}

/// What to copy and what to delete to bring the save folder from `before`
/// to `now`.
pub fn changes(before: &Snapshot, now: &Snapshot) -> (Vec<String>, Vec<String>) {
    let mut writes: Vec<String> = now
        .iter()
        .filter(|(path, stamp)| before.get(*path) != Some(stamp))
        .map(|(path, _)| path.clone())
        .collect();
    let mut deletes: Vec<String> = before
        .keys()
        .filter(|path| !now.contains_key(*path) && is_save(path))
        .cloned()
        .collect();
    writes.sort();
    deletes.sort();
    (writes, deletes)
}

#[derive(Default)]
pub struct State {
    /// What the save folder holds, as far as we know. `None` until the
    /// first check (SetupActivity copied everything just before).
    synced: Option<Snapshot>,
    last_check: Option<Instant>,
}

impl State {
    /// What to copy and delete to bring the folder to `now`. The first
    /// time, that's from `at_start` (see [note_start]); with nothing noted,
    /// `now` only becomes the baseline (`None`).
    fn compare(
        &mut self,
        now: Snapshot,
        at_start: Option<Snapshot>,
    ) -> Option<(Vec<String>, Vec<String>)> {
        let before = self.synced.replace(now.clone()).or(at_start)?;
        Some(changes(&before, &now))
    }
}

/// The saves as the engine found them, before a pending restore changes
/// them (SetupActivity had just copied these out).
static AT_START: std::sync::Mutex<Option<Snapshot>> = std::sync::Mutex::new(None);

/// At startup, before [super::support::restore_pending]: note what the
/// save folder holds, so the first [tick] copies what the restore changed.
pub fn note_start(base: &Path) {
    if cfg!(target_os = "android") && base.join(POINTER_FILE).is_file() {
        *AT_START.lock().unwrap() = Some(snapshot(base));
    }
}

/// Copy what changed. `now_too` checks even if one ran a moment ago (the
/// game is quitting).
pub fn tick(env: &mut Environment, now_too: bool) {
    if !cfg!(target_os = "android") {
        return;
    }
    let base = crate::paths::user_data_base_path().to_path_buf();
    if !base.join(POINTER_FILE).is_file() {
        return;
    }
    let state = &mut env.framework_state.song_summoner.setup_mirror;
    let due = state
        .last_check
        .map_or(true, |at| at.elapsed() >= CHECK_EVERY);
    if !due && !now_too {
        return;
    }
    state.last_check = Some(Instant::now());
    let now = snapshot(&base);
    let at_start = AT_START.lock().unwrap().take();
    let Some((writes, deletes)) = state.compare(now, at_start) else {
        return;
    };
    if writes.is_empty() && deletes.is_empty() {
        return;
    }
    let mut failed = Vec::new();
    for path in &writes {
        if let Err(e) = platform::write(env, &base, path) {
            log!("save folder: couldn't copy {}: {}", path, e);
            failed.push(path.clone());
        }
    }
    for path in &deletes {
        if let Err(e) = platform::delete(env, path) {
            log!("save folder: couldn't remove {}: {}", path, e);
        }
    }
    log!(
        "save folder: copied {} changed files, removed {}",
        writes.len() - failed.len(),
        deletes.len()
    );
    // Try the failed ones again next time.
    if let Some(synced) = env.framework_state.song_summoner.setup_mirror.synced.as_mut() {
        for path in failed {
            synced.remove(&path);
        }
    }
}

#[cfg(not(target_os = "android"))]
mod platform {
    use super::*;
    pub fn write(_env: &mut Environment, _base: &Path, _path: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn delete(_env: &mut Environment, _path: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(target_os = "android")]
mod platform {
    use super::*;
    use jni::objects::JValue;
    use jni::JNIEnv;
    use std::io::Write;
    use std::os::fd::FromRawFd;

    extern "C" {
        fn SDL_AndroidGetJNIEnv() -> *mut std::ffi::c_void;
    }

    /// Call `SaveFolder.<method>(path)` on the thread's real stack.
    fn call(env: &mut Environment, method: &'static str, sig: &'static str, path: &str) -> Result<jni::sys::jvalue, String> {
        let path = path.to_string();
        env.on_parent_stack_in_coroutine(move |_, _| {
            let raw = unsafe { SDL_AndroidGetJNIEnv() };
            if raw.is_null() {
                return Err("no JNIEnv for this thread".to_string());
            }
            let mut jni = unsafe { JNIEnv::from_raw(raw as *mut jni::sys::JNIEnv) }
                .map_err(|e| e.to_string())?;
            let out = (|| {
                let jpath = jni.new_string(&path)?;
                let value = jni.call_static_method(
                    "org/touchhle/android/SaveFolder",
                    method,
                    sig,
                    &[JValue::Object(&jpath)],
                )?;
                jni.delete_local_ref(jpath)?;
                Ok::<_, jni::errors::Error>(value.as_jni())
            })();
            if out.is_err() && jni.exception_check().unwrap_or(false) {
                let _ = jni.exception_describe();
                let _ = jni.exception_clear();
            }
            out.map_err(|e| e.to_string())
        })
    }

    pub fn write(env: &mut Environment, base: &Path, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(base.join(path)).map_err(|e| e.to_string())?;
        let fd = unsafe { call(env, "openForWrite", "(Ljava/lang/String;)I", path)?.i };
        if fd < 0 {
            return Err("the save folder refused it".to_string());
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(&bytes).map_err(|e| e.to_string())
    }

    pub fn delete(env: &mut Environment, path: &str) -> Result<(), String> {
        let ok = unsafe { call(env, "delete", "(Ljava/lang/String;)Z", path)?.z };
        if ok != 0 {
            Ok(())
        } else {
            Err("the save folder refused it".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(rest: &str) -> String {
        format!("touchHLE_sandbox/{BUNDLE}/Documents/{rest}")
    }
    fn prefs(rest: &str) -> String {
        format!("touchHLE_sandbox/{BUNDLE}/Library/Preferences/{rest}")
    }

    // The same cases as SaveFolderTest.kt, so the two lists stay one.
    #[test]
    fn the_games_own_files_are_allowed() {
        assert!(allowed(&docs("save.dat"), 1000));
        assert!(allowed(&docs("slot/1.dat"), 1000));
        assert!(allowed(&prefs(&format!("{BUNDLE}.plist")), 1000));
        assert!(allowed("song_summoner_settings.txt", 100));
        assert!(allowed("library/playcounts.tsv", 100));
        assert!(allowed("library/source.txt", 100));
        assert!(allowed("backups/saves-2026-09-28_1904.zip", 1000));
        assert!(allowed("bug-reports/bug-report-2026-09-28_1904.zip", 1000));
    }

    #[test]
    fn everything_else_is_refused() {
        for path in [
            "Song Summoner The Unsung Heroes Encore.ipa".to_string(),
            "touchHLE_log.txt".to_string(),
            "touchHLE_options.txt".to_string(),
            "library/index.tsv".to_string(),
            "library/art/1.png".to_string(),
            "touchHLE_sandbox/com.other.Game/Documents/save.dat".to_string(),
            prefs("notes.txt"),
            prefs("sub/x.plist"),
            "backups/notes.txt".to_string(),
            "backups/evil.zip".to_string(),
            "touchHLE_apps/Game.ipa".to_string(),
        ] {
            assert!(!allowed(&path, 10), "{path}");
        }
    }

    #[test]
    fn path_tricks_are_refused() {
        for path in [
            docs("../../../evil"),
            docs("./save"),
            docs("/save"),
            docs(".hidden"),
            docs("a\\b"),
            docs("C:save"),
            docs("tab\tname"),
            format!("/{}", docs("save")),
            docs(""),
            String::new(),
            docs("a/b/c/d/e/too-deep"),
        ] {
            assert!(!allowed(&path, 10), "{path:?}");
        }
    }

    #[test]
    fn oversized_files_are_refused() {
        assert!(!allowed(&docs("save.dat"), MAX_SAVE_BYTES + 1));
        assert!(!allowed("song_summoner_settings.txt", MAX_TEXT_BYTES + 1));
        assert!(!allowed("backups/saves-x.zip", MAX_ZIP_BYTES + 1));
    }

    fn snap(entries: &[(&str, u64)]) -> Snapshot {
        entries
            .iter()
            .map(|&(p, t)| (p.to_string(), (t, 1)))
            .collect()
    }

    #[test]
    fn changed_and_new_files_are_copied() {
        let a = docs("a");
        let b = docs("b");
        let before = snap(&[(&a, 1), (&b, 1)]);
        let now = snap(&[(&a, 2), (&b, 1), ("song_summoner_settings.txt", 1)]);
        let (writes, deletes) = changes(&before, &now);
        // Sorted, so the log reads the same every run.
        assert_eq!(writes, vec!["song_summoner_settings.txt".to_string(), a]);
        assert!(deletes.is_empty());
    }

    #[test]
    fn deleted_saves_are_removed_but_backups_stay() {
        let a = docs("a");
        let before = snap(&[(&a, 1), ("backups/saves-1.zip", 1)]);
        let now = snap(&[]);
        let (writes, deletes) = changes(&before, &now);
        assert!(writes.is_empty());
        assert_eq!(deletes, vec![a]);
    }

    // Setup > Restore saves runs as the engine starts, after
    // SetupActivity copied the old saves out: the first check sends the
    // restore (and the backup made before it), not just a baseline.
    #[test]
    fn a_restore_at_startup_reaches_the_save_folder() {
        let (slot1, slot2) = (docs("slot1"), docs("slot2"));
        let at_start = snap(&[(&slot1, 1), (&slot2, 1)]);
        let restored = snap(&[(&slot1, 2), ("backups/saves-2.zip", 1)]);
        let mut state = State::default();
        let (writes, deletes) = state
            .compare(restored.clone(), Some(at_start))
            .unwrap();
        assert_eq!(writes, vec!["backups/saves-2.zip".to_string(), slot1]);
        assert_eq!(deletes, vec![slot2]);
        // Later checks compare with what was sent.
        assert_eq!(state.compare(restored, None), Some((vec![], vec![])));
    }

    #[test]
    fn with_nothing_noted_at_startup_the_first_check_is_a_baseline() {
        let mut state = State::default();
        assert_eq!(state.compare(snap(&[(&docs("a"), 1)]), None), None);
    }

    #[test]
    fn the_snapshot_holds_only_allowed_files() {
        let dir = std::env::temp_dir().join(format!("touchHLE-mirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let save = dir.join(docs("slot1.dat"));
        std::fs::create_dir_all(save.parent().unwrap()).unwrap();
        std::fs::write(&save, "s").unwrap();
        std::fs::write(dir.join("touchHLE_log.txt"), "log").unwrap();
        std::fs::write(dir.join("Song Summoner The Unsung Heroes Encore.ipa"), "PK").unwrap();
        let snapshot = snapshot(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let paths: Vec<&String> = snapshot.keys().collect();
        assert_eq!(paths, vec![&docs("slot1.dat")]);
    }
}
