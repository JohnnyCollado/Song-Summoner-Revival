@echo off
REM ---------------------------------------------------------------------
REM Install + launch the Song Summoner WRAPPER build on a connected
REM Android device via adb. Force-stops any previous instance, installs
REM the freshly built wrapper APK (.ipa is bundled inside it -- no need
REM to push anything to /sdcard), launches the activity, then tails the
REM relevant log streams.
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
set PKG=org.touchhle.android.songsummoner
set ACT=%PKG%/org.touchhle.android.MainActivity

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
echo [1/5] Stopping any previous instance of %PKG%...
"%ADB%" shell am force-stop %PKG%

echo [2/5] Installing %APK%...
"%ADB%" install -r "%APK%"
if errorlevel 1 (
    echo [ERROR] install failed; see message above. Common causes:
    echo   - signature mismatch ^(uninstall existing wrapper and rerun^)
    echo   - storage full ^(wrapper APK is ~285 MB and unpacks an IPA on first launch^)
    pause
    exit /b 1
)

echo [3/5] Syncing local touchHLE_options.txt to /sdcard/touchHLE/...
REM Wrapper still reads /sdcard/touchHLE/touchHLE_options.txt at launch,
REM so we mirror the Windows-side controller layout here.
"%ADB%" shell mkdir -p /sdcard/touchHLE >nul 2>&1
"%ADB%" push "%~dp0touchHLE_options.txt" /sdcard/touchHLE/touchHLE_options.txt

echo [4/5] Clearing logcat buffer...
"%ADB%" logcat -c

echo [5/5] Launching %ACT% and streaming logs.
echo       Ctrl+C to stop tailing (app keeps running on phone).
echo.
"%ADB%" shell am start -n %ACT% >nul
"%ADB%" logcat -v time *:E "SDL/APP:V" "touchHLE:V" "AndroidRuntime:W" "DEBUG:I" "libc:F"

endlocal
