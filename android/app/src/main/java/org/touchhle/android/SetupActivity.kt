/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.PackageManager
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.provider.Settings
import android.util.Log
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

// Launcher activity that runs all one-time Android setup BEFORE touchHLE
// starts, then hands off to MainActivity.
//
// This has to be a separate activity: touchHLE exits the whole process as
// soon as it loses focus (uikit.rs's app-will-resign-active handler), so
// any Settings page or SAF picker opened while SDL is running kills the
// game and loses the picker's result. Here, nothing emulated is running
// yet, so we can leave for Settings / the picker and come back normally.
//
// Steps, in order:
//   1. Storage access: MANAGE_EXTERNAL_STORAGE on Android 11+, or the
//      READ/WRITE_EXTERNAL_STORAGE runtime permissions on Android 6-10.
//   2. Game IPA (wrapper flavor only): if the IPA isn't in the user-data
//      folder yet, ask the user to pick their copy with the SAF file
//      picker and copy it there under the name MainActivity expects.
//   3. Music folder: if none was chosen yet (or the "Change music folder"
//      shortcut was used), open the SAF folder picker, keep a persistable
//      read permission for the tree and write its URI to
//      library/source.txt.
//   4. Music library: read the folder into library/index.tsv
//      (LibraryScanner), with a progress dialog. Later launches only re-read
//      files that changed.
//   5. Start MainActivity and finish.
class SetupActivity : Activity() {

    companion object {
        private const val TAG = "touchHLE"
        private const val PREFS_NAME = "touchHLE"
        private const val PREF_DECLINED_STORAGE = "declined_storage_access"

        // Default public user-data folder for the generic touchhle flavor.
        private const val DEFAULT_USER_DATA_DIR = "/sdcard/touchHLE"

        private const val REQ_STORAGE_PERMISSIONS = 2001
        private const val REQ_PICK_MUSIC_FOLDER = 2002
        private const val REQ_PICK_IPA = 2003

        // Set by the "Change music folder" launcher shortcut: ask for the
        // folder again even though one is saved.
        const val EXTRA_CHOOSE_MUSIC_FOLDER =
            "org.touchhle.android.CHOOSE_MUSIC_FOLDER"
        private const val SHORTCUT_MUSIC_FOLDER = "music-folder"

        // Public user-data folder for this flavor. Shared with MainActivity
        // so both agree on where the IPA, options and music library
        // live.
        internal fun resolveUserDataDir(): String {
            if (BuildConfig.WRAPPER_AUTO_LAUNCH &&
                BuildConfig.WRAPPER_USER_DATA_DIR.isNotEmpty()) {
                return BuildConfig.WRAPPER_USER_DATA_DIR
            }
            return DEFAULT_USER_DATA_DIR
        }

        private fun needsRuntimeStoragePermission(): Boolean =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.M &&
                Build.VERSION.SDK_INT < Build.VERSION_CODES.R
    }

    private class NotAnIpaException : IOException()

    private val userDataDir = resolveUserDataDir()
    // Shared with the engine: see src/media.rs.
    private val libraryDir = File(userDataDir, "library")
    private val musicSourceFile = File(libraryDir, "source.txt")

    // True while we're away in the All-files-access Settings page, so
    // onResume knows to re-check and continue.
    private var waitingForSettings = false
    // Guards against re-entering the flow while a dialog, permission
    // request or picker is already outstanding.
    private var busy = false
    private var finished = false
    // The music folder picker was already shown (or skipped) this time.
    private var musicFolderAsked = false
    // The library scan already ran this time.
    private var libraryScanned = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        publishShortcuts()
        advance()
    }

    override fun onResume() {
        super.onResume()
        if (waitingForSettings) {
            waitingForSettings = false
            busy = false
            advance()
        }
    }

    // Run the next outstanding setup step, or launch the game.
    private fun advance() {
        if (busy || finished) {
            return
        }

        // Step 1: storage access.
        if (!hasStorageAccess() &&
            !prefs().getBoolean(PREF_DECLINED_STORAGE, false)) {
            busy = true
            showStorageDialog()
            return
        }

        // Step 2: game IPA. Needs storage access to write into the public
        // user-data folder; without it MainActivity shows its own
        // "drop the IPA into ..." toast instead.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && hasStorageAccess() &&
            !isWrapperIpaPresent()) {
            busy = true
            showIpaDialog()
            return
        }

        // Steps 3 and 4 keep their files in the user-data folder, so they
        // need storage access; without it the game runs with no music.
        if (hasStorageAccess()) {
            // Step 3: music folder.
            val forced = intent.getBooleanExtra(EXTRA_CHOOSE_MUSIC_FOLDER, false)
            if (!musicFolderAsked && (forced || savedMusicTree() == null)) {
                musicFolderAsked = true
                busy = true
                openMusicFolderPicker()
                return
            }

            // Step 4: music library.
            val tree = savedMusicTree()
            if (!libraryScanned && tree != null) {
                libraryScanned = true
                busy = true
                scanLibrary(tree)
                return
            }
        }

        launchGame()
    }

    private fun prefs(): SharedPreferences =
        getSharedPreferences(PREFS_NAME, MODE_PRIVATE)

    // ---------------------------------------------------------------------
    // Step 1: storage access
    // ---------------------------------------------------------------------

    private fun hasStorageAccess(): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            return Environment.isExternalStorageManager()
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            return checkSelfPermission(
                Manifest.permission.READ_EXTERNAL_STORAGE
            ) == PackageManager.PERMISSION_GRANTED &&
                checkSelfPermission(
                    Manifest.permission.WRITE_EXTERNAL_STORAGE
                ) == PackageManager.PERMISSION_GRANTED
        }
        return true
    }

    private fun showStorageDialog() {
        val message = if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            "${BuildConfig.APP_NAME} needs access to your files to load the " +
                "game from $userDataDir and to find your music."
        } else {
            "${BuildConfig.APP_NAME} needs access to your files so your " +
                "apps, settings and music can live in $userDataDir, where " +
                "any file manager can reach them."
        }
        val decline = if (BuildConfig.WRAPPER_AUTO_LAUNCH) "Exit" else "Not now"
        AlertDialog.Builder(this)
            .setTitle("Storage access")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Continue") { _, _ -> requestStorageAccess() }
            .setNegativeButton(decline) { _, _ ->
                // The generic flavor works from its private folder, so
                // remember the choice. The wrapper can't find its IPA
                // without access, so keep asking on every launch.
                if (!BuildConfig.WRAPPER_AUTO_LAUNCH) {
                    prefs().edit()
                        .putBoolean(PREF_DECLINED_STORAGE, true)
                        .apply()
                }
                skipOrExit()
            }
            .show()
    }

    private fun requestStorageAccess() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            waitingForSettings = true
            if (!openAllFilesAccessSettings()) {
                Log.w(TAG, "No Settings page for MANAGE_EXTERNAL_STORAGE.")
                waitingForSettings = false
                skipOrExit()
            }
        } else if (needsRuntimeStoragePermission()) {
            requestPermissions(
                arrayOf(
                    Manifest.permission.READ_EXTERNAL_STORAGE,
                    Manifest.permission.WRITE_EXTERNAL_STORAGE,
                ),
                REQ_STORAGE_PERMISSIONS
            )
        } else {
            busy = false
            advance()
        }
    }

    private fun openAllFilesAccessSettings(): Boolean {
        return try {
            startActivity(Intent(
                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:$packageName")
            ))
            true
        } catch (primary: Exception) {
            Log.w(TAG, "App-specific MANAGE_EXTERNAL_STORAGE page " +
                "unavailable: $primary")
            try {
                startActivity(Intent(
                    Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION
                ))
                true
            } catch (secondary: Exception) {
                Log.w(TAG, "Generic page also unavailable: $secondary")
                false
            }
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQ_STORAGE_PERMISSIONS) {
            return
        }
        if (!hasStorageAccess()) {
            Log.i(TAG, "Storage permission denied.")
            showStorageDialog()
            return
        }
        busy = false
        advance()
    }

    // ---------------------------------------------------------------------
    // Step 2: game IPA (wrapper flavor)
    // ---------------------------------------------------------------------

    private fun wrapperIpaFile(): File =
        File(userDataDir, BuildConfig.WRAPPER_IPA_FILENAME)

    private fun isWrapperIpaPresent(): Boolean {
        val ipa = wrapperIpaFile()
        return ipa.isFile && ipa.length() > 0
    }

    private fun showIpaDialog() {
        AlertDialog.Builder(this)
            .setTitle("Game file needed")
            .setMessage("${BuildConfig.APP_NAME} needs your copy of the " +
                "game's .ipa file. Select it and it will be copied to " +
                "$userDataDir.")
            .setCancelable(false)
            .setPositiveButton("Select file") { _, _ -> openIpaPicker() }
            .setNegativeButton("Exit") { _, _ -> skipOrExit() }
            .show()
    }

    private fun openIpaPicker() {
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            intent.addCategory(Intent.CATEGORY_OPENABLE)
            // .ipa has no registered MIME type; providers report it as
            // application/octet-stream, application/zip or something else
            // entirely, so accept anything and validate the contents.
            intent.type = "*/*"
            startActivityForResult(intent, REQ_PICK_IPA)
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't launch ACTION_OPEN_DOCUMENT: $e")
            showIpaError("Couldn't open the file picker on this device.")
        }
    }

    private fun onIpaPicked(uri: Uri) {
        val name = queryDisplayName(uri)
        if (name != null && !name.lowercase().endsWith(".ipa")) {
            showIpaError("\"$name\" doesn't look like an .ipa file.")
            return
        }
        val size = querySize(uri)

        val bar = ProgressBar(
            this, null, android.R.attr.progressBarStyleHorizontal
        )
        bar.max = 1000
        bar.isIndeterminate = size <= 0
        val label = TextView(this)
        label.text = "Copying game file..."
        val layout = LinearLayout(this)
        layout.orientation = LinearLayout.VERTICAL
        val pad = (24 * resources.displayMetrics.density).toInt()
        layout.setPadding(pad, pad, pad, pad)
        layout.addView(label)
        layout.addView(bar)
        val progress = AlertDialog.Builder(this)
            .setTitle("Importing game")
            .setView(layout)
            .setCancelable(false)
            .show()

        Thread({
            val error = copyIpa(uri, size) { done ->
                runOnUiThread {
                    bar.progress = (done * 1000 / size).toInt()
                    label.text = "Copying game file... ${done shr 20} / " +
                        "${size shr 20} MB"
                }
            }
            runOnUiThread {
                progress.dismiss()
                if (error != null) {
                    showIpaError(error)
                } else {
                    busy = false
                    advance()
                }
            }
        }, "touchHLE-ipa-import").start()
    }

    // Copy the picked document to the wrapper IPA path via a temporary
    // ".part" file, so an interrupted copy never leaves a truncated IPA
    // that MainActivity would try to boot. Returns null on success or a
    // user-facing error message.
    private fun copyIpa(
        uri: Uri,
        size: Long,
        onProgress: (bytesCopied: Long) -> Unit,
    ): String? {
        val dest = wrapperIpaFile()
        val part = File(dest.path + ".part")
        dest.parentFile?.mkdirs()
        try {
            val input = contentResolver.openInputStream(uri)
                ?: throw IOException("the file provider returned no data")
            input.use {
                FileOutputStream(part).use { out ->
                    val buf = ByteArray(1 shl 20)
                    var done = 0L
                    var lastReport = 0L
                    var first = true
                    while (true) {
                        val n = input.read(buf)
                        if (n <= 0) break
                        // An IPA is a zip archive, so it must start with
                        // "PK".
                        if (first) {
                            first = false
                            if (n < 2 || buf[0] != 'P'.code.toByte() ||
                                buf[1] != 'K'.code.toByte()) {
                                throw NotAnIpaException()
                            }
                        }
                        out.write(buf, 0, n)
                        done += n
                        if (size > 0 && done - lastReport >= (4 shl 20)) {
                            lastReport = done
                            onProgress(done)
                        }
                    }
                }
            }
        } catch (e: IOException) {
            part.delete()
            Log.e(TAG, "IPA import failed: $e")
            return if (e is NotAnIpaException) {
                "That file isn't a valid .ipa (it's not a zip archive)."
            } else {
                "Couldn't copy the file: ${e.message}"
            }
        }
        dest.delete()
        if (!part.renameTo(dest)) {
            part.delete()
            return "Couldn't move the copied file into place at ${dest.path}."
        }
        Log.i(TAG, "Imported IPA to ${dest.absolutePath}" +
            " (${dest.length()} bytes).")
        return null
    }

    private fun showIpaError(message: String) {
        AlertDialog.Builder(this)
            .setTitle("Couldn't import game")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Try again") { _, _ -> openIpaPicker() }
            .setNegativeButton("Exit") { _, _ -> skipOrExit() }
            .show()
    }

    private fun queryDisplayName(uri: Uri): String? {
        try {
            contentResolver.query(
                uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null
            )?.use { c ->
                if (c.moveToFirst() && !c.isNull(0)) {
                    return c.getString(0)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't query display name for $uri: $e")
        }
        return null
    }

    private fun querySize(uri: Uri): Long {
        try {
            contentResolver.query(
                uri, arrayOf(OpenableColumns.SIZE), null, null, null
            )?.use { c ->
                if (c.moveToFirst() && !c.isNull(0)) {
                    return c.getLong(0)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't query size for $uri: $e")
        }
        return -1
    }

    // ---------------------------------------------------------------------
    // Step 3: music folder
    // ---------------------------------------------------------------------

    // The saved music folder (library/source.txt), if we still hold a read
    // permission for it. Permissions can be revoked, or lost when the app
    // is reinstalled, in which case the user is asked again.
    private fun savedMusicTree(): Uri? {
        val text = try {
            if (musicSourceFile.isFile) musicSourceFile.readText().trim() else ""
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't read $musicSourceFile: $e")
            ""
        }
        if (text.isEmpty()) {
            return null
        }
        val uri = Uri.parse(text)
        val granted = contentResolver.persistedUriPermissions.any {
            it.uri == uri && it.isReadPermission
        }
        if (!granted) {
            Log.i(TAG, "No longer allowed to read the music folder $uri.")
            return null
        }
        return uri
    }

    private fun openMusicFolderPicker() {
        if (finished) {
            return
        }
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
            intent.addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            )
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val musicHint = Uri.parse(
                    "content://com.android.externalstorage.documents" +
                        "/document/primary%3AMusic"
                )
                intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, musicHint)
            }
            Log.i(TAG, "Asking for the music folder.")
            startActivityForResult(intent, REQ_PICK_MUSIC_FOLDER)
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't launch ACTION_OPEN_DOCUMENT_TREE: $e")
            busy = false
            advance()
        }
    }

    private fun onMusicFolderPicked(tree: Uri) {
        try {
            contentResolver.takePersistableUriPermission(
                tree, Intent.FLAG_GRANT_READ_URI_PERMISSION
            )
            libraryDir.mkdirs()
            musicSourceFile.writeText(tree.toString(), Charsets.UTF_8)
            Log.i(TAG, "Music folder: $tree")
        } catch (e: Exception) {
            Log.e(TAG, "Couldn't keep access to the music folder: $e")
        }
        // A different folder means a different library: read it again.
        libraryScanned = false
    }

    override fun onActivityResult(
        requestCode: Int,
        resultCode: Int,
        data: Intent?,
    ) {
        super.onActivityResult(requestCode, resultCode, data)
        val picked = if (resultCode == RESULT_OK) data?.data else null
        when (requestCode) {
            REQ_PICK_IPA -> {
                if (picked != null) {
                    onIpaPicked(picked)
                } else {
                    Log.i(TAG, "IPA picker cancelled.")
                    showIpaDialog()
                }
            }
            REQ_PICK_MUSIC_FOLDER -> {
                if (picked != null) {
                    onMusicFolderPicked(picked)
                } else {
                    // Keep whatever folder was saved before (if any). With
                    // none, the game runs without music and hides its iPod
                    // option.
                    Log.i(TAG, "Music folder picker cancelled.")
                }
                busy = false
                advance()
            }
        }
    }

    // ---------------------------------------------------------------------
    // Step 4: music library
    // ---------------------------------------------------------------------

    private fun scanLibrary(tree: Uri) {
        val bar = ProgressBar(
            this, null, android.R.attr.progressBarStyleHorizontal
        )
        bar.isIndeterminate = true
        val label = TextView(this)
        label.text = "Looking for songs…"
        val layout = LinearLayout(this)
        layout.orientation = LinearLayout.VERTICAL
        val pad = (24 * resources.displayMetrics.density).toInt()
        layout.setPadding(pad, pad, pad, pad)
        layout.addView(label)
        layout.addView(bar)
        val progress = AlertDialog.Builder(this)
            .setTitle("Music library")
            .setView(layout)
            .setCancelable(false)
            .show()

        Thread({
            var lastUpdate = 0L
            val count = try {
                LibraryScanner(this, libraryDir).scan(tree) { done, total ->
                    val now = System.currentTimeMillis()
                    if (total >= 0 && (done == total || now - lastUpdate >= 100)) {
                        lastUpdate = now
                        runOnUiThread {
                            bar.isIndeterminate = false
                            bar.max = maxOf(total, 1)
                            bar.progress = done
                            label.text = "Reading your music library… " +
                                "%,d / %,d".format(done, total)
                        }
                    }
                }
            } catch (e: Exception) {
                // Most likely the folder was removed. Keep the old index.
                Log.e(TAG, "media: music scan failed: $e")
                -1
            }
            runOnUiThread {
                progress.dismiss()
                Log.i(TAG, "media: library has $count songs")
                busy = false
                advance()
            }
        }, "touchHLE-music-scan").start()
    }

    // Long-pressing the launcher icon offers "Change music folder", which
    // opens this screen with EXTRA_CHOOSE_MUSIC_FOLDER. It's a dynamic
    // shortcut because a static one (res/xml/shortcuts.xml) has to spell out
    // the package id, which differs per flavor.
    private fun publishShortcuts() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N_MR1) {
            return
        }
        try {
            val manager = getSystemService(ShortcutManager::class.java)
                ?: return
            val intent = Intent(this, SetupActivity::class.java)
                .setAction(Intent.ACTION_VIEW)
                .putExtra(EXTRA_CHOOSE_MUSIC_FOLDER, true)
                .addFlags(
                    Intent.FLAG_ACTIVITY_NEW_TASK or
                        Intent.FLAG_ACTIVITY_CLEAR_TASK
                )
            val shortcut = ShortcutInfo.Builder(this, SHORTCUT_MUSIC_FOLDER)
                .setShortLabel("Change music folder")
                .setLongLabel("Change music folder")
                .setIcon(Icon.createWithResource(this, BuildConfig.APP_ICON))
                .setIntent(intent)
                .build()
            manager.dynamicShortcuts = listOf(shortcut)
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't publish launcher shortcuts: $e")
        }
    }

    // ---------------------------------------------------------------------
    // Step 5: hand off to touchHLE
    // ---------------------------------------------------------------------

    // The user declined a setup step. The generic flavor can still run
    // (touchHLE shows its app picker); the wrapper can't do anything useful
    // without storage access and the IPA, and must never expose the bare
    // touchHLE engine, so it closes instead.
    private fun skipOrExit() {
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            exitApp()
        } else {
            launchGame()
        }
    }

    private fun exitApp() {
        finished = true
        finishAndRemoveTask()
    }

    private fun launchGame() {
        if (finished) {
            return
        }
        // Belt and braces: the wrapper only ever starts touchHLE with its
        // IPA in place, so users never land in the engine's app picker.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && !isWrapperIpaPresent()) {
            Log.w(TAG, "wrapper: refusing to start touchHLE without the IPA.")
            exitApp()
            return
        }
        finished = true
        startActivity(Intent(this, MainActivity::class.java))
        finish()
    }
}
