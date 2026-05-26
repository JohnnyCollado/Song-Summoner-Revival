@echo off
REM Launch Song Summoner: The Unsung Heroes Encore on touchHLE.
REM
REM Working dir is set to this script's folder so touchHLE's user-data files
REM (touchHLE_options.txt, touchHLE_music_library.txt, run.log, etc.) live
REM next to the IPA instead of in some random shell-cwd.
REM
REM The controller layout lives in touchHLE_options.txt (single source of
REM truth, identical to the Android side). To experiment with mappings,
REM edit that file; no rebuild needed.

setlocal
cd /d "%~dp0"

REM ----------------------------------------------------------------------
REM Environment toggles
REM ----------------------------------------------------------------------
REM Enable the on-screen virtual cursor controlled by the right stick.
set TOUCHHLE_VCURSOR=1

REM set TOUCHHLE_RECHOOSE_MUSIC=1
REM set TOUCHHLE_PC_SAMPLE=1

REM ----------------------------------------------------------------------
REM Launch. All controller/orientation/cursor settings come from
REM touchHLE_options.txt (matched against bundle ID
REM com.square-enix.SongSummonerEncore).
REM ----------------------------------------------------------------------
"C:\Users\johnn\touchHLE_target\release\touchHLE.exe" ^
    "apps\Song Summoner The Unsung Heroes Encore.ipa" > run.log 2>&1

endlocal
pause
