# Run me: building Song Summoner Revival

Commands for building Windows and Android, in release and debug. Everything
here is PowerShell. Run each command on its own, from the folder named in
its section.

| Build | Command | Output |
|---|---|---|
| Windows release | `cargo dist-windows` | `dist\windows\` |
| Windows debug | `cargo debug-windows` | `debug\windows\` |
| Android Song Summoner release | `.\gradlew.bat :app:assembleSongsummonerRelease` | `dist\song-summoner-<version>.apk` |
| Android Song Summoner debug | `.\gradlew.bat :app:assembleSongsummonerDebug` | `debug\song-summoner-<version>.apk` |
| Android touchHLE release | `.\gradlew.bat :app:assembleTouchhleRelease` | `dist\touchhle-<version>.apk` |
| Android touchHLE debug | `.\gradlew.bat :app:assembleTouchhleDebug` | `debug\touchhle-<version>.apk` |

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

Fetch the submodules (from the repo root):

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

## 2. Windows (run from the repo root)

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

## 3. Android (run from `android\`)

Gradle builds the Rust side itself with `cargo ndk`, so there's no separate
cargo step. Only `arm64-v8a` is built. The Rust core is built in release mode
even for debug APKs, because a debug build of the emulator is too slow.

After every assemble, the APK is copied out of the build tree: release to
`dist\`, debug to `debug\`.

### Song Summoner flavor (`com.sqefam.songsummoner`)

This is the wrapper that launches the game automatically. On first run it
asks for storage access and for the IPA, which lives in
`/sdcard/SongSummoner/`.

Release:

```bash
.\gradlew.bat :app:assembleSongsummonerRelease
```

Debug:

```bash
.\gradlew.bat :app:assembleSongsummonerDebug
```

### touchHLE flavor (`org.touchhle.android`)

This is the plain emulator with its app picker.

Release:

```bash
.\gradlew.bat :app:assembleTouchhleRelease
```

Debug:

```bash
.\gradlew.bat :app:assembleTouchhleDebug
```

### Install on a device (still from `android\`)

Change the version number if `Cargo.toml` has moved on. `-r` keeps the
existing app data.

Release:

```bash
adb install -r ..\dist\song-summoner-0.2.3.apk
```

Debug:

```bash
adb install -r ..\debug\song-summoner-0.2.3.apk
```

For the touchHLE flavor, swap `song-summoner` for `touchhle` in the file
name.

---

## 4. Running on Windows (run from the repo root)

touchHLE loads its resources and writes its log, sandbox (saves) and
music-library files relative to its working folder. These commands stay in
the repo root but set the working folder to the build output, so the root
stays clean. They use your own IPA from `apps\`.

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

## 5. Tests and lint (run from the repo root)

Run all tests:

```bash
cargo test
```

Skip the integration tests that need the bundled test app:

```bash
cargo test -- --skip run_test_app
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

## 6. Device helpers (adb)

These work from any folder. Swap in `org.touchhle.android` and
`/sdcard/touchHLE/` for the touchHLE flavor.

List connected devices:

```bash
adb devices
```

Push your IPA to the device instead of using the in-app import (run from the
folder that holds the IPA):

```bash
adb push "Song Summoner The Unsung Heroes Encore.ipa" /sdcard/SongSummoner/
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

touchHLE also writes a log file next to its user data. Pull it into the
current folder:

```bash
adb pull /sdcard/SongSummoner/touchHLE_log.txt
```

Check which music folder the picker is using:

```bash
adb shell cat /sdcard/SongSummoner/touchHLE_music_library.txt
```

Delete the IPA from the device, e.g. to test the first-run import again:

```bash
adb shell "rm '/sdcard/SongSummoner/Song Summoner The Unsung Heroes Encore.ipa'"
```

Uninstall the app. This removes the app but not `/sdcard/SongSummoner/`, so
saves and the IPA stay:

```bash
adb uninstall com.sqefam.songsummoner
```

---

## Troubleshooting

- **Gradle download fails with a PKIX / certificate error:** add this line
  to `%USERPROFILE%\.gradle\gradle.properties` (not to the repo):
  `systemProp.javax.net.ssl.trustStoreType=Windows-ROOT`
- **Android build fails in the Rust step:** build the library on its own
  (from the repo root) to see the real error:

```bash
cargo ndk -t arm64-v8a build --release
```

- **Clean Android rebuild** (from `android\`):

```bash
.\gradlew.bat clean
```
