/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SetupPillTest {
    // A 2400x1080 phone: the 3:2 game is 1620 wide, leaving 390px bars.
    @Test
    fun gearSitsInTheMiddleOfTheLeftBar() {
        val (x, y) = SetupPill.position(2400, 1080, 120, 24)
        assertEquals((390 - 120) / 2, x)
        assertEquals(24, y)
    }

    // A 4:3 tablet has bars top and bottom, none at the sides.
    @Test
    fun noSideBarsMeansTheTopLeftCorner() {
        assertEquals(Pair(24, 24), SetupPill.position(2048, 1536, 120, 24))
    }

    // A bar too narrow for the gear and its margins.
    @Test
    fun aNarrowBarIsntUsed() {
        // 1700x1080: game 1620 wide, bars 40px.
        assertEquals(Pair(24, 24), SetupPill.position(1700, 1080, 120, 24))
    }

    @Test
    fun autoDimIsOnUnlessTheSettingsSayFalse() {
        assertTrue(SetupPill.autoDimFromText(null))
        assertTrue(SetupPill.autoDimFromText("first_run_done=true\n"))
        assertTrue(SetupPill.autoDimFromText("gear_auto_dim=true\n"))
        assertFalse(SetupPill.autoDimFromText("# x\n gear_auto_dim = false \r\n"))
        // A comment doesn't count.
        assertTrue(SetupPill.autoDimFromText("#gear_auto_dim=false\n"))
    }

    // The data folder as the external-storage provider names it, for
    // opening it in a file manager.
    @Test
    fun dataFolderDocumentIds() {
        assertEquals("primary:SongSummoner", DataFolder.documentId("/sdcard/SongSummoner"))
        assertEquals("primary:SongSummoner", DataFolder.documentId("/sdcard/SongSummoner/"))
        assertEquals(
            "primary:SongSummoner",
            DataFolder.documentId("/storage/emulated/0/SongSummoner")
        )
        assertEquals("primary:a/b", DataFolder.documentId("/sdcard/a/b"))
    }

    // A file manager that came back this fast never really showed.
    @Test
    fun aQuickReturnMeansTheFileManagerDidntOpen() {
        assertTrue(DataFolder.cameBackTooSoon(300))
        assertFalse(DataFolder.cameBackTooSoon(5000))
    }

    // Files first (both the Google and the AOSP build), then any file
    // manager, then the system folder browser, which every phone has.
    @Test
    fun fileManagersAreTriedInOrder() {
        assertEquals(
            listOf(
                DataFolder.Attempt.FILES_GOOGLE,
                DataFolder.Attempt.FILES_AOSP,
                DataFolder.Attempt.ANY_VIEWER,
                DataFolder.Attempt.FOLDER_BROWSER,
            ),
            DataFolder.ATTEMPTS
        )
    }

    @Test
    fun theTipPageIsTheDevelopersPage() {
        // Same page as credits.rs TIP_URL; only ever this one address.
        assertEquals("https://www.buymeacoffee.com/johnnycolli", TipPage.URL)
    }

    @Test
    fun actionsTheEngineSendsAreUnderstood() {
        assertEquals(SetupAction.CHANGE_IPA, SetupAction.parse("change_ipa"))
        assertEquals(SetupAction.ADD_MUSIC, SetupAction.parse("add_music"))
        assertEquals(SetupAction.RESCAN, SetupAction.parse("rescan"))
        assertEquals(SetupAction.OPEN_DATA_FOLDER, SetupAction.parse("open_data_folder"))
        assertEquals(SetupAction.RESTART, SetupAction.parse("restart"))
        assertEquals(SetupAction.PICK_SAVE_FOLDER, SetupAction.parse("pick_save_folder"))
        assertEquals(SetupAction.OPEN_TIP_PAGE, SetupAction.parse("open_tip_page"))
        assertNull(SetupAction.parse("format_disk"))
        assertNull(SetupAction.parse(null))
    }
}
