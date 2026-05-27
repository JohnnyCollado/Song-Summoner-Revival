# Stage the release-ready Dist/ folder from the latest local builds.
# Idempotent — safe to rerun. Picks up any cursor sprite PNGs you've
# dropped into <project-root>/res/ since the last run.
#
# Usage from project root:
#   powershell -ExecutionPolicy Bypass -File dev-scripts\stage-dist.ps1
#
# Prereqs:
#   - cargo build --release  (touchHLE.exe + touchHLE.dll in CARGO_TARGET_DIR)
#   - android\gradlew :app:assembleSongsummonerRelease  (APK)
# The script does NOT trigger builds itself — it just assembles artifacts.
# If a source artifact is missing, the script reports which one and exits.

$ErrorActionPreference = 'Stop'

$root = (Resolve-Path "$PSScriptRoot\..").Path
$dist = Join-Path $root 'Dist'
$winDir = Join-Path $dist 'SongSummoner-Windows-x86_64'
$winZip = Join-Path $dist 'SongSummoner-Windows-x86_64.zip'
$apkOut = Join-Path $dist 'SongSummoner-Android-arm64.apk'

$cargoTarget = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$srcExe = Join-Path $cargoTarget 'release\touchHLE.exe'
$srcDll = Join-Path $cargoTarget 'release\touchHLE.dll'
$srcApk = Join-Path $root 'android\app\build\outputs\apk\songsummoner\release\app-songsummoner-release.apk'

foreach ($p in $srcExe, $srcDll, $srcApk) {
    if (-not (Test-Path $p)) {
        Write-Host "[ERROR] Source artifact missing: $p" -ForegroundColor Red
        Write-Host "Build it first, then rerun this script." -ForegroundColor Red
        exit 1
    }
}

if (Test-Path $dist) {
    Write-Host "Removing existing Dist\ ..."
    Remove-Item -Recurse -Force $dist
}
New-Item -ItemType Directory -Path $winDir -Force | Out-Null
New-Item -ItemType Directory -Path "$winDir\res" -Force | Out-Null

Write-Host "Copying Windows binaries..."
Copy-Item -Force $srcExe "$winDir\touchHLE.exe"
Copy-Item -Force $srcDll "$winDir\touchHLE.dll"

Write-Host "Copying dylibs, fonts, options..."
Copy-Item -Recurse -Force "$root\touchHLE_dylibs" "$winDir\touchHLE_dylibs"
Copy-Item -Recurse -Force "$root\touchHLE_fonts" "$winDir\touchHLE_fonts"
Copy-Item -Force "$root\touchHLE_default_options.txt" $winDir
Copy-Item -Force "$root\touchHLE_options.txt" $winDir
Copy-Item -Force "$root\OPTIONS_HELP.txt" $winDir

Write-Host "Copying cursor sprites (if any)..."
# Match both naming conventions the loader supports:
#   underscored README aliases : cursor_hand.png / cursor_tap.png / cursor_drag.png
#   capitalised spaced names   : Cursor Pointer.png / Cursor Select.png / Cursor Move.png
$cursorPatterns = @('cursor_hand.png','cursor_tap.png','cursor_drag.png',
                     'Cursor Pointer.png','Cursor Select.png','Cursor Move.png')
$copied = 0
foreach ($name in $cursorPatterns) {
    $src = Join-Path $root "res\$name"
    if (Test-Path $src) {
        Copy-Item -Force $src "$winDir\res\$name"
        Write-Host "  + res\$name"
        $copied++
    }
}
if ($copied -eq 0) {
    Write-Host "  (no cursor PNGs in res\ - build will use legacy black-dot fallback)"
}

Write-Host "Writing run.bat..."
@'
@echo off
REM ----------------------------------------------------------------------
REM Launch Song Summoner: The Unsung Heroes Encore in touchHLE on Windows.
REM
REM Setup:
REM   1. Put your "Song Summoner The Unsung Heroes Encore.ipa" next to
REM      this script (or in the apps\ subfolder).
REM   2. Optional: edit touchHLE_options.txt to remap the controller.
REM   3. Optional: drop cursor sprite PNGs in res\ to replace the
REM      default black-dot cursor. See README.txt for filenames.
REM   4. Double-click this file.
REM ----------------------------------------------------------------------

setlocal
cd /d "%~dp0"

set IPA_NAME=Song Summoner The Unsung Heroes Encore.ipa
set IPA=
if exist "%~dp0%IPA_NAME%" set IPA=%~dp0%IPA_NAME%
if not defined IPA if exist "%~dp0apps\%IPA_NAME%" set IPA=%~dp0apps\%IPA_NAME%
if not defined IPA (
    echo [ERROR] Could not find "%IPA_NAME%".
    echo Drop the file next to this script ^(or into the apps\ subfolder^) and rerun.
    pause
    exit /b 1
)

echo Launching touchHLE with %IPA%...
"%~dp0touchHLE.exe" "%IPA%" > run.log 2>&1
if errorlevel 1 (
    echo touchHLE exited with an error. See run.log for details.
    pause
)
endlocal
'@ | Out-File -FilePath "$winDir\run.bat" -Encoding ASCII

Write-Host "Writing README.html..."
@'
<h1>Song Summoner: The Unsung Heroes Encore on touchHLE</h1>

<p>Hopefully you <a href="https://archive.org"><strong>ARCHIVED</strong></a> your legal backup so you can load the game data. <em>We can&#39;t ship it. We won&#39;t ship it. But somewhere out there, in a certain very large library on the internet, a perfectly lawful personal backup may have wandered onto a shelf where curious people sometimes look.</em> 🤷</p>

<p>This release ships <strong>just the wrapper</strong> — touchHLE with Song Summoner-specific fixes baked in (custom <code>IPDMediaPickerController</code> support so the iPod song-picker actually opens, MPMediaQuery filter-predicate plumbing so each song deterministically maps to a unique Tune Trooper, controller-driven virtual cursor with custom sprites, mouse-style autohide, F11 fullscreen, landscape-left default, and a few dozen UIKit / MediaPlayer stub fills). The IPA is BYO.</p>

<h2>What&#39;s in the box</h2>

<table>
  <thead>
    <tr><th>Download</th><th>Platform</th><th>Package ID</th></tr>
  </thead>
  <tbody>
    <tr><td><code>SongSummoner-Windows-x86_64.zip</code></td><td>Windows 10/11 (x64)</td><td>—</td></tr>
    <tr><td><code>SongSummoner-Android-arm64.apk</code></td><td>Android 5+ ARM64</td><td><code>org.touchhle.android.songsummoner</code></td></tr>
  </tbody>
</table>

<h2>Setup</h2>

<h3>Windows</h3>
<ol>
  <li>Unzip anywhere.</li>
  <li>Drop your <em>ahem</em>-acquired <code>Song Summoner The Unsung Heroes Encore.ipa</code> next to <code>run.bat</code> (or into an <code>apps\</code> subfolder).</li>
  <li>Double-click <code>run.bat</code>.</li>
  <li>On first launch you&#39;ll be prompted to point touchHLE at your music folder — pick anywhere with <code>.mp3</code>/<code>.m4a</code>/<code>.flac</code> files. The path is saved to <code>touchHLE_music_library.txt</code>; delete that file to re-prompt later.</li>
  <li>Press <kbd>F11</kbd> at any time to toggle borderless fullscreen.</li>
</ol>

<h3>Android</h3>
<ol>
  <li><code>adb install SongSummoner-Android-arm64.apk</code> (or sideload it through your file manager).</li>
  <li>Open the app once. It&#39;ll detect missing <strong>&quot;All files access&quot;</strong> and forward you straight to the system settings page — toggle it on for Song Summoner, hit back.</li>
  <li>Drop the IPA in <code>/sdcard/SongSummoner/</code>. From a PC:
    <pre><code>adb push &quot;Song Summoner The Unsung Heroes Encore.ipa&quot; /sdcard/SongSummoner/</code></pre>
  </li>
  <li>Tap Song Summoner again — it now boots straight into the game. A folder picker will pop up on first launch so you can grant access to your music library (typically <code>/sdcard/Music/</code>).</li>
</ol>

<h2>Where your saves live</h2>

<table>
  <thead>
    <tr><th>Build</th><th>Path</th></tr>
  </thead>
  <tbody>
    <tr><td>Windows</td><td><code>touchHLE_sandbox\com.square-enix.SongSummonerEncore\Documents\</code></td></tr>
    <tr><td>Android</td><td><code>/sdcard/SongSummoner/touchHLE_sandbox/com.square-enix.SongSummonerEncore/Documents/</code></td></tr>
  </tbody>
</table>

<p>Flat files, no DRM, no cloud. Copy somewhere safe after every milestone — corruption is rare but real. The settings <code>.plist</code> lives next door in <code>Library/Preferences/</code> if you want to back that up too.</p>

<h2>Controls (game controller recommended)</h2>

<table>
  <thead>
    <tr><th>Input</th><th>Action</th></tr>
  </thead>
  <tbody>
    <tr><td><kbd>F11</kbd> (Windows)</td><td>Toggle borderless fullscreen</td></tr>
    <tr><td>Left stick</td><td>On-screen cursor (mouse-style: holds position when released, auto-hides after 3 seconds idle)</td></tr>
    <tr><td>L3 (click L-stick)</td><td>Tap at the cursor position</td></tr>
    <tr><td>Right stick / D-pad</td><td>Drag region (battle camera, list scroll)</td></tr>
    <tr><td>A</td><td>Confirm / advance dialog / wheel commit</td></tr>
    <tr><td>B</td><td>Back / wheel-cancel</td></tr>
    <tr><td>X</td><td>Skip cutscene</td></tr>
    <tr><td>Y</td><td>Next ally arrow</td></tr>
    <tr><td>LB</td><td>Previous ally arrow</td></tr>
    <tr><td>Start</td><td>Turn order / system pill</td></tr>
  </tbody>
</table>

<p>All mappings live in <code>touchHLE_options.txt</code> — edit that file to remap, no rebuild needed.</p>

<h3>Custom cursor sprites</h3>

<p>The cursor renders three PNGs from <code>res/</code>:</p>

<table>
  <thead>
    <tr><th>File</th><th>Drawn when</th></tr>
  </thead>
  <tbody>
    <tr><td><code>res\Cursor Pointer.png</code></td><td>L3 not held (idle)</td></tr>
    <tr><td><code>res\Cursor Select.png</code></td><td>L3 held, cursor still (tap)</td></tr>
    <tr><td><code>res\Cursor Move.png</code></td><td>L3 held, cursor moving (drag)</td></tr>
  </tbody>
</table>

<p>The loader also accepts the alias names <code>cursor_hand.png</code> / <code>cursor_tap.png</code> / <code>cursor_drag.png</code>. Any missing sprite falls back to a small black dot for that state. Pixel-art (16×16 to 32×32) works best — drawn with nearest-neighbour sampling so it stays crisp. On Android the sprites are baked into the APK at build time.</p>

<h2>Known weirdness</h2>
<ul>
  <li><strong>Tune Trooper generation is gated behind opt-in.</strong> The picker is fully working — songs from your library show up, and each song deterministically yields a Tune Trooper with stats and a unique name. To expose songs to the game, set <code>TOUCHHLE_MUSIC_LIBRARY_LIMIT=N</code> for some N&gt;0 before launching. A non-zero value currently triggers a &quot;Calculating Play Points&quot; hang on certain levels (root cause: a guest-thread <code>@synchronized</code> spin-loop that touchHLE&#39;s no-op <code>+exit</code> can&#39;t unwind); with the env var unset the game takes its &quot;Ain&#39;t no tunes&quot; branch and runs cleanly through every menu and battle.</li>
  <li>The game is intrinsically landscape — the default options pin <code>--landscape-left</code>, so touch coordinates don&#39;t end up rotated 90° wrong.</li>
  <li>Album-art placeholder is generic (real cover-art extraction from <code>.m4a</code> tags isn&#39;t implemented yet).</li>
  <li>The wrapper passes <code>--disable-analog-stick-tilt-controls</code> automatically — Song Summoner doesn&#39;t use the accelerometer, and leaving tilt simulation on would fight the left stick over the same physical input now that it&#39;s wired to the virtual cursor.</li>
</ul>

<h2>About this project</h2>

<p>This is purely a technical exercise — an attempt to see whether older iOS software (specifically a 2009-era ARMv6/iOS-3 title that is no longer sold or supported on any current Apple platform) can be made to run on modern Windows and Android hardware via <a href="https://github.com/touchHLE/touchHLE">touchHLE</a>. Everything published here is the wrapper code and build glue around that emulator; no game assets, code, or data ship with this release.</p>

<p><em>Song Summoner: The Unsung Heroes</em>, <em>Song Summoner: The Unsung Heroes Encore</em>, and all associated characters, artwork, music, story, trademarks, and source code are the intellectual property of <strong>Square Enix Holdings Co., Ltd.</strong> This project is not affiliated with, endorsed by, or supported by Square Enix in any way. No infringement is intended; all rights remain with their respective owners.</p>

<p>If you enjoyed Song Summoner, please support Square Enix by buying their currently-available titles.</p>
'@ | Out-File -FilePath "$dist\README.html" -Encoding utf8

Write-Host "Zipping Windows tree..."
Compress-Archive -Path "$winDir\*" -DestinationPath $winZip -CompressionLevel Optimal -Force

Write-Host "Copying APK..."
Copy-Item -Force $srcApk $apkOut

Write-Host ""
Write-Host "Done." -ForegroundColor Green
Get-ChildItem $dist | Select-Object Name,
    @{n='Size';e={if($_.PSIsContainer){'(dir)'}else{'{0:N2} MB' -f ($_.Length/1MB)}}},
    LastWriteTime | Format-Table -AutoSize
