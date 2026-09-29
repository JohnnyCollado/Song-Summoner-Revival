/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Files the Setup menu writes for the player: a bug-report zip, save
//! backups (and restoring one at the next start), and the license texts.
//!
//! A bug report never contains the game or the player's saves: the game is
//! Square Enix's, and saves are the player's own business.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::FileOptions;

/// The game's bundle id: its sandbox folder name.
pub const BUNDLE_ID: &str = "com.square-enix.SongSummonerEncore";

pub const BACKUPS_DIR: &str = "backups";
pub const REPORTS_DIR: &str = "bug-reports";
/// Names the backup to restore at the next start.
pub const RESTORE_MARKER: &str = "restore-pending.txt";

/// A file name friendly timestamp, like `2026-09-28_1904`, from Unix
/// seconds (UTC).
pub fn timestamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}_{:02}{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// `<dir>/<prefix><time>.zip`, or with `_2`, `_3`… added if that name is
/// taken: two in the same minute must never overwrite each other (a
/// restore backs up the current saves first, and that mustn't replace the
/// backup being restored). The suffix sorts after the plain name, so the
/// newest stays last.
fn fresh_zip_path(dir: &Path, prefix: &str) -> PathBuf {
    let stamp = now_stamp();
    let mut path = dir.join(format!("{prefix}{stamp}.zip"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{prefix}{stamp}_{n}.zip"));
        n += 1;
    }
    path
}

pub fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    timestamp(secs)
}

/// The files that go into a bug report, relative to the data folder.
/// Missing ones are skipped when zipping.
pub fn report_files() -> Vec<&'static str> {
    vec![
        "touchHLE_log.txt",
        "touchHLE_options.txt",
        super::settings::FILE_NAME,
        "library/source.txt",
    ]
}

/// Whether a path may ever go into a bug report.
pub fn allowed_in_report(relative: &str) -> bool {
    let lower = relative.to_lowercase();
    !lower.ends_with(".ipa")
        && !lower.starts_with("touchhle_sandbox")
        && !lower.starts_with(BACKUPS_DIR)
        && !lower.starts_with("touchhle_apps")
}

/// Write `<base>/bug-reports/bug-report-<time>.zip`. `about` is a text of
/// versions and facts added as `about.txt`. Returns the zip's path.
pub fn write_bug_report(base: &Path, about: &str) -> Result<PathBuf, String> {
    let dir = base.join(REPORTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = fresh_zip_path(&dir, "bug-report-");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("about.txt", options).map_err(|e| e.to_string())?;
    zip.write_all(about.as_bytes()).map_err(|e| e.to_string())?;
    for relative in report_files() {
        assert!(allowed_in_report(relative));
        let Ok(bytes) = std::fs::read(base.join(relative)) else {
            continue;
        };
        zip.start_file(relative, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(path)
}

/// The game's save folders under the sandbox, relative to it.
const SAVE_DIRS: [&str; 2] = ["Documents", "Library/Preferences"];

fn sandbox(base: &Path) -> PathBuf {
    base.join(crate::paths::SANDBOX_DIR).join(BUNDLE_ID)
}

/// Every file under `dir`, relative to `root`, `/`-separated.
fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
}

/// Zip the saves to `<base>/backups/saves-<time>.zip`.
pub fn backup_saves(base: &Path) -> Result<PathBuf, String> {
    let root = sandbox(base);
    let mut files = Vec::new();
    for dir in SAVE_DIRS {
        files_under(&root, &root.join(dir), &mut files);
    }
    if files.is_empty() {
        return Err("There are no saves yet".to_string());
    }
    files.sort();
    let dir = base.join(BACKUPS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = fresh_zip_path(&dir, "saves-");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for relative in files {
        let bytes = std::fs::read(root.join(&relative)).map_err(|e| e.to_string())?;
        zip.start_file(relative.as_str(), options)
            .map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(path)
}

/// The newest backup's file name, if any.
pub fn latest_backup(base: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(base.join(BACKUPS_DIR))
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("saves-") && n.ends_with(".zip"))
        .collect();
    // The timestamps sort as text.
    names.sort();
    names.pop()
}

/// Ask for `name` to be restored at the next start (the running game
/// could otherwise write its saves over it as it quits).
pub fn request_restore(base: &Path, name: &str) -> Result<(), String> {
    std::fs::write(base.join(BACKUPS_DIR).join(RESTORE_MARKER), name).map_err(|e| e.to_string())
}

/// Only paths inside the save folders, with no way out of them.
fn safe_save_path(relative: &str) -> bool {
    SAVE_DIRS.iter().any(|d| relative.starts_with(&format!("{d}/")))
        && !relative.split('/').any(|part| part == ".." || part.is_empty())
        && !relative.contains('\\')
        && !relative.contains(':')
}

/// At startup: restore a backup if one was asked for. The current saves are
/// backed up first, so a restore can itself be undone.
pub fn restore_pending(base: &Path) {
    let marker = base.join(BACKUPS_DIR).join(RESTORE_MARKER);
    let Ok(name) = std::fs::read_to_string(&marker) else {
        return;
    };
    let _ = std::fs::remove_file(&marker);
    let name = name.trim();
    if name.contains(['/', '\\']) || name.contains("..") {
        log!("setup: refusing to restore {:?}", name);
        return;
    }
    if let Err(e) = backup_saves(base) {
        log!("setup: no backup of the current saves before restoring: {}", e);
    }
    match restore(base, &base.join(BACKUPS_DIR).join(name)) {
        Ok(n) => {
            log!("setup: restored {} save files from {}", n, name);
        }
        Err(e) => {
            log!("setup: couldn't restore {}: {}", name, e);
        }
    }
}

/// The most a restore reads: a backup can come in from the shared save
/// folder on Android, so it's never trusted to be one of ours. Per file,
/// the same as the save folder's (`mirror.rs`).
const MAX_RESTORE_FILE_BYTES: u64 = 16 << 20;
const MAX_RESTORE_TOTAL_BYTES: u64 = 64 << 20;
const MAX_RESTORE_FILES: usize = 1000;

fn restore(base: &Path, zip_path: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() > MAX_RESTORE_FILES {
        return Err(format!("{} files is too many for a backup", archive.len()));
    }
    // Read it all first (within the limits, whatever sizes the zip
    // claims), so a bad backup leaves the current saves alone.
    let mut files = Vec::new();
    let mut total = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if entry.is_dir() || !safe_save_path(&name) {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .take(MAX_RESTORE_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        total += bytes.len() as u64;
        if bytes.len() as u64 > MAX_RESTORE_FILE_BYTES || total > MAX_RESTORE_TOTAL_BYTES {
            return Err(format!("{name} is too big for a save"));
        }
        files.push((name, bytes));
    }
    let root = sandbox(base);
    // Clear the save folders, so files the backup didn't have go too.
    for dir in SAVE_DIRS {
        let _ = std::fs::remove_dir_all(root.join(dir));
        let _ = std::fs::create_dir_all(root.join(dir));
    }
    let mut count = 0;
    for (name, bytes) in files {
        let path = root.join(&name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

/// Write the license texts to `<base>/licenses.txt`.
pub fn write_licenses(base: &Path) -> Result<PathBuf, String> {
    let path = base.join("licenses.txt");
    let text = format!(
        "{}\n\n{}\n",
        crate::licenses::get_text(),
        include_str!("../../../CREDITS.txt")
    );
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "touchHLE-support-test-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
        fn file(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "1970-01-01_0000");
        // 2026-09-28 19:04 UTC.
        assert_eq!(timestamp(1_790_622_240), "2026-09-28_1904");
        // A leap day.
        assert_eq!(timestamp(951_782_400), "2000-02-29_0000");
    }

    #[test]
    fn reports_never_carry_the_game_or_saves() {
        for relative in report_files() {
            assert!(allowed_in_report(relative), "{relative}");
        }
        assert!(!allowed_in_report("Song Summoner The Unsung Heroes Encore.ipa"));
        assert!(!allowed_in_report("touchHLE_sandbox/com.square-enix.SongSummonerEncore/Documents/save.dat"));
        assert!(!allowed_in_report("backups/saves-2026.zip"));
        assert!(!allowed_in_report("touchHLE_apps/Game.IPA"));
    }

    #[test]
    fn bug_report_zips_the_log_and_settings_only() {
        let dir = TempDir::new("report");
        dir.file("touchHLE_log.txt", "log");
        dir.file("song_summoner_settings.txt", "first_run_done=true");
        dir.file("Song Summoner The Unsung Heroes Encore.ipa", "PK");
        dir.file(&format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/save"), "s");
        let path = write_bug_report(&dir.0, "version 0.2.3").unwrap();
        let mut names = zip_names(&path);
        names.sort();
        assert_eq!(names, ["about.txt", "song_summoner_settings.txt", "touchHLE_log.txt"]);
    }

    #[test]
    fn backup_then_restore_brings_saves_back() {
        let dir = TempDir::new("backup");
        let save = format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/slot1.dat");
        let prefs = format!("touchHLE_sandbox/{BUNDLE_ID}/Library/Preferences/p.plist");
        dir.file(&save, "old");
        dir.file(&prefs, "prefs");
        let backup = backup_saves(&dir.0).unwrap();
        let mut names = zip_names(&backup);
        names.sort();
        assert_eq!(names, ["Documents/slot1.dat", "Library/Preferences/p.plist"]);

        // The game overwrites the save and adds another.
        dir.file(&save, "new");
        dir.file(&format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/slot2.dat"), "x");
        let name = backup.file_name().unwrap().to_string_lossy().into_owned();
        request_restore(&dir.0, &name).unwrap();
        restore_pending(&dir.0);
        assert_eq!(std::fs::read_to_string(dir.0.join(&save)).unwrap(), "old");
        assert!(!dir.0.join(format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/slot2.dat")).exists());
        // The marker is gone, so it happens once.
        assert!(!dir.0.join(BACKUPS_DIR).join(RESTORE_MARKER).exists());
    }

    #[test]
    fn two_backups_in_a_minute_are_two_files_and_the_newest_is_last() {
        let dir = TempDir::new("twice");
        dir.file(&format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/slot1.dat"), "a");
        let first = backup_saves(&dir.0).unwrap();
        let second = backup_saves(&dir.0).unwrap();
        assert_ne!(first, second);
        assert!(first.exists());
        let newest = second.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(latest_backup(&dir.0), Some(newest));
    }

    #[test]
    fn no_saves_no_backup() {
        let dir = TempDir::new("empty");
        assert!(backup_saves(&dir.0).is_err());
        assert_eq!(latest_backup(&dir.0), None);
    }

    #[test]
    fn restore_ignores_paths_outside_the_save_folders() {
        assert!(safe_save_path("Documents/slot1.dat"));
        assert!(safe_save_path("Library/Preferences/p.plist"));
        assert!(!safe_save_path("../../evil.txt"));
        assert!(!safe_save_path("Documents/../../evil.txt"));
        assert!(!safe_save_path("Library/Caches/x"));
        assert!(!safe_save_path("C:/Windows/x"));
    }

    // A backup can come in from the shared save folder, so its size is
    // checked before anything is deleted: a too-big one changes nothing.
    #[test]
    fn an_oversized_backup_is_refused_and_the_saves_stay() {
        let dir = TempDir::new("oversized");
        let save = format!("touchHLE_sandbox/{BUNDLE_ID}/Documents/slot1.dat");
        dir.file(&save, "mine");
        let zip_path = dir.0.join(BACKUPS_DIR).join("saves-9999.zip");
        std::fs::create_dir_all(zip_path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("Documents/slot1.dat", options).unwrap();
        std::io::Write::write_all(&mut zip, &vec![0u8; MAX_RESTORE_FILE_BYTES as usize + 1])
            .unwrap();
        zip.finish().unwrap();

        assert!(restore(&dir.0, &zip_path).is_err());
        assert_eq!(std::fs::read_to_string(dir.0.join(&save)).unwrap(), "mine");
    }

    #[test]
    fn latest_backup_is_the_newest() {
        let dir = TempDir::new("latest");
        dir.file("backups/saves-2026-01-02_0900.zip", "");
        dir.file("backups/saves-2026-09-28_1904.zip", "");
        dir.file("backups/notes.txt", "");
        assert_eq!(latest_backup(&dir.0).as_deref(), Some("saves-2026-09-28_1904.zip"));
    }
}
