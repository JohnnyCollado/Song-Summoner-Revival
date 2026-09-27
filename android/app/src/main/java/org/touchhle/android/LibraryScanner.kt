/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.atomic.AtomicLong

// Reads the music folder the user picked (a Storage Access Framework tree)
// into the library index the engine loads: <user data>/library/index.tsv,
// plus cover art in library/art/<id>.png.
//
// The format is shared with src/media/index.rs (which documents it) and the
// desktop scanner src/media/scan_windows.rs. Keep all three in step.
//
// Runs on a worker thread in SetupActivity, before touchHLE starts. A
// rescan only reads tags for new or changed files, so after the first run
// it takes a second or two. The files that do need reading go to a small
// thread pool: opening a document through SAF is most of the cost, and
// several opens overlap well.
class LibraryScanner(private val context: Context, private val libraryDir: File) {

    companion object {
        private const val TAG = "touchHLE"
        private const val HEADER = "SSLIB\t1"
        private const val COLUMNS = 13
        private const val MAX_ART_SIDE = 256
        // Threads reading tags: one per core, at least 2 and at most 8.
        // Reading is mostly waiting on the storage provider, so even one
        // core overlaps two reads; past 8, concurrent SAF opens contend
        // and nothing is gained. Covers are decoded downscaled (saveArt),
        // so 8 at once is still only a few MB.
        fun readThreads(cores: Int): Int = cores.coerceIn(2, 8)

        // Formats the engine's decoder plays; see AUDIO_EXTENSIONS in
        // src/media.rs. Picked by extension rather than MIME type, because
        // audio/* also covers FLAC and Ogg, which the engine can't play.
        private val AUDIO_EXTENSIONS = setOf("mp3", "m4a", "aac", "wav", "caf")
        // For documents without a usable file name extension.
        private val AUDIO_MIME_TYPES = setOf(
            "audio/mpeg", "audio/mp3", "audio/mp4", "audio/x-m4a", "audio/aac",
            "audio/aacp", "audio/wav", "audio/x-wav", "audio/wave",
            "audio/x-caf",
        )

        // The persistent ID: 64-bit FNV-1a of a stable key (here the SAF
        // document ID), never 0. Same as media::index::persistent_id.
        fun persistentId(key: String): Long {
            var hash = -0x340d631b7bdddcdbL // 0xcbf29ce484222325
            for (b in key.toByteArray(Charsets.UTF_8)) {
                hash = hash xor (b.toLong() and 0xff)
                hash *= 0x100000001b3L
            }
            return if (hash == 0L) 1L else hash
        }

        fun escape(s: String): String {
            val out = StringBuilder(s.length)
            for (c in s) {
                when (c) {
                    '\\' -> out.append("\\\\")
                    '\t' -> out.append("\\t")
                    '\n' -> out.append("\\n")
                    '\r' -> out.append("\\r")
                    else -> out.append(c)
                }
            }
            return out.toString()
        }

        fun unescape(s: String): String {
            val out = StringBuilder(s.length)
            var i = 0
            while (i < s.length) {
                val c = s[i]
                if (c != '\\' || i + 1 >= s.length) {
                    out.append(c)
                    i++
                    continue
                }
                when (val next = s[i + 1]) {
                    '\\' -> out.append('\\')
                    't' -> out.append('\t')
                    'n' -> out.append('\n')
                    'r' -> out.append('\r')
                    else -> out.append('\\').append(next)
                }
                i += 2
            }
            return out.toString()
        }
    }

    data class Song(
        val id: Long,
        val locator: String,
        val mtime: Long,
        val size: Long,
        val durationMs: Long,
        val track: Int,
        val title: String,
        val artist: String,
        val album: String,
        val albumArtist: String,
        val genre: String,
        val folder: String,
        val hasArt: Boolean,
    )

    private class Found(
        val documentId: String,
        val uri: Uri,
        val name: String,
        val folder: String,
        val mtime: Long,
        val size: Long,
    )

    private val indexFile = File(libraryDir, "index.tsv")
    private val artDir = File(libraryDir, "art")

    // Time spent in each part of reading a file, summed over all read
    // threads (so it can exceed the scan's own time).
    private val tagsNs = AtomicLong()
    private val artNs = AtomicLong()

    // Scan the tree and write the index. onProgress(done, total) is called
    // from this (worker) thread; total is -1 while the folders are still
    // being listed. Returns the number of songs.
    fun scan(treeUri: Uri, onProgress: (Int, Int) -> Unit): Int {
        val start = System.nanoTime()
        onProgress(0, -1)
        val found = listAudioFiles(treeUri)
        val listMs = msSince(start)
        Log.i(TAG, "media: found ${found.size} audio files")

        artDir.mkdirs()
        val previous = readIndex()
        tagsNs.set(0)
        artNs.set(0)
        // One slot per song, in index order. Unchanged songs are filled in
        // now; the rest are handed to the pool and collected in order below.
        val slots = ArrayList<Song?>(found.size)
        val jobs = ArrayList<Pair<Int, Future<Song>>>()
        val seen = HashSet<Long>()
        val readStart = System.nanoTime()
        val threads = readThreads(Runtime.getRuntime().availableProcessors())
        val pool = Executors.newFixedThreadPool(threads)
        try {
            for (file in found) {
                val id = persistentId(file.documentId)
                if (!seen.add(id)) continue
                val locator = file.uri.toString()
                val old = previous[id]
                if (old != null && old.mtime == file.mtime &&
                    old.size == file.size && old.locator == locator) {
                    slots.add(old.copy(hasArt = old.hasArt && artFile(id).isFile))
                } else {
                    val job = Callable { readSong(file, id, locator) }
                    jobs.add(slots.size to pool.submit(job))
                    slots.add(null)
                }
            }
            // Waiting in order keeps onProgress on this thread, as
            // SetupActivity expects; the count can lag a few songs behind.
            var done = slots.size - jobs.size
            onProgress(done, slots.size)
            for ((slot, job) in jobs) {
                slots[slot] = job.get()
                onProgress(++done, slots.size)
            }
        } finally {
            pool.shutdownNow()
        }
        val readMs = msSince(readStart)
        val songs = slots.filterNotNull()

        val writeStart = System.nanoTime()
        writeIndex(songs)
        removeStaleArt(songs)
        val writeMs = msSince(writeStart)
        Log.i(TAG, "media: scanned ${songs.size} songs (${jobs.size} read, " +
            "${songs.size - jobs.size} unchanged) in ${msSince(start)} ms: " +
            "list $listMs ms, read $readMs ms on " +
            "${if (jobs.isEmpty()) 0 else threads} threads " +
            "(summed: tags ${tagsNs.get() / 1_000_000} ms, " +
            "art ${artNs.get() / 1_000_000} ms), write $writeMs ms")
        return songs.size
    }

    private fun msSince(startNs: Long): Long = (System.nanoTime() - startNs) / 1_000_000

    // Walk the tree one directory at a time: one ContentResolver query per
    // folder, rather than DocumentFile's one call per file and property.
    private fun listAudioFiles(treeUri: Uri): List<Found> {
        val out = ArrayList<Found>()
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        val pending = ArrayDeque<Pair<String, String>>()
        pending.add(DocumentsContract.getTreeDocumentId(treeUri) to "")
        while (pending.isNotEmpty()) {
            val (dirId, folder) = pending.removeFirst()
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(
                treeUri, dirId
            )
            try {
                context.contentResolver.query(
                    children, projection, null, null, null
                )?.use { c ->
                    while (c.moveToNext()) {
                        val docId = c.getString(0) ?: continue
                        val name = c.getString(1) ?: continue
                        if (name.startsWith(".")) continue
                        val mime = c.getString(2) ?: ""
                        if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                            val sub = if (folder.isEmpty()) name else "$folder/$name"
                            pending.add(docId to sub)
                            continue
                        }
                        val ext = name.substringAfterLast('.', "").lowercase()
                        if (ext !in AUDIO_EXTENSIONS &&
                            (ext.isNotEmpty() || mime !in AUDIO_MIME_TYPES)) {
                            continue
                        }
                        out.add(Found(
                            documentId = docId,
                            uri = DocumentsContract.buildDocumentUriUsingTree(
                                treeUri, docId
                            ),
                            name = name,
                            folder = folder,
                            mtime = if (c.isNull(3)) 0 else c.getLong(3) / 1000,
                            size = if (c.isNull(4)) 0 else c.getLong(4),
                        ))
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "media: couldn't list $folder: $e")
            }
        }
        out.sortBy { it.folder + "/" + it.name }
        return out
    }

    private fun readSong(file: Found, id: Long, locator: String): Song {
        var title = file.name.substringBeforeLast('.')
        var album = file.folder.substringAfterLast('/')
        var artist = ""
        var albumArtist = ""
        var genre = ""
        var durationMs = 0L
        var track = 0
        var hasArt = false
        var picture: ByteArray? = null
        val tagsStart = System.nanoTime()
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(context, file.uri)
            fun tag(key: Int): String? =
                retriever.extractMetadata(key)?.trim()?.takeIf { it.isNotEmpty() }
            tag(MediaMetadataRetriever.METADATA_KEY_TITLE)?.let { title = it }
            tag(MediaMetadataRetriever.METADATA_KEY_ALBUM)?.let { album = it }
            tag(MediaMetadataRetriever.METADATA_KEY_ARTIST)?.let { artist = it }
            tag(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST)?.let {
                albumArtist = it
            }
            tag(MediaMetadataRetriever.METADATA_KEY_GENRE)?.let { genre = it }
            durationMs = tag(MediaMetadataRetriever.METADATA_KEY_DURATION)
                ?.toLongOrNull() ?: 0L
            // "3/12" or "3".
            track = tag(MediaMetadataRetriever.METADATA_KEY_CD_TRACK_NUMBER)
                ?.substringBefore('/')?.trim()?.toIntOrNull() ?: 0
            picture = retriever.embeddedPicture
        } catch (e: Exception) {
            Log.w(TAG, "media: no tags for ${file.folder}/${file.name}: $e")
        } finally {
            try {
                retriever.release()
            } catch (e: Exception) {
                // Nothing useful to do.
            }
        }
        tagsNs.addAndGet(System.nanoTime() - tagsStart)
        // Saved after release(), so the retriever's native buffers are
        // gone before the picture is decoded.
        picture?.let {
            val artStart = System.nanoTime()
            hasArt = saveArt(id, it)
            artNs.addAndGet(System.nanoTime() - artStart)
        }
        return Song(
            id = id,
            locator = locator,
            mtime = file.mtime,
            size = file.size,
            durationMs = durationMs,
            track = track,
            title = title,
            artist = artist,
            album = album,
            albumArtist = albumArtist,
            genre = genre,
            folder = file.folder,
            hasArt = hasArt,
        )
    }

    private fun artFile(id: Long): File =
        File(artDir, java.lang.Long.toUnsignedString(id) + ".png")

    // Shrink a cover picture so its longest side is at most 256 and store
    // it as a PNG. This is the user's art, so resizing it is fine.
    private fun saveArt(id: Long, picture: ByteArray): Boolean {
        // Decode at a power-of-two fraction that still covers 256, not at
        // full size: with several read threads, full-size decodes of big
        // scans (a 3000px cover is 36 MB) could run out of memory.
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(picture, 0, picture.size, bounds)
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >=
            MAX_ART_SIDE) {
            sample *= 2
        }
        val options = BitmapFactory.Options().apply { inSampleSize = sample }
        val decoded = BitmapFactory.decodeByteArray(picture, 0, picture.size, options)
            ?: return false
        val longest = maxOf(decoded.width, decoded.height)
        val bitmap = if (longest > MAX_ART_SIDE) {
            val scale = MAX_ART_SIDE.toFloat() / longest
            Bitmap.createScaledBitmap(
                decoded,
                maxOf(1, Math.round(decoded.width * scale)),
                maxOf(1, Math.round(decoded.height * scale)),
                true
            )
        } else {
            decoded
        }
        return try {
            FileOutputStream(artFile(id)).use {
                bitmap.compress(Bitmap.CompressFormat.PNG, 100, it)
            }
        } catch (e: Exception) {
            Log.w(TAG, "media: couldn't save art for $id: $e")
            false
        } finally {
            if (bitmap !== decoded) bitmap.recycle()
            decoded.recycle()
        }
    }

    // The previous index, for skipping unchanged files. Missing or corrupt
    // means everything gets read again.
    private fun readIndex(): Map<Long, Song> {
        val out = HashMap<Long, Song>()
        if (!indexFile.isFile) return out
        try {
            val lines = indexFile.readLines(Charsets.UTF_8)
            if (lines.firstOrNull()?.trimStart('﻿') != HEADER) return out
            for (line in lines.drop(1)) {
                val f = line.split('\t')
                if (f.size != COLUMNS) continue
                val id = f[0].toULongOrNull()?.toLong() ?: continue
                out[id] = Song(
                    id = id,
                    locator = unescape(f[1]),
                    mtime = f[2].toLongOrNull() ?: continue,
                    size = f[3].toLongOrNull() ?: continue,
                    durationMs = f[4].toLongOrNull() ?: continue,
                    track = f[5].toIntOrNull() ?: continue,
                    title = unescape(f[6]),
                    artist = unescape(f[7]),
                    album = unescape(f[8]),
                    albumArtist = unescape(f[9]),
                    genre = unescape(f[10]),
                    folder = unescape(f[11]),
                    hasArt = f[12] == "1",
                )
            }
        } catch (e: Exception) {
            Log.w(TAG, "media: ignoring unreadable index: $e")
        }
        return out
    }

    private fun writeIndex(songs: List<Song>) {
        val text = StringBuilder()
        text.append(HEADER).append('\n')
        for (s in songs) {
            text.append(java.lang.Long.toUnsignedString(s.id)).append('\t')
                .append(escape(s.locator)).append('\t')
                .append(s.mtime).append('\t')
                .append(s.size).append('\t')
                .append(s.durationMs).append('\t')
                .append(s.track).append('\t')
                .append(escape(s.title)).append('\t')
                .append(escape(s.artist)).append('\t')
                .append(escape(s.album)).append('\t')
                .append(escape(s.albumArtist)).append('\t')
                .append(escape(s.genre)).append('\t')
                .append(escape(s.folder)).append('\t')
                .append(if (s.hasArt) "1" else "0").append('\n')
        }
        libraryDir.mkdirs()
        val tmp = File(libraryDir, "index.tsv.tmp")
        tmp.writeText(text.toString(), Charsets.UTF_8)
        if (!tmp.renameTo(indexFile)) {
            // renameTo won't replace an existing file on every filesystem.
            indexFile.delete()
            if (!tmp.renameTo(indexFile)) {
                Log.e(TAG, "media: couldn't move $tmp into place")
            }
        }
    }

    private fun removeStaleArt(songs: List<Song>) {
        val keep = songs.filter { it.hasArt }
            .map { java.lang.Long.toUnsignedString(it.id) + ".png" }
            .toHashSet()
        artDir.listFiles()?.forEach {
            if (it.name.endsWith(".png") && it.name !in keep) it.delete()
        }
    }
}
