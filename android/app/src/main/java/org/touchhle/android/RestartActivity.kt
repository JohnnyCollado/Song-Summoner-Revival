/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.io.File

// Restarts the app into SetupActivity for a job the Setup menu can't do
// while the game runs (a system picker, a file manager).
//
// It runs in its own process (":restart" in the manifest), started by
// MainActivity.requestSetup just before the engine quits. It waits for the
// game's process to finish quitting (so the game can save on the way
// out), makes sure it's gone, then starts SetupActivity in a fresh main
// process with the job, and ends its own.
class RestartActivity : Activity() {

    companion object {
        private const val TAG = "touchHLE"
        const val EXTRA_ACTION = "org.touchhle.android.RESTART_ACTION"
        const val EXTRA_PID = "org.touchhle.android.RESTART_PID"
        private const val POLL_MS = 100L
        private const val GIVE_UP_MS = 4000L
    }

    private val handler = Handler(Looper.getMainLooper())
    private var waited = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        waitForGame()
    }

    private fun waitForGame() {
        val pid = intent.getIntExtra(EXTRA_PID, -1)
        val alive = pid > 0 && File("/proc/$pid").exists()
        if (alive && waited < GIVE_UP_MS) {
            waited += POLL_MS
            handler.postDelayed({ waitForGame() }, POLL_MS)
            return
        }
        if (alive) {
            Log.w(TAG, "setup: the game didn't quit, stopping it")
            android.os.Process.killProcess(pid)
        }
        val action = intent.getStringExtra(EXTRA_ACTION)
        Log.i(TAG, "setup: starting SetupActivity for $action")
        startActivity(
            Intent(this, SetupActivity::class.java)
                .putExtra(SetupActivity.EXTRA_SETUP_ACTION, action)
                .addFlags(
                    Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK
                )
        )
        finish()
        // This process was only for the restart.
        handler.postDelayed({ android.os.Process.killProcess(android.os.Process.myPid()) }, 300)
    }
}
