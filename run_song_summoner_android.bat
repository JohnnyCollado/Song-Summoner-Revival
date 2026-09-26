@echo off
REM ---------------------------------------------------------------------
REM Install + launch the Song Summoner WRAPPER build on a connected
REM Android device via adb. The wrapper APK no longer bundles the IPA;
REM this script pushes the .ipa AND the options file into
REM /sdcard/SongSummoner/ before launching, so the wrapper finds
REM everything it needs in one folder.
REM
REM Double-click this from F:\ios_emu\. Ctrl+C in the window stops the
REM log tail; the app keeps running on the phone.
REM ---------------------------------------------------------------------

setlocal
cd /d "%~dp0"

REM ---------------------------------------------------------------------
REM Locate adb. Prefer the in-tree platform-tools if you have one;
REM otherwise fall back to the Android SDK install (LOCALAPPDATA).
REM ---------------------------------------------------------------------
set ADB=
if exist "%~dp0platform-tools\adb.exe" set ADB=%~dp0platform-tools\adb.exe
if not defined ADB if exist "%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe" set ADB=%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe
if not defined ADB (
    echo [ERROR] adb.exe not found. Looked in:
    echo   %~dp0platform-tools\adb.exe
    echo   %LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe
    echo Install Android SDK platform-tools or drop them next to this script.
    pause
    exit /b 1
)
echo Using adb: %ADB%

REM ---------------------------------------------------------------------
REM Locate the wrapper APK. Prefer the fresh Gradle output; fall back
REM to the packaged one in dist_wrapper\ (handy if you're running off a
REM machine that hasn't done a fresh build).
REM ---------------------------------------------------------------------
set APK=
if exist "%~dp0android\app\build\outputs\apk\songsummoner\release\app-songsummoner-release.apk" set APK=%~dp0android\app\build\outputs\apk\songsummoner\release\app-songsummoner-release.apk
if not defined APK if exist "%~dp0dist_wrapper\SongSummoner.apk" set APK=%~dp0dist_wrapper\SongSummoner.apk
if not defined APK (
    echo [ERROR] Wrapper APK not found. Looked in:
    echo   %~dp0android\app\build\outputs\apk\songsummoner\release\app-songsummoner-release.apk
    echo   %~dp0dist_wrapper\SongSummoner.apk
    echo Build it first: cd android ^&^& gradlew.bat :app:assembleSongsummonerRelease
    pause
    exit /b 1
)
echo Using APK: %APK%

REM The songsummoner flavor adds ".songsummoner" to the application id,
REM so the final package is "org.touchhle.android.songsummoner". The
REM Java class for the activity lives in the original namespace though,
REM so the component name must spell out the full class path.
REM Launch SetupActivity (the launcher), not MainActivity, so the
REM storage-permission and music-folder setup still runs.
set PKG=org.touchhle.android.songsummoner
set ACT=%PKG%/org.touchhle.android.SetupActivity

REM ---------------------------------------------------------------------
REM Make sure a device is attached and authorized.
REM ---------------------------------------------------------------------
"%ADB%" start-server >nul 2>&1
"%ADB%" wait-for-device
for /f "tokens=1,2" %%a in ('"%ADB%" devices ^| findstr /R "device$ unauthorized$ offline$"') do (
    if "%%b"=="unauthorized" (
        echo [ERROR] Device %%a is unauthorized. Tap "Allow USB debugging" on your phone, then retry.
        pause
        exit /b 1
    )
    if "%%b"=="offline" (
        echo [ERROR] Device %%a is offline. Replug USB and retry.
        pause
        exit /b 1
    )
)

REM ---------------------------------------------------------------------
REM Install, sync options (optional), launch, tail logs.
REM ---------------------------------------------------------------------
echo.
echo [1/6] Stopping any previous instance of %PKG%...
"%ADB%" shell am force-stop %PKG%

echo [2/6] Installing %APK%...
"%ADB%" install -r "%APK%"
if errorlevel 1 (
    echo [ERROR] install failed; see message above. Common causes:
    echo   - signature mismatch ^(uninstall existing wrapper and rerun^)
    echo   - storage full
    pause
    exit /b 1
)

REM ---------------------------------------------------------------------
REM Locate the Song Summoner IPA on the host. Prefer the one in apps/,
REM which is where the rest of the repo expects it.
REM ---------------------------------------------------------------------
set IPA_NAME=Song Summoner The Unsung Heroes Encore.ipa
set IPA=
if exist "%~dp0apps\%IPA_NAME%" set IPA=%~dp0apps\%IPA_NAME%

echo [3/6] Ensuring /sdcard/SongSummoner/ exists on device...
"%ADB%" shell mkdir -p /sdcard/SongSummoner >nul 2>&1

echo [4/6] Syncing IPA + touchHLE_options.txt to /sdcard/SongSummoner/...
if defined IPA (
    REM `adb push --sync` skips the transfer when local and remote mtime
    REM + size already match, so the ~260 MB push only happens once per
    REM modified IPA.
    "%ADB%" push --sync "%IPA%" "/sdcard/SongSummoner/%IPA_NAME%"
) else (
    echo       [WARN] No IPA found at %~dp0apps\%IPA_NAME% -- skipping push.
    echo              Drop the file there ^(or directly into /sdcard/SongSummoner/ on the device^) and rerun.
)
REM Wrapper reads /sdcard/SongSummoner/touchHLE_options.txt at launch,
REM so we mirror the Windows-side controller layout here.
"%ADB%" push "%~dp0touchHLE_options.txt" /sdcard/SongSummoner/touchHLE_options.txt

echo [5/6] Clearing logcat buffer...
"%ADB%" logcat -c

echo [6/6] Launching %ACT% and streaming logs.
echo       Ctrl+C to stop tailing (app keeps running on phone).
echo.
"%ADB%" shell am start -n %ACT% >nul
"%ADB%" logcat -v time *:E "SDL/APP:V" "touchHLE:V" "AndroidRuntime:W" "DEBUG:I" "libc:F"

endlocal
