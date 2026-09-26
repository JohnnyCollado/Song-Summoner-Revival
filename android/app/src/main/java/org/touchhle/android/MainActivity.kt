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
import java.io.FileOutputStream
import java.io.IOException

// A wrapper class over SDLActivity that points touchHLE at its public
// user-data folder and, in the wrapper flavor, at the game IPA.
//
// All interactive setup (storage permission, music-folder picker) happens
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

        private const val ALBUM_PLACEHOLDER = "album_placeholder.png"
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
        // Extract the album-art placeholder bundled in assets to the
        // user-data folder so the Rust runtime can fs::read it. Safe to
        // call every launch -- it's a no-op when the file is already
        // present at the right size.
        ensureAlbumPlaceholderUnpacked()
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

    // Copy assets/album_placeholder.png into the user-data folder under
    // res/album_placeholder.png so touchHLE's Rust loader (which does a
    // plain fs::read("res/album_placeholder.png") keyed off
    // user_data_base_path on Android) can find it. We write to BOTH
    // candidate locations the Rust paths logic might resolve to:
    //   - <userDataDir>/res/      (preferred; only writable once the
    //                              user grants MANAGE_EXTERNAL_STORAGE)
    //   - getExternalFilesDir/res (always writable; fallback the Rust
    //                              side falls back to as well)
    // That way the file is in place before *or* after the user grants
    // "All files access" -- whichever Rust resolves to, it'll find it.
    // Idempotent: skips the copy when the destination already exists
    // with the expected byte count.
    private fun ensureAlbumPlaceholderUnpacked() {
        val expectedSize: Long = try {
            assets.openFd(ALBUM_PLACEHOLDER).use { it.length }
        } catch (e: IOException) {
            // Asset is markCompressed by default; openFd fails for
            // compressed assets. Fall back to streaming through open()
            // and count bytes ourselves.
            try {
                assets.open(ALBUM_PLACEHOLDER).use { input ->
                    val buf = ByteArray(64 * 1024)
                    var n = 0L
                    while (true) {
                        val r = input.read(buf)
                        if (r <= 0) break
                        n += r
                    }
                    n
                }
            } catch (e2: IOException) {
                Log.w(TAG, "$ALBUM_PLACEHOLDER asset missing: $e2")
                return
            }
        }
        val targets = mutableListOf(File("$userDataDir/res/$ALBUM_PLACEHOLDER"))
        getExternalFilesDir(null)?.let {
            targets.add(File(it, "res/$ALBUM_PLACEHOLDER"))
        }
        for (dest in targets) {
            if (dest.isFile && dest.length() == expectedSize) continue
            dest.parentFile?.mkdirs()
            try {
                assets.open(ALBUM_PLACEHOLDER).use { input ->
                    FileOutputStream(dest).use { out -> input.copyTo(out) }
                }
                Log.i(TAG, "Extracted $ALBUM_PLACEHOLDER -> ${dest.absolutePath}")
            } catch (e: IOException) {
                // Permission-denied on the public user-data dir before
                // MANAGE_EXTERNAL_STORAGE is granted is normal -- the
                // fallback ext-files path will succeed.
                Log.i(TAG, "Could not extract $ALBUM_PLACEHOLDER to " +
                    "${dest.absolutePath}: $e")
            }
        }
    }

    override fun getLibraries(): Array<String> = arrayOf("SDL2", "touchHLE")
}
