# Run me: building Song Summoner Revival

Commands for building Windows and Android, in release and debug. Everything
here is PowerShell, and **every command runs from the repo root**, as written.
Run each one on its own.

| Build | Command | Output |
|---|---|---|
| Windows release | `cargo dist-windows` | `dist\windows\` |
| Windows debug | `cargo debug-windows` | `debug\windows\` |
| Android Song Summoner release | `.\android\gradlew.bat -p android :app:assembleSongsummonerRelease` | `dist\song-summoner-<version>.apk` |
| Android Song Summoner debug | `.\android\gradlew.bat -p android :app:assembleSongsummonerDebug` | `debug\song-summoner-<version>.apk` |
| Android touchHLE release | `.\android\gradlew.bat -p android :app:assembleTouchhleRelease` | `dist\touchhle-<version>.apk` |
| Android touchHLE debug | `.\android\gradlew.bat -p android :app:assembleTouchhleDebug` | `debug\touchhle-<version>.apk` |

`<version>` is `[workspace.package] version` in the root `Cargo.toml`
(currently `0.2.3`).

The game IPA is never part of any build. Every user supplies their own copy.

---

## 1. One-time setup

Full details are in [dev-docs/building.md](dev-docs/building.md).

- Install git, the Rust toolchain, CMake and Visual Studio's C/C++ build tools.
- Extract Boost into `vendor\boost` (download it from boost.org).
- For Android, also install Android Studio (SDK + NDK). The NDK version is
  pinned in `android\app\build.gradle.kts`.

Fetch the submodules:

```bash
git submodule update --init
```

Android only, add the Rust target:

```bash
rustup target add aarch64-linux-android
```

Android only, install cargo-ndk (version 3.4.0 or later):

```bash
cargo install cargo-ndk
```

Android only, create `android\local.properties` pointing at your SDK. This
file is machine-specific and gitignored:

```
sdk.dir=C\:\\Users\\<you>\\AppData\\Local\\Android\\Sdk
```

---

## 2. Windows

The helper in `xtask/` runs `cargo build`, then puts `touchHLE.exe` together
with `touchHLE_dylibs\`, `touchHLE_fonts\`, `res\` and
`touchHLE_default_options.txt` into a runnable folder. The folder is never
wiped, so your IPA, saves (`touchHLE_sandbox\`) and `touchHLE_options.txt`
survive a rebuild.

### Release → `dist\windows\`

```bash
cargo dist-windows
```

### Debug → `debug\windows\`

```bash
cargo debug-windows
```

---

## 3. Android

Gradle is pointed at `android\` with `-p`; without it, it looks for a build
in the root and fails. Gradle builds the Rust side itself with `cargo ndk`, so there's no separate
cargo step. Only `arm64-v8a` is built. The Rust core is built in release mode
even for debug APKs, because a debug build of the emulator is too slow.

After every assemble, the APK is copied out of the build tree: release to
`dist\`, debug to `debug\`.

### Song Summoner flavor (`com.sqefam.songsummoner`)

This is the wrapper that launches the game automatically. It asks for no
storage permission. On first run it asks for the IPA, then a save folder
(picked with the system's folder picker), then starts the game. The IPA
and everything touchHLE writes live in the app's own folder,
`/sdcard/Android/data/com.sqefam.songsummoner/files/`; saves, settings,
backups and bug reports are also copied to the save folder.

Release:

```bash
.\android\gradlew.bat -p android :app:assembleSongsummonerRelease
```

Debug:

```bash
.\android\gradlew.bat -p android :app:assembleSongsummonerDebug
```

### touchHLE flavor (`org.touchhle.android`)

This is the plain emulator with its app picker.

Release:

```bash
.\android\gradlew.bat -p android :app:assembleTouchhleRelease
```

Debug:

```bash
.\android\gradlew.bat -p android :app:assembleTouchhleDebug
```

### Install on a device

Change the version number if `Cargo.toml` has moved on. `-r` keeps the
existing app data.

Release:

```bash
adb install -r .\dist\song-summoner-0.2.3.apk
```

Debug:

```bash
adb install -r .\debug\song-summoner-0.2.3.apk
```

For the touchHLE flavor, swap `song-summoner` for `touchhle` in the file
name.

---

## 4. Running on Windows

touchHLE loads its resources and writes its log, sandbox (saves) and
music-library files relative to its working folder. These commands set the
working folder to the build output, so the root stays clean. They use your
own IPA from `apps\`.

Debug:

```bash
Start-Process -NoNewWindow -Wait -FilePath .\debug\windows\touchHLE.exe -WorkingDirectory .\debug\windows -ArgumentList "`"$PWD\apps\Song Summoner The Unsung Heroes Encore.ipa`""
```

Release:

```bash
Start-Process -NoNewWindow -Wait -FilePath .\dist\windows\touchHLE.exe -WorkingDirectory .\dist\windows -ArgumentList "`"$PWD\apps\Song Summoner The Unsung Heroes Encore.ipa`""
```

Debug, with a full Rust backtrace if it crashes:

```bash
$env:RUST_BACKTRACE=1; Start-Process -NoNewWindow -Wait -FilePath .\debug\windows\touchHLE.exe -WorkingDirectory .\debug\windows -ArgumentList "`"$PWD\apps\Song Summoner The Unsung Heroes Encore.ipa`""
```

Avoid `cargo run` and running `touchHLE.exe` directly from the repo root.
Either one clutters the root with runtime files.

---

## 5. Tests and lint

Run all tests:

```bash
cargo test
```

Skip the integration tests that need the bundled test app:

```bash
cargo test -- --skip run_test_app
```

### Music library and picker unit tests

These need no device, no IPA and no music folder. Library index, sorting,
grouping, play counts and cover art (fixture in `tests\fixtures\library\`):

```bash
cargo test --lib media::
```

The Windows scanner on its own: reuse of unchanged files, index order with
parallel reads, a crashing file costing only its own song, and listing a
real temporary folder. A passing run still prints one "malformed tag"
panic; that's the crash test's deliberate panic being caught:

```bash
cargo test --lib scan_windows::
```

The picker: drawing primitives, layout, and which fighter portrait each
song gets:

```bash
cargo test --lib song_summoner::
```

The Android scanner's pure helpers (currently the read-thread count). JVM
only, no device needed:

```bash
.\android\gradlew.bat -p android :app:testSongsummonerDebugUnitTest
```

Format the code. `rustfmt` doesn't understand the `objc_classes!` / `msg!`
macros, so check those by hand afterwards:

```bash
cargo fmt
```

Lint the same way `dev-scripts/lint.sh` and CI do:

```bash
cargo clippy -- --deny warnings
```

---

## 6. Music library scan checks

Each scan ends with one log line that says how many files were read and
where the time went:

```
media: scanned 1388 songs (1388 read, 0 unchanged) in 5592 ms: list 460 ms, read 5027 ms on 8 threads (summed: tags 27480 ms, art 12491 ms), write 101 ms
```

- **clean scan:** delete the index first, so every file is read. This is
  what a first run or a big import costs.
- **incremental scan:** launch again without deleting anything. It should
  say `0 read, N unchanged` and take well under a second. If it reads
  everything again, unchanged files aren't being recognised: that's a bug.
- `tags` and `art` are summed over all read threads, so they can exceed the
  total.
- The index is only written when a scan finishes. Quitting mid-scan throws
  the work away, and the next launch starts clean again.

Reference numbers (OnePlus 8T, 1388 songs): clean 5.6 s, incremental
0.57 s.

### Android

Clean scan: delete the index, launch, and wait until the game is up:

```bash
adb shell "rm -f /sdcard/Android/data/com.sqefam.songsummoner/files/library/index.tsv"
```

```bash
adb shell monkey -p com.sqefam.songsummoner 1
```

Read the result:

```bash
adb logcat -d -s touchHLE | Select-String "media: (found|scanned)"
```

Incremental scan: force-stop, launch again, and read the result the same
way:

```bash
adb shell am force-stop com.sqefam.songsummoner
```

```bash
adb shell monkey -p com.sqefam.songsummoner 1
```

The newest `scanned` line is the latest launch.

### Windows

Clean scan: delete the index, then run the game and leave it running
until the picker's loading screen has gone. If no scan has ever
finished, there's no index yet, and this does nothing:

```bash
Remove-Item .\debug\windows\library\index.tsv -ErrorAction SilentlyContinue
```

```bash
Start-Process -NoNewWindow -Wait -FilePath .\debug\windows\touchHLE.exe -WorkingDirectory .\debug\windows -ArgumentList "`"$PWD\apps\Song Summoner The Unsung Heroes Encore.ipa`""
```

Read the result. The log is rewritten on every launch, so check it before
starting the next run:

```bash
Select-String -Path .\debug\windows\touchHLE_log.txt -Pattern "media: (found|scanned)"
```

Incremental scan: run the same `Start-Process` command again and read the
log the same way.

---

## 7. Device helpers (adb)

Swap in `org.touchhle.android` and
`/sdcard/touchHLE/` for the touchHLE flavor.

List connected devices:

```bash
adb devices
```

Push your IPA (from `apps\`) to the device instead of using the in-app
import:

```bash
adb push ".\apps\Song Summoner The Unsung Heroes Encore.ipa" /sdcard/Android/data/com.sqefam.songsummoner/files/
```

Launch the app:

```bash
adb shell monkey -p com.sqefam.songsummoner 1
```

Force-stop it:

```bash
adb shell am force-stop com.sqefam.songsummoner
```

Clear the logcat buffer before a test run:

```bash
adb logcat -c
```

Dump touchHLE's logcat output only:

```bash
adb logcat -d "touchHLE:V" "*:S"
```

touchHLE also writes a log file next to its user data. Pull it into
`debug\` (gitignored):

```bash
adb pull /sdcard/Android/data/com.sqefam.songsummoner/files/touchHLE_log.txt .\debug\touchHLE_log_android.txt
```

Check which music folder the library is read from (a SAF tree URI):

```bash
adb shell cat /sdcard/Android/data/com.sqefam.songsummoner/files/library/source.txt
```

List the library index and the cover-art cache:

```bash
adb shell "ls -l /sdcard/Android/data/com.sqefam.songsummoner/files/library /sdcard/Android/data/com.sqefam.songsummoner/files/library/art | head -20"
```

Delete the IPA from the device, e.g. to test the first-run import again:

```bash
adb shell "rm '/sdcard/Android/data/com.sqefam.songsummoner/files/Song Summoner The Unsung Heroes Encore.ipa'"
```

Uninstall the app. This deletes the app's own folder, IPA included; only
the copy in the save folder you picked stays:

```bash
adb uninstall com.sqefam.songsummoner
```

---

## Troubleshooting

- **Gradle download fails with a PKIX / certificate error:** add this line
  to `%USERPROFILE%\.gradle\gradle.properties` (not to the repo):
  `systemProp.javax.net.ssl.trustStoreType=Windows-ROOT`
- **Android build fails in the Rust step:** build the library on its own to
  see the real error:

```bash
cargo ndk -t arm64-v8a build --release
```

- **Clean Android rebuild:**

```bash
.\android\gradlew.bat -p android clean
```
