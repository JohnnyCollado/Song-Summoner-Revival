/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
package org.touchhle.android

import android.content.Intent
import android.os.Bundle
import android.system.Os
import android.util.Log
import org.libsdl.app.SDLActivity
import java.io.File

// A wrapper class over SDLActivity that points touchHLE at its public
// user-data folder and, in the wrapper flavor, at the game IPA.
//
// All interactive setup (storage permission, IPA, music folder) happens
// earlier, in SetupActivity: touchHLE exits the process whenever it loses
// focus, so nothing here may open another activity.
class MainActivity : SDLActivity() {

    companion object {
        private const val TAG = "touchHLE"

        // Env var read by paths.rs::resolve_android_user_data_path to
        // override the default /sdcard/touchHLE location. Set from onCreate
        // before super so the Rust side picks it up on first lookup.
        private const val ENV_USER_DATA_BASE_PATH =
            "TOUCHHLE_USER_DATA_BASE_PATH"
    }

    // Resolved public user-data folder for this flavor.
    private val userDataDir = SetupActivity.resolveUserDataDir()

    // Absolute path to the wrapper IPA on public storage. Resolved once in
    // onCreate (before super, so getArguments can return it as argv[1] when
    // SDL fires up the native thread). Null in the non-wrapper flavor or
    // when the file is missing.
    private var wrapperIpaPath: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        // CRITICAL: do wrapper setup BEFORE super.onCreate(). SDL's
        // SDLActivity.onCreate spins up the native thread which calls
        // getArguments() and starts the Rust main shortly after; if
        // wrapperIpaPath isn't set yet, touchHLE will fall back to the
        // app-picker, and if TOUCHHLE_USER_DATA_BASE_PATH isn't set yet,
        // paths.rs will cache the default /sdcard/touchHLE location and
        // never re-check.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            File(userDataDir).mkdirs()
            try {
                Os.setenv(ENV_USER_DATA_BASE_PATH, userDataDir, true)
                Log.i(TAG, "wrapper: $ENV_USER_DATA_BASE_PATH=$userDataDir")
            } catch (e: Exception) {
                Log.w(TAG, "wrapper: couldn't set $ENV_USER_DATA_BASE_PATH: $e")
            }
            wrapperIpaPath = resolveExternalWrapperIpa()
            if (wrapperIpaPath == null) {
                // Never show the bare touchHLE engine (its app picker) in
                // the wrapper: send the user back through setup, which
                // imports the IPA. SDL only starts its native thread once
                // the activity resumes, so finishing straight after
                // super.onCreate (which Android requires us to call) never
                // starts touchHLE.
                startActivity(Intent(this, SetupActivity::class.java))
                super.onCreate(savedInstanceState)
                finish()
                return
            }
        }
        // The engine opens the user's songs through this (over JNI) once it
        // is running.
        MusicFiles.init(this)
        super.onCreate(savedInstanceState)
    }

    // In wrapper-flavor builds (BuildConfig.WRAPPER_AUTO_LAUNCH = true) with
    // the IPA present at wrapperIpaPath, return that path so touchHLE's Rust
    // main skips the app picker and launches the game directly.
    //
    // In the generic touchhle flavor we return super.getArguments() (an
    // empty array), preserving the "show app picker" behaviour. The wrapper
    // never gets here without its IPA: onCreate bounces it back to
    // SetupActivity instead.
    override fun getArguments(): Array<String> {
        val ipa = wrapperIpaPath
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && ipa != null) {
            Log.i(TAG, "wrapper auto-launch: passing IPA path to SDL_main: $ipa")
            return arrayOf(ipa)
        }
        return super.getArguments()
    }

    // Look for the wrapper's IPA at
    // <WRAPPER_USER_DATA_DIR>/<WRAPPER_IPA_FILENAME> on public storage. The
    // user is expected to have copied it there themselves (we no longer
    // bundle the ~260 MB blob inside the APK).
    //
    // Returns the absolute path on success, or null if the file is missing.
    private fun resolveExternalWrapperIpa(): String? {
        val fname = BuildConfig.WRAPPER_IPA_FILENAME
        if (fname.isEmpty()) {
            return null
        }
        val ipa = File(userDataDir, fname)
        if (ipa.isFile && ipa.length() > 0) {
            Log.i(TAG, "wrapper: using IPA at ${ipa.absolutePath}" +
                " (${ipa.length()} bytes).")
            return ipa.absolutePath
        }
        Log.w(TAG, "wrapper: IPA not found at ${ipa.absolutePath}" +
            " -- returning to setup.")
        return null
    }

    override fun getLibraries(): Array<String> = arrayOf("SDL2", "touchHLE")
}
