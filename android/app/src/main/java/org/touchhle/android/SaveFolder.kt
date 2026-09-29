/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import java.io.File
import java.io.FileOutputStream

// The save folder: a folder the player picks with the Storage Access
// Framework, where saves, settings, play counts, backups and bug reports
// are kept so they outlive the app. No storage permission is needed.
//
// The engine can only work with real file paths, so it runs from the app's
// own folder (getExternalFilesDir) and this folder holds a copy, laid out
// the same way (the first release's /sdcard/SongSummoner layout):
//   - SetupActivity copies everything into it before each start, and reads
//     saves back from it when it's picked (e.g. the first release's folder,
//     or after a reinstall).
//   - While the game runs, the engine copies changed files over JNI
//     (openForWrite / delete, called from src/frameworks/song_summoner/
//     mirror.rs), and once more as it quits.
// Only the files SaveFiles allows ever move, either way.

// Which files belong in the save folder, and the limits on them. Keep in
// step with src/frameworks/song_summoner/mirror.rs (allowed()).
object SaveFiles {
    const val BUNDLE = "com.square-enix.SongSummonerEncore"
    const val DOCUMENTS = "touchHLE_sandbox/$BUNDLE/Documents"
    const val PREFERENCES = "touchHLE_sandbox/$BUNDLE/Library/Preferences"

    const val MAX_SAVE_BYTES = 16L shl 20
    const val MAX_TEXT_BYTES = 4L shl 20
    const val MAX_ZIP_BYTES = 128L shl 20
    const val MAX_FILES = 5000
    const val MAX_TOTAL_BYTES = 1L shl 30
    // Folders inside Documents/ the game may use.
    private const val MAX_DOCUMENTS_DEPTH = 4

    data class Entry(val path: String, val size: Long)

    // A single path component that can't escape or hide anything.
    private fun safeName(name: String): Boolean =
        name.isNotEmpty() && name.length <= 255 &&
            name != "." && name != ".." && !name.startsWith(".") &&
            name.none { it == '/' || it == '\\' || it == ':' || it.isISOControl() }

    // The size limit for an allowed path, or null if it isn't one.
    private fun limitFor(path: String): Long? {
        val parts = path.split('/')
        if (parts.any { !safeName(it) }) return null
        val docs = DOCUMENTS.split('/')
        val prefs = PREFERENCES.split('/')
        return when {
            parts.size > docs.size && parts.size <= docs.size + MAX_DOCUMENTS_DEPTH &&
                parts.subList(0, docs.size) == docs -> MAX_SAVE_BYTES
            parts.size == prefs.size + 1 && parts.subList(0, prefs.size) == prefs &&
                parts.last().endsWith(".plist") -> MAX_SAVE_BYTES
            path == "song_summoner_settings.txt" ||
                path == "library/playcounts.tsv" ||
                path == "library/source.txt" -> MAX_TEXT_BYTES
            parts.size == 2 && parts[0] == "backups" &&
                parts[1].startsWith("saves-") && parts[1].endsWith(".zip") -> MAX_ZIP_BYTES
            parts.size == 2 && parts[0] == "bug-reports" &&
                parts[1].startsWith("bug-report-") && parts[1].endsWith(".zip") -> MAX_ZIP_BYTES
            else -> null
        }
    }

    fun allowed(path: String, size: Long): Boolean {
        val limit = limitFor(path) ?: return false
        return size in 0..limit
    }

    // The game's own save files are there (not just settings or backups).
    fun hasSaves(paths: List<String>): Boolean = paths.any {
        (it.startsWith("$DOCUMENTS/") || it.startsWith("$PREFERENCES/")) && limitFor(it) != null
    }

    // The files to copy from a folder: the allowed ones, within the
    // overall count and size limits.
    fun importPlan(entries: List<Entry>): List<Entry> {
        val out = ArrayList<Entry>()
        var total = 0L
        for (entry in entries) {
            if (out.size >= MAX_FILES) break
            // -1: the provider didn't say. importInto stops the copy at
            // the limit, so it's safe to try.
            val size = entry.size.coerceAtLeast(0)
            if (!allowed(entry.path, size)) continue
            if (total + size > MAX_TOTAL_BYTES) continue
            total += size
            out.add(entry)
        }
        return out
    }
}

// What the first launch asks for, in order. Only what the engine can't
// start without comes before the game; the rest is in the Setup menu.
object FirstRun {
    enum class Step { PICK_IPA, PICK_SAVE_FOLDER, START }

    fun next(hasIpa: Boolean, hasSaveFolder: Boolean): Step = when {
        !hasIpa -> Step.PICK_IPA
        !hasSaveFolder -> Step.PICK_SAVE_FOLDER
        else -> Step.START
    }
}

class SaveFolder(private val context: Context, val tree: Uri) {

    companion object {
        private const val TAG = "touchHLE"
        // Names the save folder's tree URI, in the data folder. The engine
        // reads it too, to show the folder's name.
        const val POINTER_FILE = "save_folder.txt"
        private const val MIME = "application/octet-stream"
        // How deep and how many entries a listing goes, so a huge folder
        // picked by mistake can't hang setup.
        private const val MAX_DEPTH = 10
        private const val MAX_LISTED = 20000

        // The save folder, if one was picked and we may still read and
        // write it (permissions go when the app is uninstalled).
        fun saved(context: Context, userDataDir: String): Uri? {
            val text = try {
                File(userDataDir, POINTER_FILE).takeIf { it.isFile }?.readText()?.trim()
            } catch (e: Exception) {
                null
            }
            if (text.isNullOrEmpty()) return null
            val uri = Uri.parse(text)
            val ok = context.contentResolver.persistedUriPermissions.any {
                it.uri == uri && it.isReadPermission && it.isWritePermission
            }
            return if (ok) uri else null
        }

        fun remember(userDataDir: String, tree: Uri) {
            File(userDataDir).mkdirs()
            File(userDataDir, POINTER_FILE).writeText(tree.toString() + "\n")
        }

        // For the engine (JNI): the save folder this process copies to.
        @Volatile
        private var current: SaveFolder? = null

        fun init(context: Context, userDataDir: String) {
            current = saved(context, userDataDir)?.let {
                SaveFolder(context.applicationContext, it)
            }
        }

        // JNI: a writable file descriptor for `path` in the save folder
        // (created if needed, emptied), or -1. The engine writes and
        // closes it.
        @JvmStatic
        fun openForWrite(path: String): Int = try {
            current?.openFd(path) ?: -1
        } catch (e: Exception) {
            Log.w(TAG, "save folder: couldn't open $path: $e")
            -1
        }

        // JNI: remove `path` from the save folder (a save the game
        // deleted). True if it's gone.
        @JvmStatic
        fun delete(path: String): Boolean = try {
            current?.deletePath(path) ?: false
        } catch (e: Exception) {
            Log.w(TAG, "save folder: couldn't delete $path: $e")
            false
        }
    }

    data class Doc(val path: String, val id: String, val size: Long)

    private val resolver = context.contentResolver
    private val rootId = DocumentsContract.getTreeDocumentId(tree)
    // Folder path -> document id, found or created so far.
    private val dirIds = HashMap<String, String>().apply { put("", rootId) }

    // Every file in the folder, with paths relative to it.
    fun listFiles(): List<Doc> {
        val out = ArrayList<Doc>()
        val pending = ArrayDeque<Triple<String, String, Int>>()
        pending.add(Triple(rootId, "", 0))
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        while (pending.isNotEmpty() && out.size < MAX_LISTED) {
            val (dirId, dirPath, depth) = pending.removeFirst()
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, dirId)
            try {
                resolver.query(children, projection, null, null, null)?.use { c ->
                    while (c.moveToNext() && out.size < MAX_LISTED) {
                        val id = c.getString(0) ?: continue
                        val name = c.getString(1) ?: continue
                        val path = if (dirPath.isEmpty()) name else "$dirPath/$name"
                        if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                            if (depth < MAX_DEPTH) pending.add(Triple(id, path, depth + 1))
                        } else {
                            out.add(Doc(path, id, if (c.isNull(3)) -1 else c.getLong(3)))
                        }
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "save folder: couldn't list $dirPath: $e")
            }
        }
        return out
    }

    // Copy the folder's allowed files into `base` (the app's data folder),
    // replacing what's there. Returns how many were copied.
    fun importInto(base: File, files: List<Doc>): Int {
        val byPath = files.associateBy { it.path }
        val plan = SaveFiles.importPlan(files.map { SaveFiles.Entry(it.path, it.size) })
        val baseDir = base.canonicalFile
        var copied = 0
        for (entry in plan) {
            val doc = byPath[entry.path] ?: continue
            val dest = File(baseDir, entry.path).canonicalFile
            // Belt and braces: allowed() already refuses anything that
            // could leave the folder.
            if (!dest.path.startsWith(baseDir.path + File.separator)) continue
            dest.parentFile?.mkdirs()
            val tmp = File(dest.path + ".part")
            try {
                val uri = DocumentsContract.buildDocumentUriUsingTree(tree, doc.id)
                resolver.openInputStream(uri)?.use { input ->
                    FileOutputStream(tmp).use { out ->
                        // Never more than the size limit, whatever the
                        // provider said the size was.
                        val buf = ByteArray(64 shl 10)
                        var total = 0L
                        while (true) {
                            val n = input.read(buf)
                            if (n <= 0) break
                            total += n
                            if (!SaveFiles.allowed(entry.path, total)) {
                                throw java.io.IOException("larger than allowed")
                            }
                            out.write(buf, 0, n)
                        }
                    }
                } ?: continue
                dest.delete()
                if (tmp.renameTo(dest)) copied++
            } catch (e: Exception) {
                Log.w(TAG, "save folder: skipped ${entry.path}: $e")
            } finally {
                tmp.delete()
            }
        }
        Log.i(TAG, "save folder: copied $copied of ${plan.size} files in")
        return copied
    }

    // Copy every allowed file from `base` into the folder. Backups and bug
    // reports are only copied with `includeZips` (the engine copies each as
    // it's made, so the start-up pass skips them). Returns how many were
    // copied.
    fun mirrorFrom(base: File, includeZips: Boolean): Int {
        val baseDir = base.canonicalFile
        var copied = 0
        baseDir.walkTopDown().maxDepth(MAX_DEPTH).filter { it.isFile }.forEach { file ->
            val path = file.relativeTo(baseDir).invariantSeparatorsPath
            if (!SaveFiles.allowed(path, file.length())) return@forEach
            if (!includeZips && path.endsWith(".zip")) return@forEach
            try {
                val fd = openFd(path)
                if (fd >= 0) {
                    android.os.ParcelFileDescriptor.adoptFd(fd).use { pfd ->
                        android.os.ParcelFileDescriptor.AutoCloseOutputStream(pfd).use { out ->
                            file.inputStream().use { it.copyTo(out) }
                        }
                    }
                    copied++
                }
            } catch (e: Exception) {
                Log.w(TAG, "save folder: couldn't copy $path out: $e")
            }
        }
        Log.i(TAG, "save folder: copied $copied files out")
        return copied
    }

    // A detached, writable, emptied file descriptor for an allowed path.
    fun openFd(path: String): Int {
        if (!SaveFiles.allowed(path, 0)) return -1
        val slash = path.lastIndexOf('/')
        val dirId = dirId(if (slash < 0) "" else path.substring(0, slash), create = true) ?: return -1
        val name = path.substring(slash + 1)
        val id = childId(dirId, name) ?: DocumentsContract.createDocument(
            resolver, DocumentsContract.buildDocumentUriUsingTree(tree, dirId), MIME, name
        )?.let { DocumentsContract.getDocumentId(it) } ?: return -1
        val uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)
        return resolver.openFileDescriptor(uri, "wt")?.detachFd() ?: -1
    }

    fun deletePath(path: String): Boolean {
        if (!SaveFiles.allowed(path, 0)) return false
        val slash = path.lastIndexOf('/')
        val dirId = dirId(if (slash < 0) "" else path.substring(0, slash), create = false)
            ?: return true
        val id = childId(dirId, path.substring(slash + 1)) ?: return true
        return DocumentsContract.deleteDocument(
            resolver, DocumentsContract.buildDocumentUriUsingTree(tree, id)
        )
    }

    // The document id of a folder inside the save folder, made if asked.
    private fun dirId(dirPath: String, create: Boolean): String? {
        dirIds[dirPath]?.let { return it }
        val slash = dirPath.lastIndexOf('/')
        val parent = dirId(if (slash < 0) "" else dirPath.substring(0, slash), create) ?: return null
        val name = dirPath.substring(slash + 1)
        var id = childId(parent, name)
        if (id == null && create) {
            id = DocumentsContract.createDocument(
                resolver,
                DocumentsContract.buildDocumentUriUsingTree(tree, parent),
                DocumentsContract.Document.MIME_TYPE_DIR,
                name
            )?.let { DocumentsContract.getDocumentId(it) }
        }
        if (id == null) return null
        dirIds[dirPath] = id
        return id
    }

    private fun childId(parentId: String, name: String): String? {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
        resolver.query(
            children,
            arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            ),
            null, null, null
        )?.use { c ->
            while (c.moveToNext()) {
                if (c.getString(1) == name) return c.getString(0)
            }
        }
        return null
    }
}
