/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.content.Context
import android.net.Uri
import android.util.Log

// Opens the user's songs for the engine. The music folder is picked with the
// Storage Access Framework, so songs are content:// URIs that only Java can
// open; src/media/source.rs calls openFd over JNI when a song starts
// playing and wraps the descriptor in a Rust File.
//
// This never starts an activity, so it's safe while touchHLE runs (it exits
// the process as soon as it loses focus).
object MusicFiles {
    private const val TAG = "touchHLE"

    @Volatile
    private var appContext: Context? = null

    // Called by MainActivity before SDL starts the engine.
    fun init(context: Context) {
        appContext = context.applicationContext
    }

    // A detached, read-only file descriptor for a song's document URI, or
    // -1 if it can't be opened (file gone, permission revoked...). The
    // caller owns the descriptor and must close it.
    @JvmStatic
    fun openFd(uri: String): Int {
        val context = appContext ?: run {
            Log.e(TAG, "MusicFiles.openFd called before init")
            return -1
        }
        return try {
            context.contentResolver.openFileDescriptor(Uri.parse(uri), "r")
                ?.detachFd() ?: -1
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't open song $uri: $e")
            -1
        }
    }
}
