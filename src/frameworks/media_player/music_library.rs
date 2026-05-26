/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! A simulated iPod music library, populated either from the user's chosen
//! music folder on the host machine or from a small set of built-in fallback
//! songs. The chosen folder path is persisted to
//! `touchHLE_music_library.txt` in the working directory so subsequent
//! launches reuse it.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// Name of the file in the touchHLE user-data dir holding the user's chosen
/// music-folder path. Use [library_path_file] / [library_cache_file] to get
/// the absolute path on disk — resolving against the user-data dir is critical
/// on Android, where the process CWD is the (read-only) app-internal dir and
/// the writable touchHLE files live at `/sdcard/touchHLE/`.
const LIBRARY_PATH_FILE_NAME: &str = "touchHLE_music_library.txt";
/// On-disk cache of the scanned library so subsequent launches skip the
/// filesystem walk. Format is line-oriented for simplicity (header + one song
/// per line, tab-separated). Invalidated whenever the music folder's mtime is
/// newer than the cache file.
const LIBRARY_CACHE_FILE_NAME: &str = "touchHLE_music_library_cache.tsv";

fn library_path_file() -> PathBuf {
    crate::paths::user_data_base_path().join(LIBRARY_PATH_FILE_NAME)
}
fn library_cache_file() -> PathBuf {
    crate::paths::user_data_base_path().join(LIBRARY_CACHE_FILE_NAME)
}
/// Bump when the on-disk format changes.
const LIBRARY_CACHE_VERSION: u32 = 3;

/// Optional cap on how many songs the simulated library exposes. Defaults to
/// "no cap" — the calc-play-points spin-loop hang that originally motivated
/// this gate was fixed by force-unwinding `[NSThread exit]` to the host
/// return-routine, so the full library is safe to expose. Set
/// `TOUCHHLE_MUSIC_LIBRARY_LIMIT=N` to clamp for testing.
const DEFAULT_MAX_SONGS: usize = usize::MAX;

/// File extensions we treat as music. Deliberately music-only — `.mp4` is
/// excluded because in practice almost every .mp4 in a phone's storage is
/// video, and lofty cheerfully returns empty tags for those, polluting the
/// library with phantom songs. Apple's audio-in-mp4 case is covered by
/// `.m4a`.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "caf", "aif", "aiff",
];

#[derive(Clone)]
pub struct Song {
    pub persistent_id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub play_count: u32,
    pub duration_secs: f64,
    /// Absolute path to the audio file on the host filesystem. Used so the
    /// picker can play the song through rodio when the user taps a row,
    /// matching the real-iPod behaviour.
    pub path: PathBuf,
}

fn fallback_songs() -> Vec<Song> {
    [
        ("Trooper Anthem", 10, 180.0),
        ("Hero's March", 25, 200.0),
        ("Final Stand", 5, 150.0),
        ("Victory Theme", 15, 170.0),
        ("Battle Cry", 42, 220.0),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(title, play_count, duration))| Song {
        persistent_id: (i as u64) + 1,
        title: title.to_string(),
        artist: "touchHLE".to_string(),
        album: "Encore".to_string(),
        genre: "Battle".to_string(),
        play_count,
        duration_secs: duration,
        path: PathBuf::new(),
    })
    .collect()
}

fn saved_library_path() -> Option<PathBuf> {
    let path_file = library_path_file();
    // Respect `TOUCHHLE_RECHOOSE_MUSIC=1` by clearing any saved path so the
    // folder picker re-prompts on this launch.
    if std::env::var("TOUCHHLE_RECHOOSE_MUSIC").ok().as_deref() == Some("1") {
        let _ = fs::remove_file(&path_file);
        log!("touchHLE: TOUCHHLE_RECHOOSE_MUSIC=1 set; re-prompting for the music folder.");
        return None;
    }
    let raw = fs::read_to_string(&path_file).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn save_library_path(path: &Path) {
    let path_file = library_path_file();
    if let Err(e) = fs::write(&path_file, path.to_string_lossy().as_bytes()) {
        log!("Couldn't write {}: {e}", path_file.display());
    }
}

/// Open a native folder-picker dialog. Returns the chosen path, or `None` if
/// the user cancelled / the platform doesn't support it.
#[cfg(not(target_os = "android"))]
fn prompt_for_folder() -> Option<PathBuf> {
    log!("touchHLE: opening a folder picker so you can choose a music library.");
    log!("touchHLE: pick a folder containing your audio files (mp3, m4a, wav, ...).");
    log!(
        "touchHLE: the chosen path will be remembered in `{}`.",
        library_path_file().display()
    );
    rfd::FileDialog::new()
        .set_title("Choose your music library folder for touchHLE")
        .pick_folder()
}

/// Android has no native folder-picker we can invoke synchronously from the
/// Rust side (SAF is async via Intent.ACTION_OPEN_DOCUMENT_TREE and requires
/// JNI plumbing through MainActivity). Instead, auto-detect a sensible
/// candidate by counting audio files in a few well-known locations and
/// picking the winner. The choice is persisted via the normal
/// `touchHLE_music_library.txt` flow, so subsequent launches skip the scan.
///
/// Users can still override by editing that file by hand -- e.g. point it at
/// `/sdcard/Music/MyArtist` for a sub-folder, or anywhere else on /sdcard
/// once MANAGE_EXTERNAL_STORAGE is granted.
#[cfg(target_os = "android")]
fn prompt_for_folder() -> Option<PathBuf> {
    // Ordered by user intent: Music/ is the standard Android media folder,
    // touchHLE/Music sits next to our other user data, Download is where
    // people commonly drop sideloaded MP3s. The first one with audio files
    // wins; on tie we prefer the one earlier in the list.
    const CANDIDATES: &[&str] = &[
        "/sdcard/Music",
        "/sdcard/touchHLE/Music",
        "/sdcard/Download",
        "Music", // last-resort: relative to cwd (private app-files dir)
    ];

    log!("touchHLE: scanning candidate music folders on Android...");

    let mut best: Option<(PathBuf, usize)> = None;
    for raw in CANDIDATES {
        let candidate = PathBuf::from(raw);
        if !candidate.is_dir() {
            log!("touchHLE:   {raw} -- not a directory, skipping.");
            continue;
        }
        let count = count_audio_files_recursive(&candidate);
        log!("touchHLE:   {raw} -- {count} audio file(s) (recursive).");
        match &best {
            // Strictly greater so earlier candidates win ties.
            Some((_, best_count)) if count <= *best_count => {}
            _ => best = Some((candidate, count)),
        }
    }

    match best {
        Some((path, count)) if count > 0 => {
            log!(
                "touchHLE: using {} as the music library ({count} audio file(s) found recursively).",
                path.display()
            );
            Some(path)
        }
        _ => {
            // Nothing useful found. Seed /sdcard/touchHLE/Music (or, if
            // MANAGE_EXTERNAL_STORAGE isn't granted yet, fall back to the
            // private Music/ next to our cwd) so the user has somewhere
            // obvious to put files. Either way, the music library will be
            // empty until they copy something in.
            let seed = if PathBuf::from("/sdcard/touchHLE").is_dir()
                || std::fs::create_dir_all("/sdcard/touchHLE").is_ok()
            {
                PathBuf::from("/sdcard/touchHLE/Music")
            } else {
                PathBuf::from("Music")
            };
            if let Err(e) = std::fs::create_dir_all(&seed) {
                log!(
                    "touchHLE: couldn't create music folder {}: {e}. \
                     The library will use built-in fallback songs.",
                    seed.display()
                );
                return None;
            }
            log!(
                "touchHLE: no audio files found in any candidate folder; \
                 created {} as a starting point. Drop .mp3/.m4a/.wav/etc. files \
                 there, then relaunch. To use a different folder, edit \
                 {} with the absolute path.",
                seed.display(),
                library_path_file().display(),
            );
            Some(seed)
        }
    }
}

/// Count audio files under `dir` recursively, with caps so a giant tree
/// (e.g. someone pointing us at the whole of `/sdcard`) can't blow the
/// auto-detect probe time. Caps:
///   - max 4 levels deep (so `Music/Artist/Album/Track.mp3` counts; deeper
///     pathological trees stop)
///   - early-exit once we've found enough audio files to clearly distinguish
///     this candidate from the others
///
/// Hidden dot-directories are skipped (e.g. `.thumbnails`, `.cache`) — they
/// never hold the user's music but can contain decoy media.
#[cfg(target_os = "android")]
fn count_audio_files_recursive(root: &Path) -> usize {
    // Stop counting once we've seen this many — auto-detect only needs a
    // ranking, not an exact total. Saves time on huge libraries.
    const MAX_COUNT: usize = 5_000;
    const MAX_DEPTH: usize = 4;

    let mut n = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if n >= MAX_COUNT {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = match entry.file_type() {
                Ok(ft) => ft.is_dir(),
                Err(_) => path.is_dir(),
            };
            if is_dir {
                if depth + 1 > MAX_DEPTH {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(OsStr::to_str) {
                    if name.starts_with('.') {
                        continue;
                    }
                }
                stack.push((path, depth + 1));
                continue;
            }
            let Some(ext) = path.extension().and_then(OsStr::to_str) else {
                continue;
            };
            let ext_lower = ext.to_ascii_lowercase();
            if SUPPORTED_EXTENSIONS.iter().any(|e| *e == ext_lower) {
                n += 1;
                if n >= MAX_COUNT {
                    break;
                }
            }
        }
    }
    n
}

fn hash_path(path: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Unknown Title")
        .to_string()
}

fn artist_from_path(path: &Path) -> String {
    // Use the immediate parent folder name as a best-effort "artist", which
    // matches how many people organise their music on disk.
    path.parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or("Unknown Artist")
        .to_string()
}

/// Read song metadata via lofty (ID3v2 / iTunes M4A / FLAC / Vorbis comments).
/// Returns (title, artist, album) — each defaulting to the path-derived value
/// or "Unknown ..." if the file has no tag.
fn metadata_from_file(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::Accessor;
    let Ok(probe) = Probe::open(path) else {
        return (None, None, None);
    };
    let Ok(tagged) = probe.read() else {
        return (None, None, None);
    };
    // Prefer the file's "primary" tag (first one); fall back to any tag.
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let Some(tag) = tag else {
        return (None, None, None);
    };
    let clean = |s: String| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    };
    (
        tag.title().map(|s| s.into_owned()).and_then(clean),
        tag.artist().map(|s| s.into_owned()).and_then(clean),
        tag.album().map(|s| s.into_owned()).and_then(clean),
    )
}

fn scan_folder(root: &Path) -> Vec<Song> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden / dot-directories — media indexers and OS
                // caches (`.thumbnails`, `.trash`, `.cache`, `.iTunes`, etc.)
                // can contain decoy media we never want in the library.
                if let Some(name) = path.file_name().and_then(OsStr::to_str) {
                    if name.starts_with('.') {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            // Skip hidden / dot-files too (e.g. `.DS_Store`, dotted partials).
            if let Some(name) = path.file_name().and_then(OsStr::to_str) {
                if name.starts_with('.') {
                    continue;
                }
            }
            let Some(ext) = path.extension().and_then(OsStr::to_str) else {
                continue;
            };
            let ext_lc = ext.to_ascii_lowercase();
            if !SUPPORTED_EXTENSIONS.contains(&ext_lc.as_str()) {
                continue;
            }
            let id = hash_path(&path);
            // Pull real metadata via lofty (ID3v2/iTunes/FLAC/Vorbis), with
            // path-derived fallbacks so untagged files still get sensible
            // values. Title comes from the tag (e.g. "Kesha - Your Love Is My
            // Drug") rather than the filename.
            let (tag_title, tag_artist, tag_album) = metadata_from_file(&path);

            // Deterministically derive variety from the path hash for fields
            // we DON'T have in the tag (genre, play count, duration). Song
            // Summoner uses these to compute trooper stats — uniform values
            // produce identical troopers.
            let genres = [
                "Rock", "Pop", "Hip Hop", "Electronic", "Jazz", "Metal",
                "Classical", "Country", "R&B", "Folk", "Reggae", "Blues",
            ];
            let synth_albums = [
                "Singles", "Greatest Hits", "Anthems", "Sessions",
                "Compilation", "Live", "Deluxe Edition", "Remixes",
            ];
            let genre_idx = (id ^ (id >> 13)) as usize % genres.len();
            let album_idx = (id ^ (id >> 7)) as usize % synth_albums.len();
            let play_count = (((id ^ (id >> 17)) % 95) as u32) + 5;
            let duration_secs = 90.0 + (((id ^ (id >> 5)) % 360) as f64);
            out.push(Song {
                persistent_id: id,
                title: tag_title.unwrap_or_else(|| title_from_path(&path)),
                artist: tag_artist.unwrap_or_else(|| artist_from_path(&path)),
                album: tag_album.unwrap_or_else(|| synth_albums[album_idx].to_string()),
                genre: genres[genre_idx].to_string(),
                play_count,
                duration_secs,
                path: path.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

fn folder_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn load_cache(path: &Path) -> Option<Vec<Song>> {
    let cache_file = library_cache_file();
    let cache_mtime = folder_mtime(&cache_file)?;
    let folder_mt = folder_mtime(path)?;
    if folder_mt > cache_mtime {
        // Music folder changed since we cached; rescan.
        return None;
    }
    let body = fs::read_to_string(&cache_file).ok()?;
    let mut lines = body.lines();
    let header = lines.next()?;
    let mut header_parts = header.split('\t');
    if header_parts.next()? != "touchHLE-music-lib" {
        return None;
    }
    let version: u32 = header_parts.next()?.parse().ok()?;
    if version != LIBRARY_CACHE_VERSION {
        return None;
    }
    let cached_root = header_parts.next()?;
    if cached_root != path.to_string_lossy() {
        return None;
    }
    let mut out: Vec<Song> = Vec::new();
    for line in lines {
        let mut parts = line.split('\t');
        let song = Song {
            persistent_id: u64::from_str_radix(parts.next()?, 16).ok()?,
            title: parts.next()?.to_string(),
            artist: parts.next()?.to_string(),
            album: parts.next()?.to_string(),
            genre: parts.next()?.to_string(),
            play_count: parts.next()?.parse().ok()?,
            duration_secs: parts.next()?.parse().ok()?,
            path: PathBuf::from(parts.next()?),
        };
        out.push(song);
    }
    log!(
        "touchHLE: loaded {} songs from cache {}.",
        out.len(),
        cache_file.display(),
    );
    Some(out)
}

fn save_cache(root: &Path, songs: &[Song]) {
    let mut body = String::with_capacity(songs.len() * 64);
    body.push_str(&format!(
        "touchHLE-music-lib\t{}\t{}\n",
        LIBRARY_CACHE_VERSION,
        root.to_string_lossy()
    ));
    for s in songs {
        // Sanitize tabs/newlines out of strings so the TSV survives round-trip.
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        body.push_str(&format!(
            "{:016X}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            s.persistent_id,
            clean(&s.title),
            clean(&s.artist),
            clean(&s.album),
            clean(&s.genre),
            s.play_count,
            s.duration_secs,
            clean(&s.path.to_string_lossy()),
        ));
    }
    let cache_file = library_cache_file();
    if let Err(e) = fs::write(&cache_file, body) {
        log!("touchHLE: couldn't write {}: {e}", cache_file.display());
    }
}

fn build_library() -> Vec<Song> {
    let configured = saved_library_path();
    let chosen = match configured {
        Some(p) => Some(p),
        None => {
            let picked = prompt_for_folder();
            if let Some(ref p) = picked {
                save_library_path(p);
            } else {
                // Write an empty marker so we don't re-prompt every launch.
                let _ = fs::write(library_path_file(), "");
                log!("touchHLE: no music folder chosen; using built-in fallback library.");
            }
            picked
        }
    };

    let Some(path) = chosen else {
        return fallback_songs();
    };

    if !path.is_dir() {
        log!(
            "touchHLE: music library path {path:?} is missing or not a directory."
        );
        return Vec::new();
    }

    if let Some(cached) = load_cache(&path) {
        return cached;
    }

    let songs = scan_folder(&path);
    if songs.is_empty() {
        log!(
            "touchHLE: no supported audio files found under {path:?}."
        );
        // On Android, the saved path is often an auto-seeded fallback
        // (`/sdcard/touchHLE/Music`) written before the SAF picker ran.
        // If the process exited during the picker (uikit's resign-active
        // handler exits touchHLE), the fallback became permanent — and
        // future launches would forever see 0 songs because the saved
        // path bypasses the candidate-folder auto-detect.
        // Re-run the auto-detect here as a safety net: if a candidate
        // folder has music, swap to it and persist the new choice.
        #[cfg(target_os = "android")]
        {
            if let Some(better) = rescan_android_candidates(&path) {
                log!(
                    "touchHLE: saved music path {path:?} is empty; auto-detected \
                     {better:?} instead. Updating {}.",
                    library_path_file().display(),
                );
                save_library_path(&better);
                let songs = scan_folder(&better);
                if !songs.is_empty() {
                    log!(
                        "touchHLE: scanned {} songs from your music library at {better:?}; caching.",
                        songs.len()
                    );
                    save_cache(&better, &songs);
                    return songs;
                }
            }
        }
        return Vec::new();
    }
    log!(
        "touchHLE: scanned {} songs from your music library at {path:?}; caching.",
        songs.len()
    );
    save_cache(&path, &songs);
    songs
}

/// Android-only safety net: when the saved music path is empty (typically the
/// `/sdcard/touchHLE/Music` auto-seed that MainActivity writes before firing
/// the SAF picker), rescan the well-known candidate folders and return the
/// best one — but only if it's *different* from the path that just scanned
/// empty, to avoid pointless re-scans of the same dir.
#[cfg(target_os = "android")]
fn rescan_android_candidates(empty_path: &Path) -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/sdcard/Music",
        "/sdcard/touchHLE/Music",
        "/sdcard/Download",
    ];
    let mut best: Option<(PathBuf, usize)> = None;
    for raw in CANDIDATES {
        let candidate = PathBuf::from(raw);
        if !candidate.is_dir() {
            continue;
        }
        if candidate == empty_path {
            continue;
        }
        let count = count_audio_files_recursive(&candidate);
        log!("touchHLE:   rescan {raw} -- {count} audio file(s).");
        match &best {
            Some((_, best_count)) if count <= *best_count => {}
            _ => best = Some((candidate, count)),
        }
    }
    best.and_then(|(p, c)| if c > 0 { Some(p) } else { None })
}

struct Library {
    songs: Vec<Song>,
    /// `persistent_id` -> index into `songs`. Built once so the post-pick PID
    /// lookup is O(1) instead of a linear scan; for libraries with thousands
    /// of songs the linear scan was the bottleneck.
    by_pid: HashMap<u64, usize>,
}

fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| {
        let songs = build_library();
        let by_pid: HashMap<u64, usize> = songs
            .iter()
            .enumerate()
            .map(|(i, s)| (s.persistent_id, i))
            .collect();
        log!(
            "touchHLE: music library ready ({} songs, PID index built).",
            songs.len()
        );
        Library { songs, by_pid }
    })
}

pub fn song_count() -> usize {
    library().songs.len()
}

pub fn song(index: usize) -> Option<&'static Song> {
    library().songs.get(index)
}

/// Find a song by its persistent ID and return its library index. Used by
/// MPMediaQuery filter-predicate matching. O(1) thanks to the prebuilt index.
pub fn find_song_index_by_persistent_id(pid: u64) -> Option<usize> {
    library().by_pid.get(&pid).copied()
}

/// When set, [super::media_query] makes this song the index-0 entry of its
/// `-collections` result. Used by the picker-swap hack: tapping a row in the
/// scrollable 1309-row table writes the user's chosen song's PID here, then
/// triggers reloadData on the visible 5-row IPDSongsTab. The game's pick
/// handler then picks the song at index 0 — which is now the user's choice.
static SWAPPED_FIRST_PID: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

pub fn set_swapped_first_pid(pid: Option<u64>) {
    *SWAPPED_FIRST_PID.lock().unwrap() = pid;
}
pub fn swapped_first_pid() -> Option<u64> {
    *SWAPPED_FIRST_PID.lock().unwrap()
}

/// Audio output for picker previews. Lives in a thread-local because
/// `rodio::OutputStream` (cpal `Stream` underneath) is not `Sync`. Calls
/// to `play_song`/`stop_song_preview` must come from the same thread — the
/// picker dispatches them from the main thread, so that's fine.
///
/// Android intentionally has no rodio: cpal's Android backend is oboe, which
/// requires `ndk_context::initialize_android_context()` to be called before
/// any audio is touched (otherwise it panics with "android context was not
/// initialized" as soon as some background poll runs). The picker-preview
/// nicety isn't worth the JNI plumbing to set that up, so on Android we stub
/// the player out entirely.
#[cfg(not(target_os = "android"))]
struct PlayerState {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
    current_sink: Option<rodio::Sink>,
}

#[cfg(not(target_os = "android"))]
thread_local! {
    static PLAYER: std::cell::RefCell<Option<PlayerState>> = std::cell::RefCell::new(None);
}

#[cfg(not(target_os = "android"))]
fn with_player<R>(f: impl FnOnce(&mut PlayerState) -> R) -> Option<R> {
    PLAYER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => {
                    log!("touchHLE: picker preview audio output ready.");
                    *borrow = Some(PlayerState {
                        _stream: stream,
                        handle,
                        current_sink: None,
                    });
                }
                Err(e) => {
                    log!("touchHLE: couldn't open audio output for picker preview: {e}");
                    return None;
                }
            }
        }
        borrow.as_mut().map(f)
    })
}

/// Play the song at `index` through the host's default audio output (replacing
/// any currently-playing preview). Mirrors the real iPod picker behaviour of
/// previewing the track as soon as you tap it. Silently no-ops on failure so
/// audio glitches never crash the picker flow.
///
/// On Android this is a stub (see comment on `PlayerState`).
#[cfg(target_os = "android")]
pub fn play_song(_index: usize) {}

#[cfg(not(target_os = "android"))]
pub fn play_song(index: usize) {
    let Some(song) = song(index) else { return };
    let path = song.path.clone();
    if path.as_os_str().is_empty() {
        return;
    }
    with_player(|state| {
        if let Some(prev) = state.current_sink.take() {
            prev.stop();
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                log!("touchHLE: couldn't open {:?} for preview: {e}", path);
                return;
            }
        };
        // rodio's symphonia backend has a panic-on-seek bug for some files
        // (notably AAC/M4A), bubbling up as "Seek errors should not occur
        // during initialization" instead of a clean Err. Catch the panic so
        // a broken file just silently skips the preview rather than crashing
        // the picker.
        let decoder = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rodio::Decoder::new(std::io::BufReader::new(file))
        })) {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => {
                log!("touchHLE: couldn't decode {:?} for preview: {e}", path);
                return;
            }
            Err(_) => {
                log!(
                    "touchHLE: decoder panicked initialising {:?} (likely an unsupported AAC/M4A); skipping preview.",
                    path
                );
                return;
            }
        };
        let sink = match rodio::Sink::try_new(&state.handle) {
            Ok(s) => s,
            Err(e) => {
                log!("touchHLE: couldn't create rodio sink: {e}");
                return;
            }
        };
        // Cap preview at 30s, matching iTunes/Apple Music preview length.
        use rodio::Source;
        let clipped = decoder.take_duration(std::time::Duration::from_secs(30));
        sink.append(clipped);
        sink.play();
        state.current_sink = Some(sink);
        log!("touchHLE: previewing song #{index} ({:?})", path);
    });
}

/// Stop any currently-playing picker preview. Used when the picker is
/// dismissed or another song is about to start.
///
/// On Android this is a stub (see comment on `PlayerState`). It must be a
/// real no-op rather than calling into a stubbed `with_player`, because
/// `UIControl.touchesEnded` calls this on every button tap including in the
/// picker -- reaching into rodio/oboe even once panics on Android.
#[cfg(target_os = "android")]
pub fn stop_song_preview() {}

#[cfg(not(target_os = "android"))]
pub fn stop_song_preview() {
    with_player(|state| {
        if let Some(prev) = state.current_sink.take() {
            prev.stop();
        }
    });
}

/// How many songs MPMediaQuery should expose to the game. Defaults to the
/// full scanned library; override with `TOUCHHLE_MUSIC_LIBRARY_LIMIT=N` to
/// clamp for testing.
pub fn query_song_count() -> usize {
    let env_raw = std::env::var("TOUCHHLE_MUSIC_LIBRARY_LIMIT");
    let limit = env_raw
        .as_ref()
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_SONGS);
    let total = song_count();
    let exposed = total.min(limit);
    // Log once per process so we can see if a stale env var or a cache miss
    // is capping what the game sees.
    use std::sync::OnceLock;
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        log!(
            "music_library: query_song_count -> {} (library has {}, env_limit={:?}, default={})",
            exposed, total, env_raw, DEFAULT_MAX_SONGS
        );
    });
    exposed
}
