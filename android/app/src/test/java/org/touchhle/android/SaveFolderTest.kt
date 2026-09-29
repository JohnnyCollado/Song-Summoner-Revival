/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SaveFolderTest {
    private val docs = "touchHLE_sandbox/com.square-enix.SongSummonerEncore/Documents"
    private val prefs = "touchHLE_sandbox/com.square-enix.SongSummonerEncore/Library/Preferences"

    // Exactly what the first release (and this one) keeps.
    @Test
    fun theGamesOwnFilesAreAllowed() {
        assertTrue(SaveFiles.allowed("$docs/save.dat", 1000))
        assertTrue(SaveFiles.allowed("$docs/slot/1.dat", 1000))
        assertTrue(SaveFiles.allowed("$prefs/com.square-enix.SongSummonerEncore.plist", 1000))
        assertTrue(SaveFiles.allowed("song_summoner_settings.txt", 100))
        assertTrue(SaveFiles.allowed("library/playcounts.tsv", 100))
        assertTrue(SaveFiles.allowed("library/source.txt", 100))
        assertTrue(SaveFiles.allowed("backups/saves-2026-09-28_1904.zip", 1000))
        assertTrue(SaveFiles.allowed("bug-reports/bug-report-2026-09-28_1904.zip", 1000))
    }

    // Nothing else, whatever else is in the folder.
    @Test
    fun everythingElseIsRefused() {
        for (path in listOf(
            "Song Summoner The Unsung Heroes Encore.ipa",
            "touchHLE_log.txt",
            "touchHLE_options.txt",
            "library/index.tsv",
            "library/art/1.png",
            "touchHLE_sandbox/com.other.Game/Documents/save.dat",
            "$prefs/notes.txt",
            "$prefs/sub/x.plist",
            "backups/notes.txt",
            "backups/evil.zip",
            "touchHLE_apps/Game.ipa",
        )) {
            assertFalse(path, SaveFiles.allowed(path, 10))
        }
    }

    // No way out of the folder, and nothing odd in names.
    @Test
    fun pathTricksAreRefused() {
        for (path in listOf(
            "$docs/../../../evil",
            "$docs/./save",
            "$docs//save",
            "$docs/.hidden",
            "$docs/a\\b",
            "$docs/C:save",
            "$docs/tab\tname",
            "/$docs/save",
            "$docs/",
            "",
            "$docs/a/b/c/d/e/too-deep",
        )) {
            assertFalse(path, SaveFiles.allowed(path, 10))
        }
    }

    @Test
    fun oversizedFilesAreRefused() {
        assertFalse(SaveFiles.allowed("$docs/save.dat", SaveFiles.MAX_SAVE_BYTES + 1))
        assertFalse(SaveFiles.allowed("song_summoner_settings.txt", SaveFiles.MAX_TEXT_BYTES + 1))
        assertFalse(SaveFiles.allowed("backups/saves-x.zip", SaveFiles.MAX_ZIP_BYTES + 1))
        assertFalse(SaveFiles.allowed("$docs/save.dat", -1))
    }

    // A folder "has saves" only with the game's own save files in it.
    @Test
    fun oldReleaseSavesAreRecognised() {
        assertTrue(SaveFiles.hasSaves(listOf("touchHLE_log.txt", "$docs/save.dat")))
        assertTrue(SaveFiles.hasSaves(listOf("$prefs/com.square-enix.SongSummonerEncore.plist")))
        assertFalse(SaveFiles.hasSaves(listOf("song_summoner_settings.txt", "backups/saves-x.zip")))
        assertFalse(SaveFiles.hasSaves(emptyList()))
    }

    // An import takes the allowed files only, within the overall limits.
    @Test
    fun theImportPlanFiltersAndCaps() {
        val entries = listOf(
            SaveFiles.Entry("$docs/save.dat", 100),
            SaveFiles.Entry("Song Summoner The Unsung Heroes Encore.ipa", 200_000_000),
            SaveFiles.Entry("$docs/../evil", 1),
        )
        assertEquals(listOf("$docs/save.dat"), SaveFiles.importPlan(entries).map { it.path })

        val many = (0 until SaveFiles.MAX_FILES + 10).map {
            SaveFiles.Entry("$docs/f$it", 1)
        }
        assertEquals(SaveFiles.MAX_FILES, SaveFiles.importPlan(many).size)

        val big = (0 until 20).map {
            SaveFiles.Entry("backups/saves-$it.zip", SaveFiles.MAX_ZIP_BYTES)
        }
        val planned = SaveFiles.importPlan(big)
        assertTrue(planned.sumOf { it.size } <= SaveFiles.MAX_TOTAL_BYTES)
    }

    // Some providers (cloud folders) don't say how big a file is (-1).
    // Those saves still come in: the copy itself stops at the limit.
    @Test
    fun filesOfUnknownSizeAreStillImported() {
        val entries = listOf(
            SaveFiles.Entry("$docs/save.dat", -1),
            SaveFiles.Entry("$prefs/com.square-enix.SongSummonerEncore.plist", -1),
            SaveFiles.Entry("Song Summoner The Unsung Heroes Encore.ipa", -1),
        )
        assertEquals(
            listOf("$docs/save.dat", "$prefs/com.square-enix.SongSummonerEncore.plist"),
            SaveFiles.importPlan(entries).map { it.path },
        )
    }

    // First launch: the game file, then the save folder, then the game
    // (which opens the Setup menu). Nothing else is asked first.
    @Test
    fun firstLaunchAsksForTheGameThenTheSaveFolder() {
        assertEquals(FirstRun.Step.PICK_IPA, FirstRun.next(hasIpa = false, hasSaveFolder = false))
        assertEquals(FirstRun.Step.PICK_IPA, FirstRun.next(hasIpa = false, hasSaveFolder = true))
        assertEquals(FirstRun.Step.PICK_SAVE_FOLDER, FirstRun.next(hasIpa = true, hasSaveFolder = false))
        assertEquals(FirstRun.Step.START, FirstRun.next(hasIpa = true, hasSaveFolder = true))
    }
}
