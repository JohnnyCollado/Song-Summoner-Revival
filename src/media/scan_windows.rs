/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The desktop music scanner (Windows, and any other non-Android host).
//!
//! At boot, before the SDL window exists, [prepare] asks for the music folder
//! if none was chosen yet (or `--choose-music-folder` was passed), then reads
//! the folder on a host thread while the app starts. Until the scan is done
//! the engine serves the previous index, so the game can already count its
//! songs; the new library is swapped in when the scan finishes.
//!
//! A rescan only reads tags for files whose size or modification time
//! changed, so after the first run it takes a second or two. The files that
//! do need reading are spread over a few threads, since opening each one
//! (not parsing it) is most of the cost.

use super::artwork::{self, Bitmap, MAX_ART_SIDE};
use super::index::{self, Library, Song};
use super::library;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, UNIX_EPOCH};

/// Most threads used to read tags. Reading is disk-bound, so beyond a
/// handful the threads mostly queue on the same disk.
const MAX_READ_THREADS: usize = 8;

pub fn prepare(force_dialog: bool) {
    let saved = saved_folder();
    let root = match saved {
        Some(ref folder) if !force_dialog && folder.is_dir() => Some(folder.clone()),
        _ => ask_for_folder(saved.as_deref()).or(saved),
    };
    let Some(root) = root else {
        log!("media: no music folder chosen, the music library stays empty");
        return;
    };
    log!("media: music folder is {}", root.display());

    // Load the previous index now, so the game gets it while we rescan.
    let previous = library::current();
    library::set_scanning(true);
    let spawned = std::thread::Builder::new()
        .name("touchHLE music scan".to_string())
        .spawn(move || {
            let library = scan(&root, &previous);
            library::publish(library);
            library::set_scanning(false);
        });
    if let Err(e) = spawned {
        log!("media: couldn't start the music scan: {}", e);
        library::set_scanning(false);
    }
}

fn saved_folder() -> Option<PathBuf> {
    let text = std::fs::read_to_string(super::source_path()).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// Show the native "Select folder" dialog and remember the answer in
/// `library/source.txt`. `None` if the user cancelled.
fn ask_for_folder(previous: Option<&Path>) -> Option<PathBuf> {
    log!("media: asking for the music folder");
    let mut dialog = rfd::FileDialog::new().set_title("Choose the folder with your music");
    if let Some(previous) = previous.filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(previous);
    }
    let folder = dialog.pick_folder()?;
    let saved = std::fs::create_dir_all(super::library_dir())
        .and_then(|_| std::fs::write(super::source_path(), folder.to_string_lossy().as_bytes()));
    if let Err(e) = saved {
        log!("media: couldn't save {}: {}", super::source_path().display(), e);
    }
    Some(folder)
}

struct Found {
    path: PathBuf,
    /// Relative to the music folder, `/`-separated.
    relative: String,
    mtime: u64,
    size: u64,
}

fn walk(dir: &Path, relative: &str, depth: u32, out: &mut Vec<Found>) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Follows symlinks, like a file manager would.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let child = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if meta.is_dir() {
            walk(&path, &child, depth + 1, out);
            continue;
        }
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !super::AUDIO_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(Found {
            path,
            relative: child,
            mtime,
            size: meta.len(),
        });
    }
}

/// Time spent in each part of reading a file, summed over all read
/// threads (so with several threads it can exceed the scan's own time).
#[derive(Default)]
struct ReadTimes {
    tags_ns: AtomicU64,
    art_ns: AtomicU64,
}

impl ReadTimes {
    fn add(counter: &AtomicU64, since: Instant) {
        counter.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    fn ms(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed) / 1_000_000
    }
}

fn scan(root: &Path, previous: &Library) -> Library {
    let start = Instant::now();
    let found = list_audio_files(root);
    let walk_time = start.elapsed();
    log!("media: found {} audio files", found.len());

    let _ = std::fs::create_dir_all(super::art_dir());
    let read_start = Instant::now();
    let times = ReadTimes::default();
    let built = build_songs(
        &found,
        previous,
        |id| artwork::art_path(id).is_file(),
        |file, id| read_song(file, id, &times),
        &library::set_scan_progress,
    );
    let read_time = read_start.elapsed();
    let songs = built.songs;

    let write_start = Instant::now();
    save_index(&songs);
    remove_stale_art(&songs);
    artwork::clear_cache();
    let write_time = write_start.elapsed();
    log!(
        "media: scanned {} songs ({} read, {} unchanged) in {} ms: \
         walk {} ms, read {} ms on {} threads (summed: tags {} ms, \
         art {} ms), write {} ms",
        songs.len(),
        built.read,
        songs.len().saturating_sub(built.read),
        start.elapsed().as_millis(),
        walk_time.as_millis(),
        read_time.as_millis(),
        built.threads,
        ReadTimes::ms(&times.tags_ns),
        ReadTimes::ms(&times.art_ns),
        write_time.as_millis(),
    );
    Library::new(songs)
}

/// The playable audio files under `root`, sorted by relative path.
fn list_audio_files(root: &Path) -> Vec<Found> {
    let mut found = Vec::new();
    walk(root, "", 0, &mut found);
    found.sort_by(|a, b| a.relative.cmp(&b.relative));
    found
}

/// What [build_songs] decided.
struct Built {
    /// One per song, in the order of the files given.
    songs: Vec<Song>,
    /// How many files were new or changed and had to be read.
    read: usize,
    /// Read threads used, 0 if nothing needed reading.
    threads: usize,
}

/// Turn the files on disk into the new song list. A file whose size,
/// modification time and path match the previous index is taken from it;
/// the rest are read with `read` on up to [MAX_READ_THREADS] threads.
/// This does no disk access of its own: `has_art_file` checks a cover is
/// still stored, and `progress` gets the running song count. That keeps
/// it testable with stand-ins.
fn build_songs<R>(
    found: &[Found],
    previous: &Library,
    has_art_file: impl Fn(u64) -> bool,
    read: R,
    progress: &(dyn Fn(usize) + Sync),
) -> Built
where
    R: Fn(&Found, u64) -> Song + Sync,
{
    let old: HashMap<u64, &Song> = previous.songs.iter().map(|s| (s.id, s)).collect();
    // One slot per song, in index order. Unchanged songs are filled in
    // now; the rest are read below and dropped into their slot.
    let mut slots: Vec<Option<Song>> = Vec::with_capacity(found.len());
    let mut to_read = Vec::new();
    let mut seen = HashSet::new();
    for file in found {
        let id = index::persistent_id(&file.relative.to_lowercase());
        if !seen.insert(id) {
            continue;
        }
        let unchanged = old.get(&id).filter(|s| {
            s.mtime == file.mtime && s.size == file.size && s.locator == file.relative
        });
        match unchanged {
            Some(&song) => {
                let mut song = song.clone();
                song.has_art = song.has_art && has_art_file(id);
                slots.push(Some(song));
            }
            None => {
                to_read.push((slots.len(), file, id));
                slots.push(None);
            }
        }
    }
    let reused = slots.len() - to_read.len();
    progress(reused);

    let threads = if to_read.is_empty() {
        0
    } else {
        std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, MAX_READ_THREADS)
            .min(to_read.len())
    };
    // Threads take the next file off a shared counter rather than a fixed
    // chunk each, so one folder of huge files doesn't leave the rest idle.
    let next = &AtomicUsize::new(0);
    let done = &AtomicUsize::new(reused);
    let jobs = &to_read;
    let read = &read;
    let results: Vec<(usize, Song)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut out = Vec::new();
                    while let Some(&(slot, file, id)) =
                        jobs.get(next.fetch_add(1, Ordering::Relaxed))
                    {
                        // A malformed file that panics the tag reader
                        // costs only its own song.
                        let song = std::panic::catch_unwind(AssertUnwindSafe(|| read(file, id)));
                        match song {
                            Ok(song) => out.push((slot, song)),
                            Err(_) => {
                                log!("media: skipped {}, reading it crashed", file.relative);
                            }
                        }
                        progress(done.fetch_add(1, Ordering::Relaxed) + 1);
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    for (slot, song) in results {
        slots[slot] = Some(song);
    }
    Built {
        // A slot stays empty only if reading its file crashed.
        songs: slots.into_iter().flatten().collect(),
        read: to_read.len(),
        threads,
    }
}

fn read_song(file: &Found, id: u64, times: &ReadTimes) -> Song {
    let (folder, file_name) = match file.relative.rsplit_once('/') {
        Some((folder, name)) => (folder.to_string(), name),
        None => (String::new(), file.relative.as_str()),
    };
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem)
        .to_string();
    let parent_name = folder.rsplit('/').next().unwrap_or("").to_string();

    let mut song = Song {
        id,
        locator: file.relative.clone(),
        mtime: file.mtime,
        size: file.size,
        title: stem,
        album: parent_name,
        folder,
        ..Default::default()
    };
    let tags_start = Instant::now();
    let tags = read_tags(&file.path, &mut song);
    ReadTimes::add(&times.tags_ns, tags_start);
    match tags {
        Ok(Some(picture)) => {
            let art_start = Instant::now();
            song.has_art = save_art(id, &picture);
            ReadTimes::add(&times.art_ns, art_start);
        }
        Ok(None) => (),
        Err(e) => {
            log!("media: no tags for {}: {}", file.relative, e);
        }
    }
    song
}

/// Fill `song` from the file's tags. Returns the embedded cover picture.
fn read_tags(path: &Path, song: &mut Song) -> Result<Option<Vec<u8>>, String> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::picture::PictureType;
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey};

    let tagged = Probe::open(path)
        .and_then(|probe| probe.read())
        .map_err(|e| e.to_string())?;
    song.duration_ms = tagged.properties().duration().as_millis() as u64;
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };
    fn text(value: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
        value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }
    if let Some(title) = text(tag.title()) {
        song.title = title;
    }
    if let Some(artist) = text(tag.artist()) {
        song.artist = artist;
    }
    if let Some(album) = text(tag.album()) {
        song.album = album;
    }
    if let Some(genre) = text(tag.genre()) {
        song.genre = genre;
    }
    let album_artist = tag.get_string(&ItemKey::AlbumArtist);
    if let Some(album_artist) = text(album_artist.map(std::borrow::Cow::Borrowed)) {
        song.album_artist = album_artist;
    }
    song.track = tag.track().unwrap_or(0);

    let pictures = tag.pictures();
    let cover = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first());
    Ok(cover.map(|p| p.data().to_vec()))
}

/// Shrink and store a cover picture as `art/<id>.png`.
fn save_art(id: u64, picture: &[u8]) -> bool {
    let Some(bitmap) = Bitmap::decode(picture) else {
        return false;
    };
    let png = artwork::encode_png(&bitmap.scaled_to_fit(MAX_ART_SIDE));
    std::fs::write(artwork::art_path(id), png).is_ok()
}

fn save_index(songs: &[Song]) {
    let path = super::index_path();
    let tmp = path.with_extension("tsv.tmp");
    let result = std::fs::create_dir_all(super::library_dir())
        .and_then(|_| std::fs::write(&tmp, index::serialize(songs)))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = result {
        log!("media: couldn't write {}: {}", path.display(), e);
    }
}

fn remove_stale_art(songs: &[Song]) {
    let keep: HashSet<String> = songs
        .iter()
        .filter(|s| s.has_art)
        .map(|s| format!("{}.png", s.id))
        .collect();
    let Ok(entries) = std::fs::read_dir(super::art_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !keep.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    fn found(relative: &str, mtime: u64, size: u64) -> Found {
        Found {
            path: PathBuf::from(relative),
            relative: relative.to_string(),
            mtime,
            size,
        }
    }

    fn id_of(relative: &str) -> u64 {
        index::persistent_id(&relative.to_lowercase())
    }

    /// A song as it would be in the previous index.
    fn indexed(relative: &str, mtime: u64, size: u64, title: &str) -> Song {
        Song {
            id: id_of(relative),
            locator: relative.to_string(),
            mtime,
            size,
            title: title.to_string(),
            ..Default::default()
        }
    }

    /// A stand-in for reading tags: titles the song "read <path>".
    fn fake_read(file: &Found, id: u64) -> Song {
        Song {
            id,
            locator: file.relative.clone(),
            mtime: file.mtime,
            size: file.size,
            title: format!("read {}", file.relative),
            ..Default::default()
        }
    }

    fn no_progress(_: usize) {}

    fn locators(songs: &[Song]) -> Vec<&str> {
        songs.iter().map(|s| s.locator.as_str()).collect()
    }

    #[test]
    fn keeps_index_order_when_reads_finish_out_of_order() {
        let files: Vec<Found> = (0..20)
            .map(|n| found(&format!("song{n:02}.mp3"), 1, 1))
            .collect();
        // Earlier files take longer, so they finish last.
        let read = |file: &Found, id| {
            let n: u64 = file.relative[4..6].parse().unwrap();
            std::thread::sleep(Duration::from_millis(20 - n));
            fake_read(file, id)
        };
        let built = build_songs(&files, &Library::default(), |_| true, read, &no_progress);
        let expected: Vec<&str> = files.iter().map(|f| f.relative.as_str()).collect();
        assert_eq!(locators(&built.songs), expected);
        assert_eq!(built.read, 20);
    }

    #[test]
    fn only_new_and_changed_files_are_read() {
        let previous = Library::new(vec![
            indexed("same.mp3", 10, 100, "kept"),
            indexed("edited.mp3", 10, 100, "old"),
            indexed("resized.mp3", 10, 100, "old"),
        ]);
        let files = vec![
            found("edited.mp3", 11, 100),
            found("new.mp3", 10, 100),
            found("resized.mp3", 10, 101),
            found("same.mp3", 10, 100),
        ];
        let calls = Mutex::new(Vec::new());
        let read = |file: &Found, id| {
            calls.lock().unwrap().push(file.relative.clone());
            fake_read(file, id)
        };
        let built = build_songs(&files, &previous, |_| true, read, &no_progress);

        let mut calls = calls.into_inner().unwrap();
        calls.sort();
        assert_eq!(calls, ["edited.mp3", "new.mp3", "resized.mp3"]);
        assert_eq!(built.read, 3);
        let titles: Vec<&str> = built.songs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            ["read edited.mp3", "read new.mp3", "read resized.mp3", "kept"]
        );
    }

    #[test]
    fn reused_songs_keep_art_only_if_the_file_is_still_there() {
        let mut with_file = indexed("a.mp3", 1, 1, "a");
        with_file.has_art = true;
        let mut file_gone = indexed("b.mp3", 1, 1, "b");
        file_gone.has_art = true;
        let previous = Library::new(vec![with_file, file_gone]);
        let files = vec![found("a.mp3", 1, 1), found("b.mp3", 1, 1)];
        let gone = id_of("b.mp3");
        let built = build_songs(&files, &previous, |id| id != gone, fake_read, &no_progress);
        let art: Vec<bool> = built.songs.iter().map(|s| s.has_art).collect();
        assert_eq!(art, [true, false]);
        assert_eq!(built.read, 0);
    }

    #[test]
    fn paths_differing_only_in_case_are_one_song() {
        // Windows paths are case-insensitive, and the ID is built from the
        // lowercased path, so the first spelling wins.
        let files = vec![found("Song.mp3", 1, 1), found("song.mp3", 1, 1)];
        let built = build_songs(&files, &Library::default(), |_| true, fake_read, &no_progress);
        assert_eq!(locators(&built.songs), ["Song.mp3"]);
    }

    #[test]
    fn deleted_files_leave_the_library() {
        let previous = Library::new(vec![
            indexed("gone.mp3", 1, 1, "gone"),
            indexed("here.mp3", 1, 1, "here"),
        ]);
        let files = vec![found("here.mp3", 1, 1)];
        let built = build_songs(&files, &previous, |_| true, fake_read, &no_progress);
        assert_eq!(locators(&built.songs), ["here.mp3"]);
    }

    #[test]
    fn progress_counts_up_to_every_song() {
        let previous = Library::new(vec![indexed("old.mp3", 1, 1, "old")]);
        let files: Vec<Found> = std::iter::once(found("old.mp3", 1, 1))
            .chain((0..9).map(|n| found(&format!("new{n}.mp3"), 1, 1)))
            .collect();
        let seen = Mutex::new(Vec::new());
        let progress = |n: usize| seen.lock().unwrap().push(n);
        build_songs(&files, &previous, |_| true, fake_read, &progress);
        let seen = seen.into_inner().unwrap();
        // Reused songs count straight away, then each read adds one.
        assert_eq!(seen.first(), Some(&1));
        assert_eq!(seen.iter().max(), Some(&10));
    }

    #[test]
    fn a_file_that_crashes_the_reader_costs_only_that_song() {
        let files: Vec<Found> = ["a.mp3", "bad.mp3", "c.mp3", "d.mp3", "e.mp3"]
            .iter()
            .map(|name| found(name, 1, 1))
            .collect();
        let read = |file: &Found, id| {
            if file.relative == "bad.mp3" {
                panic!("malformed tag");
            }
            fake_read(file, id)
        };
        let built = build_songs(&files, &Library::default(), |_| true, read, &no_progress);
        assert_eq!(locators(&built.songs), ["a.mp3", "c.mp3", "d.mp3", "e.mp3"]);
    }

    #[test]
    fn nothing_to_read_uses_no_threads() {
        let previous = Library::new(vec![indexed("a.mp3", 1, 1, "a")]);
        let files = vec![found("a.mp3", 1, 1)];
        let built = build_songs(&files, &previous, |_| true, fake_read, &no_progress);
        assert_eq!(built.threads, 0);
    }

    /// A folder under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir()
                .join(format!("touchHLE-scan-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
        fn file(&self, relative: &str, bytes: usize) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0u8; bytes]).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn listing_finds_playable_audio_sorted_by_path() {
        let dir = TempDir::new("listing");
        dir.file("b.m4a", 3);
        dir.file("Artist/Album/01 Song.MP3", 5);
        dir.file("Artist/cover.jpg", 1);
        dir.file("a.wav", 1);
        // Formats the engine can't play, and hidden files and folders.
        dir.file("lossless.flac", 1);
        dir.file("vorbis.ogg", 1);
        dir.file(".hidden.mp3", 1);
        dir.file(".trash/old.mp3", 1);

        let files = list_audio_files(&dir.0);
        let relative: Vec<&str> = files.iter().map(|f| f.relative.as_str()).collect();
        assert_eq!(relative, ["Artist/Album/01 Song.MP3", "a.wav", "b.m4a"]);
        assert_eq!(files[0].size, 5);
        assert_eq!(files[0].path, dir.0.join("Artist/Album/01 Song.MP3"));
        assert!(files.iter().all(|f| f.mtime > 0));
    }
}
