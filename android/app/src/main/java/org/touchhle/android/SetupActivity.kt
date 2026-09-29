/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.ActivityNotFoundException
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
import android.os.SystemClock
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.provider.Settings
import android.util.Log
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import android.widget.Toast
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
//   1. Storage access (generic touchhle flavor only): MANAGE_EXTERNAL_STORAGE
//      on Android 11+, or the READ/WRITE_EXTERNAL_STORAGE runtime
//      permissions on Android 6-10. The Song Summoner wrapper needs none:
//      it runs from its own folder (getExternalFilesDir) and keeps its
//      saves in a folder the player picks (step 2b).
//   2. Game IPA (wrapper flavor only): if the IPA isn't in the user-data
//      folder yet, ask the user to pick their copy with the SAF file
//      picker and copy it there under the name MainActivity expects.
//   2b. Save folder (wrapper, required): picked with the SAF folder picker.
//      If it already holds saves (the first release's /sdcard/SongSummoner,
//      or this app's before a reinstall), they're read and copied in; see
//      SaveFolder.kt. Everything else (music folders, controls) is set up
//      in the game's Setup menu, which opens on the first launch.
//   3. Music folders: if none was chosen yet (or the "Change music folder"
//      shortcut was used), open the SAF folder picker, keep a persistable
//      read permission for the tree and write its URI to
//      library/source.txt (one folder per line).
//   4. Music library: read the folders into library/index.tsv
//      (LibraryScanner), with a progress dialog. Later launches only re-read
//      files that changed.
//   5. Start MainActivity and finish.
//
// Song Summoner's Setup menu also restarts the app into this activity for
// jobs it can't do while the game runs (EXTRA_SETUP_ACTION, see
// SetupAction): change the IPA, add a music folder, rescan, open the data
// folder. The job runs in its step above, then the game starts again.
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
        // The system folder browser, for "Open data folder"; its result
        // is ignored.
        private const val REQ_BROWSE_DATA_FOLDER = 2004
        private const val REQ_PICK_SAVE_FOLDER = 2005

        // Set by the "Change music folder" launcher shortcut: ask for the
        // folder again even though one is saved.
        const val EXTRA_CHOOSE_MUSIC_FOLDER =
            "org.touchhle.android.CHOOSE_MUSIC_FOLDER"
        // A job from the Setup menu (SetupAction's names), set by
        // RestartActivity.
        const val EXTRA_SETUP_ACTION = "org.touchhle.android.SETUP_ACTION"
        private const val SHORTCUT_MUSIC_FOLDER = "music-folder"

        // The user-data folder for this flavor. Shared with MainActivity
        // so both agree on where the IPA, options and music library live.
        // The wrapper uses its own app folder, which needs no permission
        // (the Files app shows it as "Song Summoner", see
        // DocumentsProvider.kt); saves are also copied to the save folder.
        internal fun resolveUserDataDir(context: android.content.Context): String {
            if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
                val dir = context.getExternalFilesDir(null) ?: context.filesDir
                return dir.absolutePath
            }
            return DEFAULT_USER_DATA_DIR
        }

        private fun needsRuntimeStoragePermission(): Boolean =
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.M &&
                Build.VERSION.SDK_INT < Build.VERSION_CODES.R
    }

    private class NotAnIpaException : IOException()

    private val userDataDir by lazy { resolveUserDataDir(this) }
    // Shared with the engine: see src/media.rs.
    private val libraryDir by lazy { File(userDataDir, "library") }
    private val musicSourceFile by lazy { File(libraryDir, "source.txt") }

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
    // The Setup menu's job, if the game restarted into us for one.
    private var action: SetupAction? = null
    // The job's own step already ran (picker shown, file manager opened).
    private var actionStarted = false
    // A picked music folder is added to the others rather than replacing
    // them.
    private var addingMusicFolder = false
    // Away in the file manager (open_data_folder); coming back starts the
    // game.
    private var waitingForFileManager = false
    // Away in the browser (open_tip_page); coming back starts the game.
    private var waitingForBrowser = false
    // The save folder was brought up to date this time.
    private var savesCopiedOut = false
    // The next of DataFolder.ATTEMPTS to try, and when the last one
    // started.
    private var folderAttempt = 0
    private var folderOpenedAt = 0L
    // We were paused since the file manager or browser was started. A job
    // started from onCreate still gets its onResume first (before the
    // other app covers us), which isn't the player coming back.
    private var wentAway = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        action = SetupAction.parse(intent.getStringExtra(EXTRA_SETUP_ACTION))
        if (action != null) {
            Log.i(TAG, "setup: job from the Setup menu: $action")
        }
        publishShortcuts()
        advance()
    }

    override fun onResume() {
        super.onResume()
        if (waitingForSettings) {
            waitingForSettings = false
            busy = false
            advance()
        } else if ((waitingForFileManager || waitingForBrowser) && !wentAway) {
            return
        } else if (waitingForFileManager) {
            // Back too soon: that file manager never really showed the
            // folder, so try the next way before starting the game.
            val elapsed = SystemClock.elapsedRealtime() - folderOpenedAt
            if (DataFolder.cameBackTooSoon(elapsed) && openDataFolder()) {
                return
            }
            waitingForFileManager = false
            busy = false
            advance()
        } else if (waitingForBrowser) {
            waitingForBrowser = false
            busy = false
            advance()
        }
    }

    override fun onPause() {
        super.onPause()
        wentAway = true
    }

    // Run the next outstanding setup step, or launch the game.
    private fun advance() {
        if (busy || finished) {
            return
        }

        // Step 1: storage access, for the generic flavor only (see the
        // steps above).
        if (!BuildConfig.WRAPPER_AUTO_LAUNCH && !hasStorageAccess() &&
            !prefs().getBoolean(PREF_DECLINED_STORAGE, false)) {
            busy = true
            showStorageDialog()
            return
        }
        // Whether the user-data folder can be written.
        val dataUsable = BuildConfig.WRAPPER_AUTO_LAUNCH || hasStorageAccess()

        // Step 2: game IPA.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && !isWrapperIpaPresent()) {
            busy = true
            showIpaDialog()
            return
        }
        // The Setup menu's "Change game file": straight to the picker.
        if (action == SetupAction.CHANGE_IPA && !actionStarted && dataUsable) {
            actionStarted = true
            busy = true
            openIpaPicker()
            return
        }

        // Step 2b: the save folder. Required: the game never starts
        // without one.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            val missing = SaveFolder.saved(this, userDataDir) == null
            val changing = action == SetupAction.PICK_SAVE_FOLDER && !actionStarted
            if (missing || changing) {
                if (changing) actionStarted = true
                busy = true
                showSaveFolderDialog(required = missing)
                return
            }
        }

        // Steps 3 and 4 keep their files in the user-data folder; without
        // it the game runs with no music.
        if (dataUsable) {
            // Step 3: music folders. The shortcut replaces them; the
            // Setup menu's "Add music folder" adds one. The wrapper
            // doesn't ask on its own: its Setup menu opens on the Music
            // tab instead.
            val forced = intent.getBooleanExtra(EXTRA_CHOOSE_MUSIC_FOLDER, false)
            val adding = action == SetupAction.ADD_MUSIC
            val firstTime = !BuildConfig.WRAPPER_AUTO_LAUNCH && savedMusicTrees().isEmpty()
            if (!musicFolderAsked && (forced || adding || firstTime)) {
                musicFolderAsked = true
                addingMusicFolder = adding
                busy = true
                openMusicFolderPicker()
                return
            }

            // Step 4: music library (every launch; only changed files are
            // read, so this is also the Setup menu's "Rescan").
            val trees = savedMusicTrees()
            if (!libraryScanned && trees.isNotEmpty()) {
                libraryScanned = true
                busy = true
                scanLibrary(trees)
                return
            }
        }

        // The Setup menu's "Open data folder".
        if (action == SetupAction.OPEN_DATA_FOLDER && !actionStarted) {
            actionStarted = true
            if (openDataFolder()) {
                busy = true
                waitingForFileManager = true
                return
            }
        }

        // The Setup menu's "Buy me a Taco": the browser, then the game
        // again when the player comes back.
        if (action == SetupAction.OPEN_TIP_PAGE && !actionStarted) {
            actionStarted = true
            try {
                wentAway = false
                startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(TipPage.URL)))
                busy = true
                waitingForBrowser = true
                return
            } catch (e: ActivityNotFoundException) {
                Log.w(TAG, "setup: no browser for the tip page", e)
                Toast.makeText(this, "No web browser found", Toast.LENGTH_LONG).show()
            }
        }

        // Bring the save folder up to date (in case the game didn't get to
        // it before it last quit), then start.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && !savesCopiedOut) {
            savesCopiedOut = true
            val tree = SaveFolder.saved(this, userDataDir)
            if (tree != null) {
                busy = true
                Thread({
                    SaveFolder(this, tree).mirrorFrom(File(userDataDir), includeZips = false)
                    runOnUiThread {
                        busy = false
                        advance()
                    }
                }, "touchHLE-save-copy").start()
                return
            }
        }

        launchGame()
    }

    // Show the data folder (saves, log, library) with the next way in
    // DataFolder.ATTEMPTS that starts. onResume comes back here if it
    // returned too soon to have really shown. Returns false once every way
    // has failed.
    private fun openDataFolder(): Boolean {
        // The wrapper shows the save folder: the one the player knows.
        val tree = if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            SaveFolder.saved(this, userDataDir)
        } else {
            null
        }
        val uri = if (tree != null) {
            DocumentsContract.buildDocumentUriUsingTree(
                tree, DocumentsContract.getTreeDocumentId(tree)
            )
        } else {
            DocumentsContract.buildDocumentUri(
                DataFolder.PROVIDER, DataFolder.documentId(userDataDir)
            )
        }
        while (folderAttempt < DataFolder.ATTEMPTS.size) {
            val attempt = DataFolder.ATTEMPTS[folderAttempt++]
            val view = Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            try {
                when (attempt) {
                    DataFolder.Attempt.FILES_GOOGLE ->
                        startActivity(view.setPackage("com.google.android.documentsui"))
                    DataFolder.Attempt.FILES_AOSP ->
                        startActivity(view.setPackage("com.android.documentsui"))
                    DataFolder.Attempt.ANY_VIEWER -> startActivity(view)
                    DataFolder.Attempt.FOLDER_BROWSER -> {
                        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                            continue
                        }
                        val browse = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                            .putExtra(DocumentsContract.EXTRA_INITIAL_URI, uri)
                        startActivityForResult(browse, REQ_BROWSE_DATA_FOLDER)
                    }
                }
                folderOpenedAt = SystemClock.elapsedRealtime()
                wentAway = false
                Log.i(TAG, "setup: showing the data folder with $attempt")
                return true
            } catch (e: Exception) {
                Log.w(TAG, "setup: $attempt couldn't show $uri: $e")
            }
        }
        android.widget.Toast.makeText(
            this, "Your data folder is $userDataDir", android.widget.Toast.LENGTH_LONG
        ).show()
        return false
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
                "game's .ipa file. Select it and it will be copied into " +
                "the app. If you played an earlier version, it's in your " +
                "old SongSummoner folder.")
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
        // A zip, but is there an app in it? Checked before the old file
        // goes, so a wrong pick changes nothing.
        if (!hasAppInside(part)) {
            part.delete()
            return "That .ipa has no app inside, so it isn't the game."
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

    // Whether an .ipa has an app bundle in Payload/, as the engine checks
    // (check_game_file in src/frameworks/song_summoner/setup.rs).
    private fun hasAppInside(ipa: File): Boolean {
        return try {
            java.util.zip.ZipFile(ipa).use { zip ->
                zip.entries().asSequence().any {
                    it.name.startsWith("Payload/") && it.name.contains(".app/")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't read $ipa as a zip: $e")
            false
        }
    }

    private fun showIpaError(message: String) {
        val builder = AlertDialog.Builder(this)
            .setTitle("Couldn't import game")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Try again") { _, _ -> openIpaPicker() }
        if (isWrapperIpaPresent()) {
            // Changing the game file from the Setup menu: the old one is
            // still there and fine.
            builder.setNegativeButton("Keep the current one") { _, _ ->
                busy = false
                advance()
            }
        } else {
            builder.setNegativeButton("Exit") { _, _ -> skipOrExit() }
        }
        builder.show()
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
    // Step 2b: save folder (wrapper flavor)
    // ---------------------------------------------------------------------

    private fun showSaveFolderDialog(required: Boolean) {
        val builder = AlertDialog.Builder(this)
            .setTitle("Choose a save folder")
            .setMessage("${BuildConfig.APP_NAME} keeps your saves, settings, " +
                "backups and bug reports in a folder you choose, so they stay " +
                "safe even if the app is removed.\n\n" +
                "Pick or create a folder, for example Documents/SongSummoner. " +
                "If you played an earlier version, pick your old " +
                "SongSummoner folder to bring your saves along.")
            .setCancelable(false)
            .setPositiveButton("Choose folder") { _, _ -> openSaveFolderPicker() }
        if (required) {
            builder.setNegativeButton("Exit") { _, _ -> exitApp() }
        } else {
            builder.setNegativeButton("Keep the current one") { _, _ ->
                busy = false
                advance()
            }
        }
        builder.show()
    }

    private fun openSaveFolderPicker() {
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
            intent.addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            )
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                intent.putExtra(
                    DocumentsContract.EXTRA_INITIAL_URI,
                    Uri.parse(
                        "content://com.android.externalstorage.documents" +
                            "/document/primary%3ADocuments"
                    )
                )
            }
            startActivityForResult(intent, REQ_PICK_SAVE_FOLDER)
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't launch the save folder picker: $e")
            showSaveFolderError("This device has no folder picker.")
        }
    }

    private fun showSaveFolderError(message: String) {
        AlertDialog.Builder(this)
            .setTitle("Save folder")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Try again") { _, _ -> openSaveFolderPicker() }
            .setNegativeButton("Exit") { _, _ -> exitApp() }
            .show()
    }

    private fun progressDialog(title: String, text: String): AlertDialog {
        val bar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal)
        bar.isIndeterminate = true
        val label = TextView(this)
        label.text = text
        val layout = LinearLayout(this)
        layout.orientation = LinearLayout.VERTICAL
        val pad = (24 * resources.displayMetrics.density).toInt()
        layout.setPadding(pad, pad, pad, pad)
        layout.addView(label)
        layout.addView(bar)
        return AlertDialog.Builder(this)
            .setTitle(title)
            .setView(layout)
            .setCancelable(false)
            .show()
    }

    private fun onSaveFolderPicked(tree: Uri) {
        try {
            contentResolver.takePersistableUriPermission(
                tree,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            )
        } catch (e: Exception) {
            Log.e(TAG, "Couldn't keep access to the save folder: $e")
            showSaveFolderError("Android didn't let the app keep using that " +
                "folder. Try another one, such as a folder in Documents.")
            return
        }
        val progress = progressDialog("Save folder", "Looking in the folder…")
        Thread({
            val folder = SaveFolder(this, tree)
            val files = folder.listFiles()
            val folderHasSaves = SaveFiles.hasSaves(files.map { it.path })
            val localHasSaves = localSaveFiles().isNotEmpty()
            runOnUiThread {
                progress.dismiss()
                when {
                    folderHasSaves && localHasSaves -> askWhichSaves(folder, files)
                    folderHasSaves -> bringSavesIn(folder, files, backUpCurrent = false)
                    else -> useSaveFolder(folder, broughtIn = 0)
                }
            }
        }, "touchHLE-save-folder").start()
    }

    // Both the folder and the app have saves: the player chooses. The
    // ones not kept are zipped into backups/ first, so nothing is lost.
    private fun askWhichSaves(folder: SaveFolder, files: List<SaveFolder.Doc>) {
        AlertDialog.Builder(this)
            .setTitle("That folder already has saves")
            .setMessage("Use the saves in that folder, or keep the ones you " +
                "have now? The saves you don't keep are put in a backup " +
                "(backups/ in the folder), and Setup > Data & help > Restore " +
                "saves can bring them back.")
            .setCancelable(false)
            .setPositiveButton("Use the folder's saves") { _, _ ->
                bringSavesIn(folder, files, backUpCurrent = true)
            }
            .setNegativeButton("Keep my current saves") { _, _ ->
                val progress = progressDialog("Save folder", "Backing up the folder's saves…")
                Thread({
                    val backedUp = backUpFolderSaves(folder, files)
                    runOnUiThread {
                        progress.dismiss()
                        // The folder's saves would be overwritten: never
                        // without the backup the dialog promised.
                        if (backedUp) useSaveFolder(folder, broughtIn = 0)
                        else backupFailed()
                    }
                }, "touchHLE-save-folder").start()
            }
            .show()
    }

    // Copy the folder's saves (and settings, play counts, backups) in.
    private fun bringSavesIn(
        folder: SaveFolder,
        files: List<SaveFolder.Doc>,
        backUpCurrent: Boolean,
    ) {
        val progress = progressDialog("Save folder", "Bringing your saves in…")
        Thread({
            val base = File(userDataDir)
            // The local saves are about to be deleted: never without the
            // backup the dialog promised.
            if (backUpCurrent && !backUpLocalSaves()) {
                runOnUiThread {
                    progress.dismiss()
                    backupFailed()
                }
                return@Thread
            }
            // Clear the save folders first, so the result is exactly the
            // folder's saves.
            for (dir in listOf(SaveFiles.DOCUMENTS, SaveFiles.PREFERENCES)) {
                File(base, dir).deleteRecursively()
            }
            val count = folder.importInto(base, files)
            runOnUiThread {
                progress.dismiss()
                useSaveFolder(folder, broughtIn = count)
            }
        }, "touchHLE-save-folder").start()
    }

    // Remember the folder, copy what we have into it, and carry on.
    private fun useSaveFolder(folder: SaveFolder, broughtIn: Int) {
        SaveFolder.remember(userDataDir, folder.tree)
        val progress = progressDialog("Save folder", "Copying your saves to the folder…")
        Thread({
            folder.mirrorFrom(File(userDataDir), includeZips = true)
            runOnUiThread {
                progress.dismiss()
                savesCopiedOut = true
                if (broughtIn > 0) {
                    android.widget.Toast.makeText(
                        this, "Brought back $broughtIn files from your save folder",
                        android.widget.Toast.LENGTH_LONG
                    ).show()
                }
                busy = false
                advance()
            }
        }, "touchHLE-save-folder").start()
    }

    // The app's own save files, relative to the data folder.
    private fun localSaveFiles(): List<File> {
        val base = File(userDataDir)
        return listOf(SaveFiles.DOCUMENTS, SaveFiles.PREFERENCES).flatMap { dir ->
            File(base, dir).walkTopDown().filter { it.isFile }.toList()
        }
    }

    // A backup zip in the app's backups/ (copied to the save folder with
    // everything else), named like the engine's (support.rs), entries
    // relative to the game's sandbox folder.
    // Returns false if it couldn't be made (nothing is left behind then).
    private fun writeBackup(entries: List<Pair<String, () -> java.io.InputStream?>>): Boolean {
        if (entries.isEmpty()) return true
        val dir = File(userDataDir, "backups")
        dir.mkdirs()
        val stamp = java.text.SimpleDateFormat("yyyy-MM-dd_HHmm", java.util.Locale.US)
            .apply { timeZone = java.util.TimeZone.getTimeZone("UTC") }
            .format(java.util.Date())
        var file = File(dir, "saves-$stamp.zip")
        var n = 2
        while (file.exists()) {
            file = File(dir, "saves-${stamp}_$n.zip")
            n++
        }
        try {
            java.util.zip.ZipOutputStream(FileOutputStream(file)).use { zip ->
                for ((name, open) in entries) {
                    val input = open() ?: throw IOException("couldn't open $name")
                    input.use {
                        zip.putNextEntry(java.util.zip.ZipEntry(name))
                        it.copyTo(zip)
                        zip.closeEntry()
                    }
                }
            }
            Log.i(TAG, "save folder: backed up ${entries.size} files to $file")
            return true
        } catch (e: Exception) {
            Log.e(TAG, "save folder: backup failed: $e")
            // A half-written zip would be mirrored out as a real backup.
            file.delete()
            return false
        }
    }

    // A backup failed, so nothing was changed: say so and ask again.
    private fun backupFailed() {
        Toast.makeText(
            this, "Couldn't back up your saves, so nothing was changed",
            Toast.LENGTH_LONG
        ).show()
        busy = false
        advance()
    }

    private val sandboxPrefix = "touchHLE_sandbox/${SaveFiles.BUNDLE}/"

    private fun backUpLocalSaves(): Boolean {
        val base = File(userDataDir)
        return writeBackup(localSaveFiles().map { file ->
            val name = file.relativeTo(base).invariantSeparatorsPath.removePrefix(sandboxPrefix)
            Pair(name) { file.inputStream() as java.io.InputStream? }
        })
    }

    private fun backUpFolderSaves(folder: SaveFolder, files: List<SaveFolder.Doc>): Boolean {
        // -1: the provider didn't say (see SaveFiles.importPlan).
        val saves = files.filter {
            SaveFiles.hasSaves(listOf(it.path)) &&
                SaveFiles.allowed(it.path, it.size.coerceAtLeast(0))
        }
        return writeBackup(saves.map { doc ->
            val uri = DocumentsContract.buildDocumentUriUsingTree(folder.tree, doc.id)
            Pair(doc.path.removePrefix(sandboxPrefix)) {
                contentResolver.openInputStream(uri)
            }
        })
    }

    // ---------------------------------------------------------------------
    // Step 3: music folder
    // ---------------------------------------------------------------------

    // The folders in library/source.txt, one per line.
    private fun savedMusicLines(): List<String> {
        val text = try {
            if (musicSourceFile.isFile) musicSourceFile.readText() else ""
        } catch (e: Exception) {
            Log.w(TAG, "Couldn't read $musicSourceFile: $e")
            ""
        }
        return text.lines().map { it.trim() }.filter { it.isNotEmpty() }.distinct()
    }

    // The saved music folders we still hold a read permission for.
    // Permissions can be revoked, or lost when the app is reinstalled; if
    // none are left, the user is asked again.
    private fun savedMusicTrees(): List<Uri> {
        val granted = contentResolver.persistedUriPermissions
            .filter { it.isReadPermission }
            .map { it.uri }
            .toSet()
        return savedMusicLines().map { Uri.parse(it) }.filter { uri ->
            val ok = uri in granted
            if (!ok) {
                Log.i(TAG, "No longer allowed to read the music folder $uri.")
            }
            ok
        }
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
            val lines = if (addingMusicFolder) {
                savedMusicLines() + tree.toString()
            } else {
                listOf(tree.toString())
            }
            musicSourceFile.writeText(
                lines.distinct().joinToString("\n", postfix = "\n"),
                Charsets.UTF_8
            )
            Log.i(TAG, "Music folders: $lines")
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
                    if (isWrapperIpaPresent()) {
                        // Changing it from the Setup menu: keep the old one.
                        busy = false
                        advance()
                    } else {
                        showIpaDialog()
                    }
                }
            }
            REQ_PICK_SAVE_FOLDER -> {
                if (picked != null) {
                    onSaveFolderPicked(picked)
                } else {
                    Log.i(TAG, "Save folder picker cancelled.")
                    showSaveFolderDialog(
                        required = SaveFolder.saved(this, userDataDir) == null
                    )
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

    private fun scanLibrary(trees: List<Uri>) {
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
                LibraryScanner(this, libraryDir).scan(trees) { done, total ->
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
