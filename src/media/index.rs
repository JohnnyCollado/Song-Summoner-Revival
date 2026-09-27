/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The library index (`library/index.tsv`) and the sorted views built on it.
//!
//! The format is shared with the Android scanner (`LibraryScanner.kt`), so
//! any change here must be made there too. The first line is [HEADER], then
//! one song per line, tab-separated, in this column order:
//!
//! `id  locator  mtime  size  duration_ms  track  title  artist  album
//!  album_artist  genre  folder  has_art`
//!
//! - `id` is the persistent ID as an unsigned decimal, see [persistent_id].
//! - `locator` says where the file is: a `content://` document URI on
//!   Android, a path relative to the music folder (with `/`) on the desktop.
//! - `mtime` (seconds) and `size` (bytes) let a rescan skip unchanged files.
//! - `folder` is the song's folder relative to the music folder, `/`
//!   separated, empty for the top level. It feeds the Playlist tab.
//! - `has_art` is `1` when `art/<id>.png` exists.
//!
//! Text fields escape `\`, tab, newline and carriage return as `\\`, `\t`,
//! `\n` and `\r`.

use std::cmp::Ordering;
use std::collections::HashMap;

pub const HEADER: &str = "SSLIB\t1";
const COLUMNS: usize = 13;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Song {
    pub id: u64,
    pub locator: String,
    pub mtime: u64,
    pub size: u64,
    pub duration_ms: u64,
    /// Track number on its album, 0 if unknown.
    pub track: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub folder: String,
    pub has_art: bool,
}

impl Song {
    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() {
            "Unknown Artist"
        } else {
            &self.artist
        }
    }
    pub fn display_album(&self) -> &str {
        if self.album.is_empty() {
            "Unknown Album"
        } else {
            &self.album
        }
    }
    pub fn duration_secs(&self) -> f64 {
        self.duration_ms as f64 / 1000.0
    }
}

/// The persistent ID of a song: a 64-bit FNV-1a hash of a key that stays the
/// same across rescans (the SAF document ID on Android, the lower-cased path
/// relative to the music folder on the desktop). Never 0, which the game
/// treats as "no song".
pub fn persistent_id(key: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    if hash == 0 {
        1
    } else {
        hash
    }
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Inverse of [escape]. An unknown escape is kept as written.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Result of [parse]: the songs, and how many rows were unusable (bad
/// numbers, wrong column count, a zero or repeated ID).
#[derive(Debug)]
pub struct Parsed {
    pub songs: Vec<Song>,
    pub skipped: usize,
}

/// Parse an index file. A missing or unknown header is an error (the caller
/// then treats the library as empty); bad rows are only skipped, so one
/// broken line doesn't hide the whole library.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("").trim_start_matches('\u{feff}');
    if header != HEADER {
        return Err(format!("unknown index header {header:?}"));
    }
    let mut songs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut skipped = 0;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        match parse_row(line) {
            Some(song) if seen.insert(song.id) => songs.push(song),
            _ => skipped += 1,
        }
    }
    Ok(Parsed { songs, skipped })
}

fn parse_row(line: &str) -> Option<Song> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != COLUMNS {
        return None;
    }
    let id: u64 = fields[0].parse().ok()?;
    if id == 0 {
        return None;
    }
    Some(Song {
        id,
        locator: unescape(fields[1]),
        mtime: fields[2].parse().ok()?,
        size: fields[3].parse().ok()?,
        duration_ms: fields[4].parse().ok()?,
        track: fields[5].parse().ok()?,
        title: unescape(fields[6]),
        artist: unescape(fields[7]),
        album: unescape(fields[8]),
        album_artist: unescape(fields[9]),
        genre: unescape(fields[10]),
        folder: unescape(fields[11]),
        has_art: fields[12] == "1",
    })
}

pub fn serialize(songs: &[Song]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');
    for s in songs {
        let row = [
            s.id.to_string(),
            escape(&s.locator),
            s.mtime.to_string(),
            s.size.to_string(),
            s.duration_ms.to_string(),
            s.track.to_string(),
            escape(&s.title),
            escape(&s.artist),
            escape(&s.album),
            escape(&s.album_artist),
            escape(&s.genre),
            escape(&s.folder),
            if s.has_art { "1" } else { "0" }.to_string(),
        ];
        out.push_str(&row.join("\t"));
        out.push('\n');
    }
    out
}

/// Fold the accented Latin letters people actually have in song names, so
/// "Über" sorts and files under U the way the iPod does.
fn fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }
}

/// The key lists are sorted by: case and common accents ignored, and a
/// leading "The " dropped ("The Lanterns" sorts under L).
pub fn sort_key(s: &str) -> String {
    let lower: String = s
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .collect();
    match lower.strip_prefix("the ") {
        Some(rest) if !rest.trim().is_empty() => rest.trim_start().to_string(),
        _ => lower,
    }
}

/// The A–Z section a sort key belongs to. Digits, symbols and non-Latin
/// scripts all go under `#`, which comes after Z.
pub fn section_of(key: &str) -> char {
    match key.chars().next() {
        Some(c) if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

/// Order two sort keys: by section (A…Z, then #), then by the key itself.
pub fn compare_keys(a: &str, b: &str) -> Ordering {
    let rank = |k: &str| match section_of(k) {
        '#' => 26,
        c => c as u32 - 'A' as u32,
    };
    rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
}

/// A run of rows under one letter: `first` is the index of its first entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub letter: char,
    pub first: usize,
}

/// Split an already-sorted list into sections.
pub fn sections<'a>(sorted_names: impl Iterator<Item = &'a str>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for (i, name) in sorted_names.enumerate() {
        let letter = section_of(&sort_key(name));
        if out.last().map(|s| s.letter) != Some(letter) {
            out.push(Section { letter, first: i });
        }
    }
    out
}

/// An artist, album or playlist: a name and its songs (indices into
/// [Library::songs]) in display order.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub songs: Vec<usize>,
}

/// Which property an `MPMediaPropertyPredicate` tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    PersistentId,
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterValue {
    Id(u64),
    Text(String),
}

/// A query filter. Equal-to compares exactly; contains ignores case.
#[derive(Clone, Debug)]
pub struct Filter {
    pub property: Property,
    pub value: FilterValue,
    pub contains: bool,
}

impl Filter {
    pub fn matches(&self, song: &Song) -> bool {
        let text = match self.property {
            Property::PersistentId => {
                return match self.value {
                    FilterValue::Id(id) => id == song.id,
                    FilterValue::Text(ref t) => t.trim().parse() == Ok(song.id),
                };
            }
            Property::Title => &song.title,
            Property::Artist => &song.artist,
            Property::Album => &song.album,
            Property::AlbumArtist => &song.album_artist,
            Property::Genre => &song.genre,
        };
        let wanted = match self.value {
            FilterValue::Text(ref t) => t.clone(),
            FilterValue::Id(id) => id.to_string(),
        };
        if self.contains {
            text.to_lowercase().contains(&wanted.to_lowercase())
        } else {
            *text == wanted
        }
    }
}

/// The parsed library with every view the MediaPlayer classes and the picker
/// need, built once when the index is loaded.
#[derive(Debug, Default)]
pub struct Library {
    pub songs: Vec<Song>,
    by_id: HashMap<u64, usize>,
    /// All songs, sorted by title.
    pub by_title: Vec<usize>,
    pub artists: Vec<Group>,
    pub albums: Vec<Group>,
    /// One per folder that directly holds songs.
    pub playlists: Vec<Group>,
}

impl Library {
    pub fn new(songs: Vec<Song>) -> Library {
        let by_id = songs.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        let title_keys: Vec<String> = songs.iter().map(|s| sort_key(&s.title)).collect();

        let mut by_title: Vec<usize> = (0..songs.len()).collect();
        by_title.sort_by(|&a, &b| {
            compare_keys(&title_keys[a], &title_keys[b]).then_with(|| songs[a].id.cmp(&songs[b].id))
        });

        let by_title_order = |group: &mut Vec<usize>| {
            group.sort_by(|&a, &b| {
                compare_keys(&title_keys[a], &title_keys[b])
                    .then_with(|| songs[a].id.cmp(&songs[b].id))
            })
        };

        let mut artists = group_by(&songs, &by_title, |s| s.display_artist());
        for g in &mut artists {
            by_title_order(&mut g.songs);
        }
        let mut albums = group_by(&songs, &by_title, |s| s.display_album());
        for g in &mut albums {
            // Album order: by track number, songs without one last.
            g.songs.sort_by(|&a, &b| {
                let track = |i: usize| match songs[i].track {
                    0 => u32::MAX,
                    t => t,
                };
                track(a)
                    .cmp(&track(b))
                    .then_with(|| compare_keys(&title_keys[a], &title_keys[b]))
            });
        }
        let with_folder: Vec<usize> = by_title
            .iter()
            .copied()
            .filter(|&i| !songs[i].folder.is_empty())
            .collect();
        let mut playlists = group_by(&songs, &with_folder, |s| s.folder.as_str());
        for g in &mut playlists {
            by_title_order(&mut g.songs);
        }

        Library {
            songs,
            by_id,
            by_title,
            artists,
            albums,
            playlists,
        }
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.by_id.get(&id).copied()
    }
    pub fn get(&self, id: u64) -> Option<&Song> {
        self.index_of(id).map(|i| &self.songs[i])
    }

    pub fn title_sections(&self) -> Vec<Section> {
        sections(self.by_title.iter().map(|&i| self.songs[i].title.as_str()))
    }

    /// Songs (in title order) that pass every filter.
    pub fn filtered(&self, filters: &[Filter]) -> Vec<usize> {
        self.by_title
            .iter()
            .copied()
            .filter(|&i| filters.iter().all(|f| f.matches(&self.songs[i])))
            .collect()
    }
}

/// Section list for a sorted group list.
pub fn group_sections(groups: &[Group]) -> Vec<Section> {
    sections(groups.iter().map(|g| g.name.as_str()))
}

/// Group `order` (song indices) by a name. Names that differ only in case,
/// accents or a leading "The " share a group, named after the first song
/// seen. Groups come out sorted by name.
fn group_by<'s>(
    songs: &'s [Song],
    order: &[usize],
    name_of: impl Fn(&'s Song) -> &'s str,
) -> Vec<Group> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    let mut index_by_key: HashMap<String, usize> = HashMap::new();
    for &i in order {
        let name = name_of(&songs[i]);
        let key = sort_key(name);
        let at = *index_by_key.entry(key.clone()).or_insert_with(|| {
            groups.push((
                key,
                Group {
                    name: name.to_string(),
                    songs: Vec::new(),
                },
            ));
            groups.len() - 1
        });
        groups[at].1.songs.push(i);
    }
    groups.sort_by(|a, b| compare_keys(&a.0, &b.0));
    groups.into_iter().map(|(_, g)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/library/index.tsv");

    fn fixture() -> Library {
        Library::new(parse(FIXTURE).unwrap().songs)
    }

    fn titles(lib: &Library, list: &[usize]) -> Vec<String> {
        list.iter().map(|&i| lib.songs[i].title.clone()).collect()
    }

    #[test]
    fn persistent_id_is_fnv1a_64() {
        assert_eq!(persistent_id(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(persistent_id("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(persistent_id("foobar"), 0x8594_4171_f739_67e8);
        // Stable: the same key always gives the same ID.
        assert_eq!(persistent_id("rock/song.mp3"), persistent_id("rock/song.mp3"));
        assert_ne!(persistent_id("rock/song.mp3"), persistent_id("rock/song2.mp3"));
    }

    #[test]
    fn escape_round_trips() {
        for s in ["plain", "tab\there", "line\nbreak", "cr\rhere", "back\\slash", "\\t", ""] {
            assert_eq!(unescape(&escape(s)), s);
        }
        assert_eq!(escape("a\tb\\c\nd"), "a\\tb\\\\c\\nd");
        assert!(!escape("x\ty\nz").contains(['\t', '\n']));
        // Unknown escapes and a trailing backslash are kept as written.
        assert_eq!(unescape("\\q"), "\\q");
        assert_eq!(unescape("end\\"), "end\\");
    }

    #[test]
    fn parses_fixture() {
        let parsed = parse(FIXTURE).unwrap();
        // 8 good rows; the non-numeric row, the short row and the repeated
        // ID are skipped.
        assert_eq!(parsed.songs.len(), 8);
        assert_eq!(parsed.skipped, 3);

        let first = &parsed.songs[0];
        assert_eq!(first.id, 1001);
        assert_eq!(first.locator, "Rock/Afterglow Anthem.mp3");
        assert_eq!(first.mtime, 1_700_000_000);
        assert_eq!(first.size, 4_000_000);
        assert_eq!(first.duration_ms, 215_000);
        assert_eq!(first.track, 1);
        assert_eq!(first.artist, "The Lanterns");
        assert_eq!(first.album_artist, "The Lanterns");
        assert_eq!(first.genre, "Rock");
        assert_eq!(first.folder, "Rock");
        assert!(first.has_art);

        let odd = parsed.songs.iter().find(|s| s.id == 1006).unwrap();
        assert_eq!(odd.locator, "Tabs\tand\\slashes.mp3");
        assert_eq!(odd.title, "Tab\there");
        assert_eq!(odd.artist, "Line\nBreak");
        assert_eq!(odd.album, "Back\\slash");
        assert!(!odd.has_art);

        // The first copy of a repeated ID wins.
        assert_eq!(
            parsed.songs.iter().filter(|s| s.id == 1001).count(),
            1
        );
    }

    #[test]
    fn accepts_crlf_and_rejects_bad_headers() {
        let crlf = FIXTURE.replace('\n', "\r\n");
        assert_eq!(parse(&crlf).unwrap().songs.len(), 8);
        assert!(parse("").is_err());
        assert!(parse("SSLIB\t2\n").is_err());
        assert!(parse("garbage\n1\t2\n").is_err());
        assert!(parse("SSLIB\t1\n").unwrap().songs.is_empty());
    }

    #[test]
    fn serialize_round_trips() {
        let songs = parse(FIXTURE).unwrap().songs;
        let text = serialize(&songs);
        assert!(text.starts_with("SSLIB\t1\n"));
        let again = parse(&text).unwrap();
        assert_eq!(again.skipped, 0);
        assert_eq!(again.songs, songs);
    }

    #[test]
    fn sort_keys_and_sections() {
        assert_eq!(sort_key("The Lanterns"), "lanterns");
        assert_eq!(sort_key("  THE end "), "end");
        // "The" on its own, or as part of a word, stays.
        assert_eq!(sort_key("The"), "the");
        assert_eq!(sort_key("Theory"), "theory");
        assert_eq!(sort_key("Über Alles"), "uber alles");
        assert_eq!(section_of(&sort_key("Über Alles")), 'U');
        assert_eq!(section_of(&sort_key("99 Luftballons")), '#');
        assert_eq!(section_of(&sort_key("夜に駆ける")), '#');
        assert_eq!(section_of(&sort_key("!Bang")), '#');
        assert_eq!(section_of(""), '#');
        // Letters before #, and # entries sorted among themselves.
        assert_eq!(compare_keys("zebra", "99"), Ordering::Less);
        assert_eq!(compare_keys("99", "夜"), Ordering::Less);
        assert_eq!(compare_keys("apple", "Apple".to_lowercase().as_str()), Ordering::Equal);
    }

    #[test]
    fn songs_sorted_by_title_with_sections() {
        let lib = fixture();
        assert_eq!(
            titles(&lib, &lib.by_title),
            [
                "Across the Static",
                "Afterglow Anthem",
                "Blue Hour Parade",
                "The End",
                "Tab\there",
                "Über Alles",
                "99 Luftballons",
                "夜に駆ける",
            ]
        );
        let letters: Vec<(char, usize)> = lib
            .title_sections()
            .iter()
            .map(|s| (s.letter, s.first))
            .collect();
        assert_eq!(
            letters,
            [('A', 0), ('B', 2), ('E', 3), ('T', 4), ('U', 5), ('#', 6)]
        );
    }

    #[test]
    fn groups_artists() {
        let lib = fixture();
        let names: Vec<&str> = lib.artists.iter().map(|g| g.name.as_str()).collect();
        // "The Lanterns" and "the Lanterns" are one artist, filed under L.
        assert_eq!(
            names,
            [
                "Émile",
                "The Lanterns",
                "Line\nBreak",
                "Marigold Club",
                "Nena",
                "Neon Harbor",
                "YOASOBI",
            ]
        );
        let lanterns = &lib.artists[1];
        assert_eq!(titles(&lib, &lanterns.songs), ["Afterglow Anthem", "The End"]);
        let letters: Vec<char> = group_sections(&lib.artists).iter().map(|s| s.letter).collect();
        assert_eq!(letters, ['E', 'L', 'M', 'N', 'Y']);
    }

    #[test]
    fn groups_albums_in_track_order() {
        let lib = fixture();
        let names: Vec<&str> = lib.albums.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Back\\slash",
                "THE BOOK",
                "Ñandú",
                "Paper Suns",
                "Porchlight Static",
                "Riverglass",
                "99",
            ]
        );
        let paper_suns = &lib.albums[3];
        // Track 1 before track 2, even though "Across" sorts first by title.
        assert_eq!(
            titles(&lib, &paper_suns.songs),
            ["Afterglow Anthem", "Across the Static"]
        );
    }

    #[test]
    fn folders_become_playlists() {
        let lib = fixture();
        let names: Vec<&str> = lib.playlists.iter().map(|g| g.name.as_str()).collect();
        // Top-level songs aren't in any playlist; a subfolder is its own one.
        assert_eq!(names, ["Rock", "Rock/Live"]);
        assert_eq!(
            titles(&lib, &lib.playlists[0].songs),
            ["Across the Static", "Afterglow Anthem"]
        );
        assert_eq!(titles(&lib, &lib.playlists[1].songs), ["The End"]);
    }

    #[test]
    fn lookup_by_persistent_id() {
        let lib = fixture();
        assert_eq!(lib.get(1004).unwrap().title, "The End");
        assert!(lib.get(4242).is_none());
        assert!(lib.get(0).is_none());
    }

    #[test]
    fn filters() {
        let lib = fixture();
        let by_id = Filter {
            property: Property::PersistentId,
            value: FilterValue::Id(1003),
            contains: false,
        };
        assert_eq!(titles(&lib, &lib.filtered(&[by_id])), ["Blue Hour Parade"]);

        let album = Filter {
            property: Property::Album,
            value: FilterValue::Text("Paper Suns".into()),
            contains: false,
        };
        assert_eq!(
            titles(&lib, &lib.filtered(&[album.clone()])),
            ["Across the Static", "Afterglow Anthem"]
        );

        // Filters combine with AND.
        let artist = Filter {
            property: Property::Artist,
            value: FilterValue::Text("Neon Harbor".into()),
            contains: false,
        };
        assert_eq!(
            titles(&lib, &lib.filtered(&[album, artist])),
            ["Across the Static"]
        );

        // Equal-to is exact, contains ignores case.
        let exact = Filter {
            property: Property::Title,
            value: FilterValue::Text("the end".into()),
            contains: false,
        };
        assert!(lib.filtered(&[exact]).is_empty());
        let contains = Filter {
            property: Property::Title,
            value: FilterValue::Text("THE".into()),
            contains: true,
        };
        assert_eq!(
            titles(&lib, &lib.filtered(&[contains])),
            // "Anthem" contains "the" too.
            ["Across the Static", "Afterglow Anthem", "The End"]
        );
    }

    #[test]
    fn empty_library() {
        let lib = Library::new(Vec::new());
        assert!(lib.is_empty());
        assert!(lib.title_sections().is_empty());
        assert!(lib.artists.is_empty() && lib.albums.is_empty() && lib.playlists.is_empty());
    }
}
