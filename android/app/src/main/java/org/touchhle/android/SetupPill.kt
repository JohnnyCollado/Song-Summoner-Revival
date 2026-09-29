/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import java.io.File

// The gear button over the game that opens Song Summoner's Setup menu.
//
// It's a plain Android View on top of SDL's layout, never focusable: a
// focus change would reach SDL as the app losing focus, and touchHLE quits
// then. Tapping it only sets a flag the engine reads on its next frame
// (MainActivity.nativeOpenSetupMenu), so its touches never reach the game.
//
// This file holds the parts that are plain logic, so they can be tested
// without a device.
object SetupPill {
    // The game's screen is 480x320, letterboxed to fit.
    const val GAME_ASPECT = 480f / 320f

    // Where the gear goes (left, top), for a screen of width x height
    // pixels and a gear `size` pixels across, `margin` from the edges.
    // Centred in the left black bar when the game is letterboxed and the
    // bar has room; otherwise in the top-left corner, over the game.
    fun position(width: Int, height: Int, size: Int, margin: Int): Pair<Int, Int> {
        val gameWidth = Math.round(height * GAME_ASPECT)
        val bar = (width - gameWidth) / 2
        return if (bar >= size + 2 * margin) {
            Pair((bar - size) / 2, margin)
        } else {
            Pair(margin, margin)
        }
    }

    // Whether the gear fades when untouched: `gear_auto_dim` in the Setup
    // menu's settings file (see src/frameworks/song_summoner/settings.rs).
    // On unless the file says false.
    fun autoDimFromText(settingsText: String?): Boolean {
        if (settingsText == null) return true
        for (raw in settingsText.lines()) {
            val line = raw.trim()
            if (line.startsWith("#")) continue
            val eq = line.indexOf('=')
            if (eq < 0) continue
            if (line.substring(0, eq).trim() == "gear_auto_dim") {
                return line.substring(eq + 1).trim() != "false"
            }
        }
        return true
    }

    fun autoDimFor(userDataDir: String): Boolean {
        val file = File(userDataDir, SETTINGS_FILE)
        val text = try {
            if (file.isFile) file.readText() else null
        } catch (e: Exception) {
            null
        }
        return autoDimFromText(text)
    }

    const val SETTINGS_FILE = "song_summoner_settings.txt"
    // Seconds untouched before the gear fades, and how faint it gets.
    const val DIM_AFTER_MS = 3000L
    const val DIM_ALPHA = 0.3f
}

// Showing the data folder (saves, log, library) in a file manager, for the
// Setup menu's "Open data folder". Phones differ in what answers a "view
// this folder" intent, and some file managers open and close at once, so
// SetupActivity tries these in order and moves on when one comes back too
// soon.
object DataFolder {
    const val PROVIDER = "com.android.externalstorage.documents"

    enum class Attempt {
        // Android's own Files app (DocumentsUI), Google's and AOSP's builds.
        FILES_GOOGLE,
        FILES_AOSP,
        // Whatever takes the folder link (e.g. Samsung's My Files).
        ANY_VIEWER,
        // The system folder picker, opened at the folder: shows its files
        // on every phone (Android 8+). Its result is ignored.
        FOLDER_BROWSER,
    }

    val ATTEMPTS = listOf(
        Attempt.FILES_GOOGLE,
        Attempt.FILES_AOSP,
        Attempt.ANY_VIEWER,
        Attempt.FOLDER_BROWSER,
    )

    // Back in less than this: the file manager never really showed.
    const val TOO_SOON_MS = 1500L

    fun cameBackTooSoon(elapsedMs: Long): Boolean = elapsedMs < TOO_SOON_MS

    // "/sdcard/SongSummoner" -> "primary:SongSummoner".
    fun documentId(userDataDir: String): String {
        val relative = userDataDir
            .removePrefix("/storage/emulated/0")
            .removePrefix("/sdcard")
            .trim('/')
        return "primary:$relative"
    }
}

// What SetupActivity is asked to do when the game restarts into it
// (EXTRA_SETUP_ACTION). The engine sends these names; see
// src/frameworks/song_summoner/setup.rs (apply_change).
enum class SetupAction {
    CHANGE_IPA,
    ADD_MUSIC,
    RESCAN,
    OPEN_DATA_FOLDER,
    // Pick another save folder (see SaveFolder.kt).
    PICK_SAVE_FOLDER,
    // Open the developer's tip page (TipPage) in the browser.
    OPEN_TIP_PAGE,
    // Nothing to do but start the game again (e.g. after asking for a
    // save restore, which the engine does at startup).
    RESTART;

    companion object {
        fun parse(name: String?): SetupAction? = when (name?.trim()) {
            "change_ipa" -> CHANGE_IPA
            "add_music" -> ADD_MUSIC
            "rescan" -> RESCAN
            "open_data_folder" -> OPEN_DATA_FOLDER
            "pick_save_folder" -> PICK_SAVE_FOLDER
            "open_tip_page" -> OPEN_TIP_PAGE
            "restart" -> RESTART
            else -> null
        }
    }
}

// The developer's tip jar, from the Setup menu's Credits tab. A fixed
// address (the engine only names the action), the same as credits.rs
// TIP_LINK.
object TipPage {
    const val URL = "https://www.buymeacoffee.com/johnnycolli"
}
