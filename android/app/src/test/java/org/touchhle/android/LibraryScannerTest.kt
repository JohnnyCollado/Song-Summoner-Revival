/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import org.junit.Assert.assertEquals
import org.junit.Test

class LibraryScannerTest {
    // One read thread per core, at least 2 (reading is mostly waiting on
    // the storage provider, so even a single core overlaps two) and at
    // most 8 (past that, SAF opens contend and nothing is gained).
    @Test
    fun readThreadsFollowTheCoreCount() {
        assertEquals(4, LibraryScanner.readThreads(4))
        assertEquals(6, LibraryScanner.readThreads(6))
        assertEquals(8, LibraryScanner.readThreads(8))
    }

    @Test
    fun readThreadsAreAtLeastTwo() {
        assertEquals(2, LibraryScanner.readThreads(1))
        assertEquals(2, LibraryScanner.readThreads(2))
        // availableProcessors() is documented to be at least 1, but don't
        // trust a broken device.
        assertEquals(2, LibraryScanner.readThreads(0))
    }

    @Test
    fun readThreadsAreAtMostEight() {
        assertEquals(8, LibraryScanner.readThreads(12))
        assertEquals(8, LibraryScanner.readThreads(64))
    }

    // Playlists are named after their music folder only when there are
    // several, the same rule as the desktop scanner.
    @Test
    fun oneMusicFolderMeansNoPrefix() {
        assertEquals("Rock/Live", LibraryScanner.folderFor("Music", "Rock/Live", 1))
        assertEquals("", LibraryScanner.folderFor("Music", "", 1))
    }

    @Test
    fun severalMusicFoldersPrefixTheirName() {
        assertEquals("OSTs/Rock", LibraryScanner.folderFor("OSTs", "Rock", 2))
        assertEquals("OSTs", LibraryScanner.folderFor("OSTs", "", 2))
    }

    @Test
    fun musicFolderNamesComeFromTheTree() {
        assertEquals("Music", LibraryScanner.rootName("primary:Music"))
        assertEquals("OSTs", LibraryScanner.rootName("primary:Music/OSTs"))
        assertEquals("Internal storage", LibraryScanner.rootName("primary:"))
        assertEquals("SD card", LibraryScanner.rootName("1234-5678:"))
    }
}
