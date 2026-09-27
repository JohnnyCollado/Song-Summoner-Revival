# Song Summoner: The Unsung Heroes Encore — touchHLE port notes

A fork of [touchHLE](https://github.com/touchHLE/touchHLE) that runs
*Song Summoner: The Unsung Heroes Encore* on Windows and Android, with the
game's troopers made from **your own music**. Everything below is about
that game specifically; the upstream emulator already runs other titles.

You supply your own copy of the game's IPA. It is never included in this
repository, the APK or the Windows build, and neither is any of its art.

Design and background:

- [`dev-docs/song-summoner-media-plan.md`](dev-docs/song-summoner-media-plan.md):
  how the music library, player and picker work, and the decisions behind
  them.
- [`dev-docs/song-summoner-re.md`](dev-docs/song-summoner-re.md): the
  game's side (its picker contract, input funnel, addresses).

## Your music

The game builds troopers from songs in the iPhone's music library. Here the
"iPhone music library" is a folder of your own audio files: **mp3, m4a,
aac, wav or caf** (FLAC and Ogg can't be played yet, so they're skipped).
Tags give each song its title, artist and album; embedded cover art is
shown too. Each subfolder also appears as a playlist.

**Windows.** The first time you start the game, a *Select folder* dialog
asks where your music is. The library is read in the background while the
game boots. Later starts only re-read files that changed. To pick another
folder, start with `--choose-music-folder` or delete `library\source.txt`
from the game folder.

**Android.** After the storage permission and the IPA, Android's own folder
picker asks for your music folder (for example `Music/`), then a progress
screen reads the library once. To pick another folder, long-press the app
icon and choose **Change music folder** (close the game first).

Everything the scan produces lives in `library/` inside the game's data
folder (next to `touchHLE.exe`, or `/sdcard/SongSummoner/` on Android):
`index.tsv`, `art/`, `playcounts.tsv` and `source.txt`.

## Picking a song

In the Soul Master's palace, the iPod option opens our picker, drawn in the
style of the game's original one using the game's own art: tabs for
**Song, Artist, Album and Playlist**, an A–Z strip, and the fighter
portrait on each song. Tap a song to pick it; the game shows its own
"Create this trooper?" panel. **No** brings the picker back where it was;
**Cancel** leaves the iPod menu. The trooper's song then plays from your
file in the palace, in battle and on the results screen, and each play
counts towards the game's listening points.

## Heads-up: antivirus false positives on Windows

`touchHLE.exe` is unsigned and ships a JIT (dynarmic) that allocates
executable memory at runtime to translate ARM code — a pattern that
trips heuristic scanners. **Avast, AVG, Bitdefender, occasionally
Windows Defender, and SmartScreen** may flag it or move it to
quarantine on first launch. It's a false positive.

Workarounds: restore the file from your AV's quarantine and add an
exception for the folder you unzipped into, or build touchHLE from
source on your own machine (the locally-built `.exe` generally
won't trip these heuristics).

The Android wrapper APK is signed (with the standard debug key)
and is not affected.

## Where the code is

| Path | What |
|---|---|
| `src/media/` | Host-side library: index format and views, desktop scanner, song files, streaming playback, play counts, cover-art cache. No Objective-C. |
| `src/frameworks/media_player/` | `MPMediaQuery`, `MPMediaItem`, `MPMediaItemCollection`, `MPMediaPropertyPredicate`, `MPMusicPlayerController` on top of `src/media/`. |
| `src/objc/app_overrides.rs` | Generic hook that replaces methods an app defines with host code, only for apps that define those classes. |
| `src/frameworks/song_summoner/` | Everything specific to this game: the picker hook (`IPDMediaPickerController`), the picker view and its drawing. |
| `android/.../SetupActivity.kt`, `LibraryScanner.kt`, `MusicFiles.kt` | Android music folder picker, library scan, and the file-descriptor helper the engine calls over JNI. |
| `tests/fixtures/library/` | Hand-written library index used by the unit tests. |
| `inspect/` | RE tools (`mo.py`, `ssdis.py`, `xref.py`) and `picker_mockup.py`, which renders the picker design from your own IPA into `debug/picker-mockup/`. |

Building: see [`dev-docs/building.md`](dev-docs/building.md).

## Credits

Controller button icons: "Button Icons and Controls" by
[Zacksly](https://zacksly.itch.io), licensed under
[CC BY 3.0](http://creativecommons.org/licenses/by/3.0/). The files in
`res/controller_glyphs/` are unmodified; see `CREDITS.txt` there.
