# Song Summoner: The Unsung Heroes Encore — touchHLE port notes

A fork of [touchHLE](https://github.com/touchHLE/touchHLE) that runs
*Song Summoner: The Unsung Heroes Encore* on Windows and Android, with the
game's troopers made from **your own music**. Everything below is about
that game specifically; the upstream emulator already runs other titles.

You supply your own copy of the game's IPA. It is never included in this
repository, the APK or the Windows build, and neither is any of its art.

**New in v0.2.3:** play the whole game with a controller, an easier-to-see
cursor, the Setup menu on both platforms, and saves that survive an
uninstall on Android. See the
[release notes](dev-docs/releases/v0.2.3.md).

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

Add your music folders in the **Music** tab of the Setup menu (below),
which opens by itself on the first launch. The library is read in the
background (Windows) or on the next start (Android). Later starts only
re-read files that changed.

**Android first launch.** No storage permission is asked. You pick your
copy of the game (.ipa), then a **save folder** (for example
`Documents/SongSummoner`), where your saves, settings, backups and bug
reports are kept so they survive an uninstall. If you played the first
release, pick your old `SongSummoner` folder and your saves come along.
Then the game starts in the Setup menu. To start over with a single music
folder, long-press the app icon and choose **Change music folder** (close
the game first).

Everything the scan produces lives in `library/` inside the game's data
folder (next to `S.S.Encore.exe`, or the app's own folder on Android):
`index.tsv`, `art/`, `playcounts.tsv` and `source.txt` (one music folder
per line).

## The Setup menu

The game opens a **Setup** menu by itself on its first launch. After that:

- **Keyboard:** press **F2** (on Android too, with a USB or Bluetooth
  keyboard).
- **Android:** tap the **gear** in the black bar beside the game.
- **Controller (both):** hold **Select**, then press **Start**.

The game is paused while it's open. L1/R1 switch tabs, Confirm steps into a
tab, Back steps out, and Back on the tab list resumes the game. Touch and
mouse work too: one tap chooses, and a drag scrolls long tabs.

| Tab | What's there |
|---|---|
| Game | Change the game file (.ipa), button icon style, show FPS, fullscreen (Windows), fading gear (Android), cursor look and colour, how to type the shop password |
| Music | Your music folders (add, remove), rescan, scroll speed |
| Controller | Move any action to another button; the left stick always works like the D-pad; stick dead zone |
| Keyboard | Two keys per action, all remappable; F2, F11 and F12 are reserved |
| Data & help | Open the data folder, make a bug-report file, back up and restore saves, button tester |
| Credits | touchHLE, the icon artists, the Square Enix notice, and the tip jar |

Changing the game file or music folders restarts the game after asking
(anything since your last in-game save is lost). The settings are kept in
`song_summoner_settings.txt` in the data folder.

**Where your saves are.** On Windows, everything lives next to
`S.S.Encore.exe`. On Android the game runs from its own folder (shown as
"Song Summoner" in the Files app) and keeps a copy in your save folder, with
the same layout; Setup > Data & help > Open data folder shows it. Saves are in
`touchHLE_sandbox/com.square-enix.SongSummonerEncore/` (`Documents/` and
`Library/Preferences/`), the log is `touchHLE_log.txt`, bug reports go to
`bug-reports/` and save backups to `backups/`. A bug report never contains
the game or your saves.

## Playing with a controller

Every menu, dialog and list, the battle map, the world map and the shop work
with a controller. The D-pad moves between buttons by where they are on
screen. In a Yes/No dialog, left is always **No** and right is always
**Yes**; lists wrap from the bottom back to the top.

- **Battle:** move the tile cursor with the D-pad, Confirm to choose, L1/R1
  to step through your units, L2/R2 to zoom, Start for the battle menu.
- **Cutscenes:** Start skips.
- **Shop password:** use the game's keyboard with the D-pad, or type it on
  a keyboard or your phone's own keyboard (Setup > Game > Shop password).
- **Cursor:** Setup > Game > Cursor makes it Outlined or Bold, in gold,
  white, yellow or sky blue. Every colour sits on a dark edge, so it stands
  out without telling colours apart.

`--confirm-button=south|east` picks which face button confirms on the first
launch; after that, change buttons in Setup > Controller.

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

`S.S.Encore.exe` is unsigned and ships a JIT (dynarmic) that allocates
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

Built on [touchHLE](https://github.com/touchHLE/touchHLE): touchHLE ©
2023–2026 touchHLE project contributors, GPL-3.0 (source files under
MPL-2.0).

Controller button icons: "Button Icons and Controls" by
[Zacksly](https://zacksly.itch.io), licensed under
[CC BY 3.0](http://creativecommons.org/licenses/by/3.0/). The files in
`res/controller_glyphs/` are unmodified; see `CREDITS.txt` there.

Gear icon: "Settings" from Material Icons by Google, Apache License 2.0.

The full text is in [`CREDITS.txt`](CREDITS.txt), which also ships with the
Windows build and the APK, and in the Setup menu's Credits tab.

## Support the developer

Song Summoner Revival is free, and always will be. It's made by one person
in their spare time. If it brings you some joy and you'd like to support my
work as a solo developer, a tip is always appreciated but never required.
Thank you for playing!

[![Buy me a Taco](https://img.buymeacoffee.com/button-api/?text=Buy%20me%20a%20Taco&emoji=%F0%9F%8C%AE&slug=johnnycolli&button_colour=BD5FFF&font_colour=ffffff&font_family=Cookie&outline_colour=000000&coffee_colour=FFDD00)](https://www.buymeacoffee.com/johnnycolli)

buymeacoffee.com/johnnycolli (also in the Setup menu's Credits tab).

## Legal

Song Summoner: The Unsung Heroes Encore © 2009 SQUARE ENIX CO., LTD. All
rights reserved. The game, its name, characters, story, music and artwork
are the property of Square Enix. SQUARE ENIX is a registered trademark of
Square Enix Holdings Co., Ltd.

Song Summoner Revival is an unofficial, non-commercial fan project. It is
not made, approved or endorsed by Square Enix. It contains no part of the
game: you must supply your own legally obtained copy.

If you enjoy Song Summoner, please support Square Enix by buying their
games through their official store and channels.
