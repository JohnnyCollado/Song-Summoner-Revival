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
import android.os.Handler
import android.os.Looper
import android.system.Os
import android.util.Log
import android.view.MotionEvent
import android.view.ViewGroup
import android.widget.ImageButton
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

        // The running activity, for requestSetup (called from the engine's
        // thread over JNI).
        @Volatile
        private var instance: MainActivity? = null

        // The gear button was tapped: the engine opens Setup on its next
        // poll. Implemented in src/window.rs.
        @JvmStatic
        external fun nativeOpenSetupMenu()

        // Called by the engine (src/frameworks/song_summoner/setup.rs)
        // when the Setup menu needs a system picker or another app: hand
        // the job to SetupActivity in a fresh process. The engine quits
        // right after this returns, so startActivity is called here, on
        // its thread, rather than posted.
        @JvmStatic
        fun requestSetup(action: String) {
            val activity = instance ?: return
            Log.i(TAG, "setup: restarting into SetupActivity for $action")
            val intent = Intent(activity, RestartActivity::class.java)
                .putExtra(RestartActivity.EXTRA_ACTION, action)
                .putExtra(RestartActivity.EXTRA_PID, android.os.Process.myPid())
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            activity.startActivity(intent)
        }
    }

    private var gear: ImageButton? = null
    private val dimHandler = Handler(Looper.getMainLooper())
    // The setting is read when it's time to fade, not once at the start,
    // so turning it off in the Setup menu takes effect right away.
    private val dimGear = Runnable {
        if (SetupPill.autoDimFor(userDataDir)) {
            gear?.animate()?.alpha(SetupPill.DIM_ALPHA)?.setDuration(400)
        }
    }

    // The user-data folder for this flavor (see SetupActivity.resolveUserDataDir).
    private val userDataDir by lazy { SetupActivity.resolveUserDataDir(this) }

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
        // ...and copies changed saves to the save folder through this.
        SaveFolder.init(this, userDataDir)
        super.onCreate(savedInstanceState)
        instance = this
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            addSetupGear()
        }
    }

    override fun onDestroy() {
        if (instance === this) {
            instance = null
        }
        dimHandler.removeCallbacks(dimGear)
        super.onDestroy()
    }

    // The gear that opens Song Summoner's Setup menu, in the black bar
    // beside the game (see SetupPill). Always there, so the Android build
    // needs no "press F2" reminder.
    private fun addSetupGear() {
        val layout = SDLActivity.mLayout ?: return
        val density = resources.displayMetrics.density
        val size = (44 * density).toInt()
        val margin = (12 * density).toInt()
        val button = ImageButton(this)
        button.setImageResource(R.drawable.ic_setup_gear)
        button.setBackgroundResource(R.drawable.setup_gear_bg)
        button.contentDescription = "Setup"
        button.scaleType = android.widget.ImageView.ScaleType.CENTER_INSIDE
        val pad = (10 * density).toInt()
        button.setPadding(pad, pad, pad, pad)
        // Never take focus: SDL would see the game lose it, and touchHLE
        // quits then.
        button.isFocusable = false
        button.isFocusableInTouchMode = false
        button.setOnClickListener {
            showGear()
            nativeOpenSetupMenu()
        }
        layout.addView(button, ViewGroup.LayoutParams(size, size))
        gear = button
        layout.addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
            val (x, y) = SetupPill.position(v.width, v.height, size, margin)
            button.x = x.toFloat()
            button.y = y.toFloat()
            button.bringToFront()
        }
        showGear()
    }

    // Full strength now, fading again later if the player wants that.
    private fun showGear() {
        val button = gear ?: return
        dimHandler.removeCallbacks(dimGear)
        button.animate().cancel()
        button.alpha = 1f
        dimHandler.postDelayed(dimGear, SetupPill.DIM_AFTER_MS)
    }

    // Any touch brings a faded gear back. The touch still goes to the game.
    override fun dispatchTouchEvent(event: MotionEvent?): Boolean {
        if (event?.actionMasked == MotionEvent.ACTION_DOWN && gear != null) {
            showGear()
        }
        return super.dispatchTouchEvent(event)
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
