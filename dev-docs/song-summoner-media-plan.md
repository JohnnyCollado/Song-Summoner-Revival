# Song Summoner: media + music picker rewrite (implementation plan)

Status: **implemented (M0–M6), awaiting on-device testing.** Written
2026-09-26; implemented the same day. Where the code differs from the
original plan, the text below says so ("Changed in implementation").
Background and addresses: [song-summoner-re.md](song-summoner-re.md).
Mockup: run `python inspect/picker_mockup.py` from the repo root. It
renders from your own IPA into `debug/picker-mockup/` (gitignored). The
game's art is never committed.

---

## Part 1: How it works (for humans)

### What the player sees

**First launch on Android**

1. The app asks for storage access (as today), then for the game's IPA
   (as today).
2. **New:** Android's own folder picker opens: "Choose the folder with
   your music". The player taps their music folder (for example
   `Music/`) and then **Use this folder**.
3. A progress screen appears: "Reading your music library… 812 / 1,317".
   This happens once. Later launches only re-read files that changed, so
   they take a second or two.
4. The game starts.

**First launch on Windows**

1. A normal Windows "Select folder" dialog asks for the music folder.
2. The library is read in the background while the game boots.

**Picking a song in the game**

1. In the Soul Master's palace the player chooses the iPod option. The
   game fades out as it always did.
2. The **new picker** slides in (see mockup). It has four tabs at the
   bottom: **Song, Artist, Album, Playlist**. It looks like the game's
   original iPod picker: dark rows, a blue glow on the touched row, and
   the fighter portrait fading in on the right of every song.
3. The player scrolls, uses the A–Z strip on the right to jump, or opens
   an artist/album/playlist to see its songs.
4. Tapping a song picks it. The picker disappears and the game shows its
   own "Create this trooper?" panel with the song's title, artist and
   artwork.
5. **No** on that panel brings the picker back exactly where it was.
   **Cancel** in the picker's top-right corner leaves the iPod menu.
6. Later, when that trooper's song plays in battle or on the results
   screen, it is the player's real audio file.

**Changing the music folder later**

- Android: long-press the app icon → **Change music folder**. This opens
  the setup screen again, before the game runs.
- Windows: start with `--choose-music-folder`, or delete
  `library\source.txt` from the game folder.

*Changed in implementation:* the Android shortcut is a **dynamic**
shortcut published by `SetupActivity`, not a static `shortcuts.xml`: a
static one must spell out the package id, which differs per flavor. If the
game is running when the shortcut is used, touchHLE exits on focus loss,
so close the game first.

### What happens under the hood, in plain words

- **The scanner** (Kotlin on Android, Rust on Windows) walks the music
  folder once. It writes a plain text **index** of every song: its ID,
  where it lives, title, artist, album and length. Album art is cached as
  small image files next to the index.
- **The engine** (touchHLE) never walks folders itself. It reads the
  index and pretends to be the iPhone's music library, so the game's own
  code can query songs the way it did on a 2010 iPhone.
- **The picker** belongs to us, not the game. The game only asks to
  "show a picker" and waits for a song ID or a cancel. Everything else
  (hiding the picker, the confirm panel, bringing it back on "No") is
  already the game's job, so we let it do that job.
- **Playback** decodes the player's file with the same audio code
  touchHLE already uses for the game's own music, so it works on Windows
  and Android alike.

---

## Part 2: Decisions already made

| Topic | Decision |
|---|---|
| Scope | Rewrite **all** media code from scratch: scanner, index, MediaPlayer framework classes, playback, picker. Nothing from the old implementation is reused. |
| Screen size | Picker renders at the game's native **480×320**. The widescreen hack is a separate, later project (see RE doc §4 and the `glOrthof` note). |
| Look | The mockup: the game's own picker art drawn **1:1, never resampled**; LiberationSans text (touchHLE's UIKit font). |
| Input | Touch only. Controller support comes later and does not affect this design. |
| Android scanning | Storage Access Framework (SAF) tree URI, scanned in Kotlin before the engine starts. |
| Windows scanning | Native folder dialog (`rfd`) plus a background filesystem walk, writing the same index format. |
| Legal | The picker loads the game's art from the user's IPA at run time. No game art is committed or bundled. |

### Formerly open decisions

All five were implemented with their defaults:

| Topic | Decision |
|---|---|
| Storage | `/sdcard/SongSummoner/` + `MANAGE_EXTERNAL_STORAGE` for user data (index, art, play counts); SAF only for music. |
| Playlist tab | Each folder that directly holds songs is a playlist (named by its path, e.g. `Rock/Live`). Top-level songs are in no playlist. |
| Fighter portraits | The picker calls the game's own `getGraphicNo(u64)` (symbol `__Z12getGraphicNoy`, called as Thumb) for each visible song and shows `list_fighter%03d.png` for a result in 0–89; anything else shows no portrait. **Unverified** until tested: the first 8 results are logged as `picker: fighter for … is …`. If they're wrong, trace `Music.fid` in `nowLoading` (0x5bd20) as originally planned. |
| Persistent IDs | New stable scheme (§3.1). Troopers saved by the old build won't find their songs (accepted, pre-release). |
| Play counts | A play counts once our player passes 50% of the song or 4 minutes; stored in `library/playcounts.tsv`. |

The original notes follow.

1. **Storage.** **Keep `/sdcard/SongSummoner/` for user data and
   `MANAGE_EXTERNAL_STORAGE`; use SAF only for music.** The alternative is
   SAF for everything, including the IPA and user data in app storage.
   That is friendlier to the Play Store, but the files get harder to
   reach by hand.
2. **Playlist tab.** **Each subfolder of the music folder is a
   "playlist".** Alternatives: read `.m3u` files, or drop the tab.
3. **Fighter portraits.** **Trace how the game computes `Music.fid` in
   `-[IPDSongsTableViewController nowLoading]` (0x5bd20) and call the same
   code.** If it depends on random rolls, show the base fighter or no
   portrait.
4. **Persistent IDs vs. old saves.** **New stable scheme (§3.1).** Saves
   made with the old build store the old IDs, so their troopers won't
   find their songs. We accept that (pre-release) or add a one-time
   old→new ID map.
5. **Play counts.** The game reads `MPMediaItemPropertyPlayCount` in
   `calclisteningpoint` (listening points) and `PalaceFlow_Recommend`.
   **Count a play when our player gets past 50% of a song or 4 minutes;
   store counts in `library/playcounts.tsv`.**

---

## Part 3: Architecture

```
 Android (Kotlin, before engine)       Windows (Rust, at boot)
 ┌──────────────────────────┐          ┌──────────────────────────┐
 │ SetupActivity            │          │ library::scan_windows    │
 │  SAF tree picker         │          │  rfd folder dialog       │
 │  LibraryScanner (thread) │          │  walkdir + lofty tags    │
 └────────────┬─────────────┘          └────────────┬─────────────┘
              │ writes                               │ writes
              ▼                                      ▼
        <userdata>/library/index.tsv  +  art/<id>.png  +  source.txt
              │ read at boot
              ▼
 ┌────────────────────────────────────────────────────────────────┐
 │ src/media/  (new, host-side, no guest types)                   │
 │   index.rs    parse + sort + group (songs/artists/albums/…)    │
 │   source.rs   open a song's bytes: path | Android fd via JNI   │
 │   playback.rs symphonia decode → OpenAL source (existing stack)│
 │   playcount.rs                                                 │
 └───────────────┬─────────────────────────────┬──────────────────┘
                 │                             │
 ┌───────────────▼──────────────┐  ┌───────────▼──────────────────┐
 │ frameworks/media_player/     │  │ frameworks/song_summoner/    │
 │  MPMediaQuery, MPMediaItem,  │  │  picker_hook.rs  (overrides  │
 │  MPMediaItemCollection,      │  │    IPDMediaPickerController) │
 │  MPMediaPropertyPredicate,   │  │  picker_view.rs  (SSPicker…) │
 │  MPMusicPlayerController     │  │  picker_art.rs   (IPA art)   │
 └──────────────────────────────┘  └──────────────────────────────┘
                 ▲                             │ didPickMediaNumber:
                 │ songsQuery + PID predicate  ▼ / mediaPickerDidCancel:
             ┌────────────────── game (Palace_Main, iPodView2) ───┐
```

### 3.1 Library index (shared format)

Location: `<userdata>/library/`. On Android that is
`/sdcard/SongSummoner/library/`; on Windows, `library\` next to the exe.

| File | Contents |
|---|---|
| `source.txt` | Android: the tree URI. Windows: the folder path. |
| `index.tsv` | Header line `SSLIB\t1`, then one song per line: `id  locator  mtime  size  duration_ms  track  title  artist  album  album_artist  genre  folder  has_art`. Tab-separated, with `\t`, `\n`, `\r` and `\\` escaped. `id` is an unsigned decimal. |
| `art/<id>.png` | Embedded cover art, re-encoded to PNG, longest side ≤ 256. This is the user's art, not the game's, so resizing it is fine. |
| `playcounts.tsv` | `id  count` |

- **`id` (the persistent ID)** is a 64-bit FNV-1a hash of a stable key,
  never 0. The key is the SAF document ID on Android, or the lower-cased
  path relative to the music folder on Windows.
- **`locator`** is the document URI on Android, or the relative path on
  Windows.
- **`folder`** (*added in implementation*) is the song's folder relative
  to the music folder, `/`-separated, empty at the top level. It feeds the
  Playlist tab; an Android document URI can't be turned back into a path
  reliably.
- Only formats the engine can decode are indexed: mp3, m4a, aac, wav,
  caf (*changed in implementation*: FLAC and Ogg are skipped because
  touchHLE's Symphonia build has no decoder for them, and a song that is
  listed but never plays is worse than one that isn't listed).
- A **rescan** keeps any row whose `mtime`+`size` are unchanged and only
  re-reads tags for new or changed files.
- The **Rust reader** tolerates a missing or corrupt index by treating
  the library as empty. The game then hides the iPod option on its own,
  because `SysiPodList_Get_SongCount` returns 0.

### 3.2 Android scanner (Kotlin)

New file `android/.../LibraryScanner.kt`. `SetupActivity` gets a
rewritten step 3.

1. If `source.txt` is missing, or its URI no longer has a persisted
   permission, launch `ACTION_OPEN_DOCUMENT_TREE` (hint: `Music/`). On
   the result, call `takePersistableUriPermission(uri, READ)` and write
   `source.txt`.
2. Scan on a worker thread and show a progress dialog, all inside
   `SetupActivity` (touchHLE isn't running yet, so the focus rule is
   safe).
   - Walk with `DocumentsContract.buildChildDocumentsUriUsingTree` +
     `ContentResolver.query` (projection: id, name, mime, mtime, size).
     Recurse into directories. Don't use `DocumentFile`: it does one IPC
     per property and is far slower.
   - Keep files whose extension is on the allow-list (mp3, m4a, aac, wav,
     caf), or with no extension and a matching audio MIME type.
   - Tags: `MediaMetadataRetriever.setDataSource(context, uri)` for
     title, artist, album, album artist, genre, duration and track. Art:
     `embeddedPicture` → scale → `art/<id>.png`.
   - Fall back to the file name for the title and the parent folder name
     for the album.
3. Write `index.tsv.tmp`, then rename it to `index.tsv`, then start
   `MainActivity`.
4. **Change music folder:** a static app shortcut
   (`res/xml/shortcuts.xml`) that starts `SetupActivity` with an extra
   forcing the tree picker.
5. **Rust needs file bytes at play time.** `MusicFiles.openFd(uri): Int`
   is a `@JvmStatic` Kotlin helper doing
   `contentResolver.openFileDescriptor(uri, "r")!!.detachFd()`. Rust calls
   it over JNI (`SDL_AndroidGetJNIEnv`) and wraps the fd in a
   `std::fs::File`. It opens no activity, so it's safe while touchHLE
   runs.

Delete the old step-3 code: folder auto-detect, tree-URI → filesystem
path resolution, and `touchHLE_music_library.txt` handling.

### 3.3 Windows scanner (Rust)

`src/media/scan_windows.rs`, compiled for `not(target_os = "android")`:

- **At boot, before the SDL window:** if `source.txt` is missing (or
  `--choose-music-folder` was passed), show an `rfd` folder dialog.
- **Then scan on a host thread** (`walkdir`-style recursion, `lofty`
  tags + pictures). The index is published to the engine when done.
- *Changed in implementation:* this only happens with `--music-library`,
  which `touchHLE_default_options.txt` sets for Song Summoner, so other
  apps never get a folder dialog.
- **Until then** the MediaPlayer layer reports "loading", and the picker
  shows the loading state from the mockup (cube + count). If the game
  asks for the song count before the scan finishes, answer with the
  previous index.

### 3.4 `src/media/` (host library core)

This layer is pure Rust with no ObjC types, so it can be unit-tested
with `cargo test`.

- `index.rs`: `Library { songs: Vec<Song>, by_id: HashMap<u64, usize> }`
  plus precomputed views: songs sorted by title, artists, albums and
  playlists (folders), each with A–Z sections. Sorting ignores case and
  leading "The ", and puts digits and symbols under `#`.
- `source.rs`: `open(song) -> io::Result<File>`, by path on Windows or
  the JNI fd on Android.
- `artwork.rs`: loads `art/<id>.png` on demand (small LRU).
- `playback.rs`: one player. `symphonia` (already a dependency) decodes
  into a streaming OpenAL source through touchHLE's existing OpenAL
  wrapper. Supports play, stop, volume, seek, repeat and shuffle queues.
  Replaces the old `rodio` path, which was desktop-only.
  (Seeking decodes and discards from the start of the file; repeat and
  shuffle live in `MPMusicPlayerController`.)
- `playcount.rs`: load/save `playcounts.tsv`; `note_progress(id, t)`.

### 3.5 MediaPlayer framework (guest-facing classes)

These are rewritten files in `src/frameworks/media_player/`, covering
only what the binary actually references (RE doc §3d). Anything else
gets a `log!` stub.

| Class | Must support |
|---|---|
| `MPMediaQuery` | `+songsQuery`, `+albumsQuery`, `+artistsQuery`, `+playlistsQuery`, `-addFilterPredicate:`, `-items`, `-collections` (grouped per query type) |
| `MPMediaPropertyPredicate` | `+predicateWithValue:forProperty:comparisonType:` (equal-to on PersistentID, Title, AlbumTitle, Artist; contains is optional) |
| `MPMediaItem` | `-valueForProperty:` for Title, Artist, AlbumTitle, AlbumArtist, Genre, PersistentID (`NSNumber` u64), PlayCount, PlaybackDuration, Artwork |
| `MPMediaItemArtwork` | `-imageWithSize:` → `UIImage` from `art/<id>.png`, or nil |
| `MPMediaItemCollection` | `+collectionWithItems:`, `-items`, `-count`, `-representativeItem`, `-valueForProperty:` |
| `MPMusicPlayerController` | `+applicationMusicPlayer` (singleton), `-setQueueWithItemCollection:`, `-play`, `-stop`, `-volume`/`-setVolume:`, `-setCurrentPlaybackTime:`, `-setRepeatMode:`, `-setShuffleMode:` |

Items are created lazily and cached per ID, and live for the whole run
(the game keeps pointers to items it never retained). *Changed in
implementation:* `-items`/`-collections` build ordinary arrays, so the
first unfiltered `songsQuery` does make one item per song; the game walks
the whole library for play counts anyway, and an item is a few bytes. `MPMediaPickerController` stays a minimal generic class for other
apps; Song Summoner never uses it.

### 3.6 Guest method override hook

`src/objc/app_overrides.rs` (new). After `register_bin_classes`
(`src/objc/classes.rs:617`), apply a static table of
`(class, selector, &'static dyn HostIMP)` entries. For each, if the app
defines that class, it does
`ClassHostObject.methods.insert(sel, IMP::Host(imp))`. Entries for
classes the app doesn't define are skipped, so this is inert for every
other app. The table lives next to the feature
(`frameworks/song_summoner/picker_hook.rs` exports `OVERRIDES`).

### 3.7 Picker hook (the game contract)

Overrides on `IPDMediaPickerController` (inherited by `iPodView2`):

| Selector | Implementation |
|---|---|
| `initWithSelectionIndex:` | Create `SSPickerView` (480×320, tag `0x123`), remember the tab, store the view in host state keyed by `this`. Return `this`. |
| `getView` | That view. `-[ViewManager animationDidStop]` adds it to the window. |
| `setHidden:` / `hidden` | Forward to the view |
| `rotate:` | No-op (the game is landscape-only; the argument is the angle) |
| `selectedTabIndex` | Current tab, 0–3. The game saves it and passes it back next time. |
| `dealloc` | Stop the preview, release the view, drop the host state, then call super |

Also override `-[ViewManager start_iPodView]` to call
`[self animationDidStop]` directly. That skips the game's threaded
`IPDMediaPickerLoadingView`, which only filled caches used by the old
IPD tables.

**Outputs, and nothing else:**
- `[ipodview mediaPicker:ipodview didPickMediaNumber:@(id)]` on a song tap
- `[ipodview mediaPickerDidCancel:ipodview]` on Cancel

The game's state machine does all the hiding, re-showing and closing
(RE doc §2). There are no statics, no run-loop hooks, no guest memory
writes and no touch-coordinate guessing.

### 3.8 Picker UI spec (matches the mockup)

`frameworks/song_summoner/picker_view.rs`: host `UIView` subclasses
using touchHLE UIKit drawing (`CGContext`, `UIImage drawAtPoint:`,
`NSString drawInRect:withFont:`). Art is drawn at integer positions and
native size, so nothing is resampled.

**Layout (points = pixels at native size)**

| Element | Rect | Art / style |
|---|---|---|
| Nav bar | 0,0 – 480×44 | Vertical gradient #46464A→#08080A, cyan rule at y=42, black at y=43. Title LiberationSans Bold 19 centred (ellipsized to 250). **Cancel** button right: rounded 5, fill #1C1C20, stroke #5F5F69, Bold 12. **‹ Back** button left when drilled in. |
| List | 0,44 – 480×220 | Exactly 4 rows of 55. Scrolls vertically. |
| Section header | 480×22 | Gradient #3A404A→#242830, Bold 14 letter at x=12. |
| Song row | 480×55 | `cellbg.png`, or `touchBG.png` while touched. Title Bold 16 white at (15, 9); subtitle Regular 13 #969696 at (15, 31); both ellipsized to x=292. `list_fighter%03d.png` at x=262. |
| Row with artwork (drill-in, albums) | 480×55 | Artwork 50×50 at (4, 2): `noartwork.png`, or the song's art. Text shifts to x=62. |
| Group row (artist/album/playlist) | 480×55 | Name Bold 16 at (15, 17); "N songs  ›" Regular 13 right-aligned at x=450. |
| A–Z index strip | 464,46 – 14×216 | Rounded 7, black at 50% alpha; Bold 7 letters; the current section is cyan #5AC8FF. Touch or drag to jump. |
| Tab bar | 0,264 – 480×56 | Gradient #26262A→#000. Four 120-wide tabs with the native 40×40 `song/artist/album/playlist.png` at y+2 and a Bold 10 label at y+42. Selected: cyan-tinted icon + label on a rounded #3C3C42 plate. Unselected: icon at 45% alpha. |
| Loading overlay | full | Black at 67% over the list; `cube_anm_00..09.png` (native 48×48) animated at 12 fps; "Reading your music library…" Bold 14 cyan; count Regular 12 #AAAAAA. |

**Behaviour**
- **Scrolling:** drag with momentum and a rubber-band at the edges. A tap
  is a touch that moves less than 8 px. The row highlights on touch-down
  and fires on touch-up inside the row.
- **Rows:** tapping a song picks it (see §3.7). Tapping a group row
  pushes its song list, with a slide animation of about 0.25 s. **‹ Back**
  pops it.
- **Tabs:** switching tabs resets that tab's list to the top-level view
  but remembers its scroll position.
- **Preview (optional, Windows first):** the first tap highlights and
  plays a 30 s preview through `media::playback`; a second tap picks. Off
  by default. Decide after playtesting. *Not implemented*: a tap picks.
- **Performance:** only visible rows are drawn. Each row is a small
  cached layer, so scrolling moves layers instead of redrawing text.
  *As implemented:* the whole picker is composed in Rust into one 480×320
  bitmap (`picker_render.rs`) from cached row, bar and index-strip bitmaps,
  and drawn with one `CGContextDrawImage`. Animation runs off an `NSTimer`
  that only exists while something moves.
- **Missing art:** if an art file isn't in the IPA (another version of
  the game), fall back to flat colours of the same layout rather than
  failing.

---

## Part 4: Deletion list (old code to remove)

The media layer is removed and rebuilt. Of the files touched by the old
picker, only generic touchHLE code survives, restored to its upstream
behaviour.

| File | Remove |
|---|---|
| `src/frameworks/media_player/*` | All of it. Rewritten per §3.5. |
| `src/frameworks/uikit/ui_view/ui_table_view.rs` | Promoted table, `PROMOTED_TABLE`, `IPD_SONGS_TABLE`, `PENDING_PICKER_SWAP`, remount/dismiss/drain, `invoke_ipodview_reset`, scene/MainLoop observers, the HUD re-skin. Restore the upstream touchHLE file. |
| `src/frameworks/uikit/ui_touch.rs` | Confirmation-panel button classifier and `stop_song_preview` calls |
| `src/frameworks/foundation/ns_run_loop.rs` | Picker drain and scene observer hooks |
| `src/frameworks/opengles/gles_guest.rs` | Texture tracker |
| `src/frameworks/uikit/ui_bar_button_item.rs` | Picker-specific comments and behaviour (check against upstream) |
| `src/frameworks/foundation/ns_thread.rs` | Review the "Song Summoner waits on the worker thread" change. Keep it only if it fixes a generic bug. |
| `android/.../SetupActivity.kt` | Old music step (auto-detect, path resolution) |
| Repo root `.gitignore` | `touchHLE_music_library*` entries (replace with `library/`) |
| Docs | `dev-docs/song-summoner-picker.md`, `inspect/RE_FINDINGS.md`, and old inspect scripts superseded by `ssdis.py`/`xref.py` |

Cargo: drop `rodio`. Keep `lofty` and `rfd` (Windows scanner). Add
`jni` for Android only. (*In implementation:* `lofty` moved to the
desktop-only dependencies; `flate2` added for the cover-art PNG cache.
The now-unused `album_placeholder.png` was removed from `res/` and the APK
assets.)

---

## Part 5: Milestones

Each milestone ends in a state the game can run in. **Commands are run
by a human** (repo rule), from the repo root unless stated.

### M0: Clean slate
- Remove everything in Part 4. The MediaPlayer classes return an empty
  library.
- ✅ The game boots and the palace **hides** the iPod option (0 songs).
  Both builds are warning-free.

### M1: Library core + index reader
- `src/media/index.rs` with unit tests: parse, escape, sort, sections,
  grouping.
- A hand-written `index.tsv` fixture in `tests/`.
- ✅ `cargo test` passes.

```bash
cargo test media::
```

### M2: MediaPlayer classes on top of the index
- §3.5, without playback.
- ✅ Copy a fixture `library/` folder into `dist\windows\`, then:
  - the iPod option appears;
  - the game's **original** IPD picker (still active until M4) lists the
    fixture songs;
  - picking one shows the correct title/artist on the game's confirm
    panel.

The game's own picker is the test harness for the query layer.

### M3: Scanners
- Windows scanner (§3.3), then Android SAF scanner (§3.2) and the
  `openFd` JNI helper.
- ✅ Windows: first run asks for a folder; `library\index.tsv` appears;
  a second run is fast.
- ✅ Android: first run shows the tree picker and progress;
  `/sdcard/SongSummoner/library/index.tsv` exists.

```bash
cargo debug-windows
```

From `android/`:

```bash
.\gradlew.bat :app:assembleSongsummonerDebug
```

```bash
adb install -r ..\debug\song-summoner-0.2.3.apk
```

```bash
adb shell "ls -l /sdcard/SongSummoner/library"
```

### M4: Override hook + picker
- §3.6, §3.7, then §3.8 in order: Song tab → drill-in → Artist/Album →
  Playlist → index strip → loading overlay.
- ✅ The flow checklist below passes on both platforms.

### M5: Playback + play counts
- §3.4 playback, `MPMusicPlayerController`, `playcount.rs`.
- ✅ The picked song plays in the palace preview, in battle and on the
  results screen. Play counts increase and listening points change.

### M6: Docs + tidy
- Update the README/CLAUDE.md sections on music. Delete the old docs.
- Move open decisions into this doc's "Decisions" table.

### Flow checklist (M4 and later)
1. Palace → iPod → the picker appears on the Song tab (or last used tab).
2. Scroll, fling, use the A–Z jump, switch tabs, drill into an artist and
   come back.
3. Pick → the picker disappears → the game panel shows the right song
   (title, artist, art).
4. **No** → the same picker returns at the same scroll position. Pick a
   *different* song → the panel shows the new song, with no stacked
   text.
5. **Cancel** → back to the palace menu. Reopen → the picker appears
   again.
6. Create Trooper → the trooper is created. Reopen the picker later →
   everything works.
7. Background/foreground the app while the picker is up (Android). The
   game's resign-active exit is expected; after a restart everything
   still works.

---

## Part 6: Working on it (developer workflow)

### Where to look
- **The game's side of any behaviour:**
  `dev-docs/song-summoner-re.md`, then disassemble:
  ```bash
  python inspect/ssdis.py "-[iPodView2 mediaPicker:didPickMediaNumber:]"
  ```
  `ssdis.py` and `xref.py` run from a folder containing the extracted
  `Payload/`. `inspect/Payload` is gitignored, so extract there.
- **Who calls a function:** `python inspect/xref.py 322c`
- **Picker visuals:** change `inspect/picker_mockup.py` first, agree on
  the look, then mirror the constants in `picker_view.rs`. The layout
  constants are named the same in both.

### Logging
- Prefix all new logs `media:` or `picker:`. Log each contract event once:
  picker open, pick (ID), cancel, hide/show/close, and index load (count
  + ms).
- Android logs:
  ```bash
  adb logcat -s touchHLE SongSummoner
  ```

### Rules that bite
- **Nothing may start an activity once touchHLE runs** (it exits on
  focus loss). Every Android prompt goes in `SetupActivity`.
- **Never commit game art.** The picker loads it from the IPA at run
  time. Mockups go to `debug/`.
- **Keep game-specific code in `frameworks/song_summoner/`.** Generic
  engine changes (the override hook, MediaPlayer classes, playback) stay
  app-agnostic, so other apps keep working.
- **`rustfmt` can't format `objc_classes!`/`msg!`.** Format those by hand
  (`dev-docs/code-style.md`).
