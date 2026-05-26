/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
package org.touchhle.android;

import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.provider.DocumentsContract;
import android.provider.Settings;
import android.util.Log;

import org.libsdl.app.SDLActivity;
import org.touchhle.android.BuildConfig;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileReader;
import java.io.IOException;
import java.io.InputStream;

/**
 * A wrapper class over SDLActivity. Adds two pieces of Android setup that
 * touchHLE's Rust side can't do itself:
 *
 *   1. A one-time, deferred prompt for MANAGE_EXTERNAL_STORAGE so user data
 *      can live at /sdcard/touchHLE/ instead of the Android/data sandbox.
 *   2. A one-time SAF (Storage Access Framework) folder picker so the user
 *      can choose where their music library lives. The picked path is
 *      converted to a real /storage path and written to
 *      touchHLE_music_library.txt, which music_library.rs already honors.
 *
 * Both prompts are gated on SharedPreferences flags so the user isn't
 * bounced out of the app every relaunch, and both are deferred via
 * Handler.postDelayed so SDL's SurfaceView has time to establish before we
 * background the activity (firing startActivity inline in onCreate races
 * SDL init and produces a permanent black screen on return).
 */
public class MainActivity extends SDLActivity {

    private static final String TAG = "touchHLE";
    private static final String PREFS_NAME = "touchHLE";
    private static final String PREF_REQUESTED_ALL_FILES = "requested_all_files_access";

    /** Path where music_library.rs reads the chosen music folder from. */
    private static final String MUSIC_LIBRARY_FILE =
        "/sdcard/touchHLE/touchHLE_music_library.txt";

    private static final int REQ_PICK_MUSIC_FOLDER = 1001;

    /**
     * Delay before firing any setup prompt. Must be long enough for
     * SDLActivity's super.onCreate to fully establish the SurfaceView and
     * for the SDL native thread to start running (~250ms in practice), but
     * short enough that the user hasn't had time to tap a game icon in the
     * touchHLE app picker yet (typical human reaction + tap latency is
     * ~1.5s+). If we fire after the user has loaded an iOS app, the popup
     * triggers app-will-resign-active in uikit.rs, which currently exits
     * the whole touchHLE process -- so we'd kill their game session.
     */
    private static final long SETUP_PROMPT_DELAY_MS = 1000;

    /**
     * Absolute path to the bundled IPA after it's been unpacked into the
     * app's internal-storage files dir. Computed once in {@link #onCreate}
     * (before super, so {@link #getArguments} can return it as argv[1] when
     * SDL fires up the native thread). Null in the non-wrapper flavor.
     */
    private String wrapperIpaPath = null;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // CRITICAL: unpack the bundled IPA BEFORE super.onCreate(). SDL's
        // SDLActivity.onCreate spins up the native thread which calls
        // getArguments() shortly after; if the IPA isn't on disk yet,
        // touchHLE's main() will see no bundle_path and fall back to the
        // app-picker, defeating the wrapper.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            wrapperIpaPath = ensureWrapperIpaUnpacked();
        }
        // Extract the album-art placeholder bundled in assets to the
        // user-data folder so the Rust runtime can fs::read it. Safe to
        // call every launch -- it's a no-op when the file is already
        // present at the right size.
        ensureAlbumPlaceholderUnpacked();
        super.onCreate(savedInstanceState);
        new Handler(Looper.getMainLooper()).postDelayed(
            this::runFirstLaunchSetup,
            SETUP_PROMPT_DELAY_MS
        );
    }

    /**
     * In wrapper-flavor builds (BuildConfig.WRAPPER_AUTO_LAUNCH = true),
     * return the absolute path to the bundled IPA so touchHLE's Rust main
     * skips the app picker and launches the game directly.
     *
     * In the generic touchhle flavor we return {@code super.getArguments()}
     * (an empty array), preserving the historical "show app picker"
     * behaviour.
     */
    @Override
    protected String[] getArguments() {
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && wrapperIpaPath != null) {
            Log.i(TAG, "wrapper auto-launch: passing IPA path to SDL_main: " + wrapperIpaPath);
            return new String[] { wrapperIpaPath };
        }
        return super.getArguments();
    }

    /**
     * Extract the IPA bundled in {@code assets/<WRAPPER_IPA_ASSET>} into the
     * app's internal-storage files dir, named with the human-readable
     * {@code WRAPPER_IPA_FILENAME}. Idempotent: if a same-sized file already
     * exists at the destination we return its path without re-copying
     * (saves ~260 MB of I/O on every launch after the first).
     *
     * Returns the absolute path on success, or null on failure (in which
     * case the wrapper falls back to the picker, like the generic flavor).
     */
    private String ensureWrapperIpaUnpacked() {
        String asset = BuildConfig.WRAPPER_IPA_ASSET;
        String fname = BuildConfig.WRAPPER_IPA_FILENAME;
        if (asset == null || asset.isEmpty() || fname == null || fname.isEmpty()) {
            return null;
        }
        File dest = new File(getFilesDir(), fname);
        long expectedSize;
        try (android.content.res.AssetFileDescriptor afd = getAssets().openFd(asset)) {
            // Reliable byte count for an uncompressed asset (we mark .ipa
            // noCompress in build.gradle.kts so openFd works -- compressed
            // assets fail with FileNotFoundException here).
            expectedSize = afd.getLength();
        } catch (IOException e) {
            Log.e(TAG, "Bundled IPA asset \"" + asset + "\" not openable as FD "
                + "(was build.gradle's noCompress(\"ipa\") in effect?): " + e);
            return null;
        }
        if (dest.isFile() && dest.length() == expectedSize) {
            // Already unpacked from a previous launch.
            return dest.getAbsolutePath();
        }
        Log.i(TAG, "Unpacking bundled IPA \"" + asset + "\" -> " + dest.getAbsolutePath()
            + " (" + expectedSize + " bytes).");
        long t0 = System.currentTimeMillis();
        try (InputStream in = getAssets().open(asset);
             FileOutputStream out = new FileOutputStream(dest)) {
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) {
                out.write(buf, 0, n);
            }
        } catch (IOException e) {
            Log.e(TAG, "Failed to unpack bundled IPA: " + e);
            // Don't leave a half-written file around -- next launch would
            // size-match and skip the copy, leaving us with a corrupt bundle.
            //noinspection ResultOfMethodCallIgnored
            dest.delete();
            return null;
        }
        Log.i(TAG, "Unpacked IPA in " + (System.currentTimeMillis() - t0) + " ms.");
        return dest.getAbsolutePath();
    }

    /**
     * Copy assets/album_placeholder.png into the user-data folder under
     * res/album_placeholder.png so touchHLE's Rust loader (which does a
     * plain {@code fs::read("res/album_placeholder.png")} keyed off
     * user_data_base_path on Android) can find it. We write to BOTH
     * candidate locations the Rust paths logic might resolve to:
     *   - /sdcard/touchHLE/res/   (preferred; only writable once the user
     *                              grants MANAGE_EXTERNAL_STORAGE)
     *   - getExternalFilesDir/res (always writable; fallback the Rust
     *                              side falls back to as well)
     * That way the file is in place before *or* after the user grants
     * "All files access" -- whichever Rust resolves to, it'll find it.
     * Idempotent: skips the copy when the destination already exists
     * with the expected byte count.
     */
    private void ensureAlbumPlaceholderUnpacked() {
        final String ASSET = "album_placeholder.png";
        long expectedSize;
        try (android.content.res.AssetFileDescriptor afd = getAssets().openFd(ASSET)) {
            expectedSize = afd.getLength();
        } catch (IOException e) {
            // Asset is markCompressed by default; openFd fails for
            // compressed assets. Fall back to streaming through open()
            // and count bytes ourselves.
            try (InputStream in = getAssets().open(ASSET)) {
                long n = 0;
                byte[] buf = new byte[64 * 1024];
                int r;
                while ((r = in.read(buf)) > 0) n += r;
                expectedSize = n;
            } catch (IOException e2) {
                Log.w(TAG, "album_placeholder.png asset missing: " + e2);
                return;
            }
        }
        java.util.ArrayList<File> targets = new java.util.ArrayList<>();
        targets.add(new File("/sdcard/touchHLE/res/" + ASSET));
        File ext = getExternalFilesDir(null);
        if (ext != null) {
            targets.add(new File(ext, "res/" + ASSET));
        }
        for (File dest : targets) {
            if (dest.isFile() && dest.length() == expectedSize) continue;
            File parent = dest.getParentFile();
            if (parent != null) //noinspection ResultOfMethodCallIgnored
                parent.mkdirs();
            try (InputStream in = getAssets().open(ASSET);
                 FileOutputStream out = new FileOutputStream(dest)) {
                byte[] buf = new byte[64 * 1024];
                int n;
                while ((n = in.read(buf)) > 0) {
                    out.write(buf, 0, n);
                }
                Log.i(TAG, "Extracted " + ASSET + " -> " + dest.getAbsolutePath());
            } catch (IOException e) {
                // Permission-denied on /sdcard/touchHLE/ before
                // MANAGE_EXTERNAL_STORAGE is granted is normal -- the
                // fallback ext-files path will succeed.
                Log.i(TAG, "Could not extract " + ASSET + " to "
                    + dest.getAbsolutePath() + ": " + e);
            }
        }
    }

    /**
     * Chain of one-time setup steps. Each step is gated on its own
     * SharedPreferences flag so we never re-prompt for something we already
     * asked about. We only fire one prompt per launch -- if storage isn't
     * granted yet, we ask for storage and bail; the music picker waits for
     * the next launch (when storage is granted and the user is back at the
     * picker).
     */
    private void runFirstLaunchSetup() {
        SharedPreferences prefs = getSharedPreferences(PREFS_NAME, MODE_PRIVATE);

        // Step 1: MANAGE_EXTERNAL_STORAGE.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R
                && !Environment.isExternalStorageManager()) {
            if (!prefs.getBoolean(PREF_REQUESTED_ALL_FILES, false)) {
                prefs.edit().putBoolean(PREF_REQUESTED_ALL_FILES, true).apply();
                Log.i(TAG, "Opening Settings to request MANAGE_EXTERNAL_STORAGE.");
                if (!openAllFilesAccessSettings()) {
                    Log.w(TAG, "No Settings page for MANAGE_EXTERNAL_STORAGE on this device; "
                        + "staying in the private Android/data folder.");
                }
            } else {
                Log.i(TAG, "MANAGE_EXTERNAL_STORAGE not granted; already asked once, not re-asking.");
            }
            // Don't chain the music picker here -- it needs /sdcard access
            // which we don't have yet, and we don't want to stack two
            // startActivity calls in the same tick.
            return;
        }

        // Step 2: SAF music-folder picker. Only useful once we have
        // /sdcard access -- without MANAGE_EXTERNAL_STORAGE, the resolved
        // path wouldn't be readable anyway.
        //
        // We re-prompt every launch if the saved path doesn't point at a
        // real directory (file missing, or path stale because the user
        // deleted/moved the folder, ejected an SD card, etc.). The
        // `pickedThisLaunch` static guards against firing the picker twice
        // in the same process if onCreate ever ran more than once (e.g.
        // config-change recreate).
        if (pickedThisLaunch) {
            return;
        }
        if (!isSavedMusicLibraryPathValid()) {
            pickedThisLaunch = true;
            Log.i(TAG, "No valid music folder set yet -- opening SAF folder picker.");
            if (!openMusicFolderPicker()) {
                Log.w(TAG, "SAF folder picker unavailable; music_library.rs will "
                    + "auto-detect a folder instead.");
            }
        }
    }

    /** Set true once we've actually launched the SAF picker, so a config
     *  recreate (rotation, etc.) doesn't fire it twice in one session. */
    private static boolean pickedThisLaunch = false;

    /**
     * Returns true iff {@link #MUSIC_LIBRARY_FILE} exists, can be read, and
     * its trimmed contents name a directory that currently exists. Anything
     * else (file missing, empty, non-absolute path, deleted folder) counts
     * as "no valid path" and triggers a re-prompt.
     */
    private static boolean isSavedMusicLibraryPathValid() {
        File libFile = new File(MUSIC_LIBRARY_FILE);
        if (!libFile.isFile()) {
            return false;
        }
        try (BufferedReader br = new BufferedReader(new FileReader(libFile))) {
            String first = br.readLine();
            if (first == null) {
                return false;
            }
            String trimmed = first.trim();
            if (trimmed.isEmpty()) {
                return false;
            }
            return new File(trimmed).isDirectory();
        } catch (Exception e) {
            Log.w(TAG, "Couldn't read " + MUSIC_LIBRARY_FILE + ": " + e);
            return false;
        }
    }

    private boolean openAllFilesAccessSettings() {
        try {
            Intent intent = new Intent(
                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:" + getPackageName())
            );
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            startActivity(intent);
            return true;
        } catch (Exception primary) {
            Log.w(TAG, "App-specific MANAGE_EXTERNAL_STORAGE page unavailable: " + primary);
            try {
                Intent fallback = new Intent(
                    Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION
                );
                fallback.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                startActivity(fallback);
                return true;
            } catch (Exception secondary) {
                Log.w(TAG, "Generic page also unavailable: " + secondary);
                return false;
            }
        }
    }

    private boolean openMusicFolderPicker() {
        // CRITICAL: write a fallback path to MUSIC_LIBRARY_FILE *before*
        // we fire the SAF picker. Launching the picker backgrounds
        // touchHLE, and touchHLE's app-will-resign-active handler in
        // uikit.rs exits the whole process before our onActivityResult
        // can run. That means if we waited until the picker returned to
        // persist anything, the file would never get written, and next
        // launch we'd fire the picker again -- infinite loop, user can
        // never actually use touchHLE. By pre-seeding the file we
        // guarantee the loop is broken even if our process dies mid-pick.
        // If the user does pick something, onActivityResult overwrites
        // the fallback with their actual choice.
        //
        // Pick the BEST candidate (most audio files) so users who never
        // complete the SAF flow still get a working library on next
        // launch, instead of being locked into an empty seed folder.
        String fallback = pickBestCandidateMusicFolder();
        writeMusicLibraryFile(fallback);

        try {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            intent.addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION |
                Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            );
            // Hint at a starting location if we can; ignored by some pickers.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Uri musicHint = Uri.parse(
                    "content://com.android.externalstorage.documents/document/primary%3AMusic"
                );
                intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, musicHint);
            }
            startActivityForResult(intent, REQ_PICK_MUSIC_FOLDER);
            return true;
        } catch (Exception e) {
            Log.w(TAG, "Couldn't launch ACTION_OPEN_DOCUMENT_TREE: " + e);
            return false;
        }
    }

    /**
     * Choose the music-folder fallback to pre-seed before opening SAF.
     * Tries the well-known external-storage music locations and returns
     * the one with the most audio files. If none have any music we still
     * return (and create) /sdcard/touchHLE/Music so there's a writable
     * destination for the user to drop files into later.
     *
     * Mirrors the Rust-side candidate set in
     * music_library.rs::prompt_for_folder so picker-side and Rust-side
     * fallbacks agree on which folder "wins".
     */
    private static String pickBestCandidateMusicFolder() {
        String[] candidates = new String[] {
            "/sdcard/Music",
            "/sdcard/touchHLE/Music",
            "/sdcard/Download",
        };
        String best = null;
        int bestCount = 0;
        for (String raw : candidates) {
            File dir = new File(raw);
            if (!dir.isDirectory()) {
                continue;
            }
            int n = countAudioFilesRecursive(dir, 0);
            Log.i(TAG, "fallback scan: " + raw + " -> " + n + " audio file(s).");
            // Strictly greater so earlier candidates win ties — matches Rust.
            if (n > bestCount) {
                best = raw;
                bestCount = n;
            }
        }
        if (best != null && bestCount > 0) {
            Log.i(TAG, "pre-seeding music folder with best candidate: " + best);
            return best;
        }
        // Nothing useful found — still create /sdcard/touchHLE/Music so the
        // user has somewhere obvious to drop files later.
        File seed = new File("/sdcard/touchHLE/Music");
        if (!seed.isDirectory()) {
            seed.mkdirs();
        }
        Log.i(TAG, "no audio found in any candidate; seeding " + seed.getAbsolutePath());
        return seed.getAbsolutePath();
    }

    /** Audio-file extensions counted by the fallback scanner. Must stay in
     *  sync with music_library.rs::SUPPORTED_EXTENSIONS. */
    private static final String[] AUDIO_EXTS = {
        ".mp3", ".m4a", ".aac", ".wav", ".flac", ".ogg", ".oga",
        ".caf", ".aif", ".aiff",
    };

    /** Recursive count of audio files under {@code dir}. Caps depth and total
     *  count to keep the pre-launch probe quick on huge trees, matching the
     *  Rust-side count_audio_files_recursive. */
    private static int countAudioFilesRecursive(File dir, int depth) {
        final int MAX_COUNT = 5000;
        final int MAX_DEPTH = 4;
        if (depth > MAX_DEPTH) return 0;
        File[] entries = dir.listFiles();
        if (entries == null) return 0;
        int n = 0;
        for (File f : entries) {
            String name = f.getName();
            if (name.startsWith(".")) continue;
            if (f.isDirectory()) {
                n += countAudioFilesRecursive(f, depth + 1);
                if (n >= MAX_COUNT) return n;
                continue;
            }
            String lower = name.toLowerCase();
            for (String ext : AUDIO_EXTS) {
                if (lower.endsWith(ext)) {
                    n++;
                    break;
                }
            }
            if (n >= MAX_COUNT) return n;
        }
        return n;
    }

    /** Best-effort write of an absolute path into MUSIC_LIBRARY_FILE.
     *  Logs (and swallows) any I/O error: the worst case is the prompt
     *  fires again next launch, which is no worse than before. */
    private static void writeMusicLibraryFile(String absolutePath) {
        try {
            File parent = new File(MUSIC_LIBRARY_FILE).getParentFile();
            if (parent != null && !parent.isDirectory()) {
                parent.mkdirs();
            }
            try (FileOutputStream out = new FileOutputStream(MUSIC_LIBRARY_FILE)) {
                out.write(absolutePath.getBytes("UTF-8"));
            }
            Log.i(TAG, "Wrote music folder to " + MUSIC_LIBRARY_FILE + ": " + absolutePath);
        } catch (Exception e) {
            Log.e(TAG, "Couldn't persist music folder choice: " + e);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQ_PICK_MUSIC_FOLDER) {
            return;
        }
        // openMusicFolderPicker() already wrote a fallback path before
        // firing this picker (so the user isn't stuck in an
        // exit-on-resign-active loop if our process dies mid-pick). All we
        // do here is *overwrite* the fallback if the user actually picked
        // something resolvable. NOTE: in practice this rarely runs on
        // touchHLE today, because uikit.rs's app-will-resign-active
        // handler exits the process before we get here. The pre-write in
        // openMusicFolderPicker is what users actually rely on.
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            Log.i(TAG, "Music folder picker cancelled (fallback already on disk).");
            return;
        }
        Uri treeUri = data.getData();
        String resolved = resolveTreeUriToFilesystemPath(treeUri);
        if (resolved == null) {
            Log.w(TAG, "Couldn't resolve picked folder URI " + treeUri
                + " (probably a cloud or Downloads provider, which doesn't map "
                + "to a real /storage path). Keeping the fallback. Try a folder "
                + "under \"Internal storage\" or an SD card next time.");
            return;
        }
        Log.i(TAG, "Music folder picked: " + resolved);
        writeMusicLibraryFile(resolved);
    }

    /**
     * Convert a SAF tree URI from the external-storage DocumentsProvider into
     * the corresponding /storage filesystem path. We only support the
     * external-storage provider (which covers both /sdcard primary storage
     * and physical SD cards); other providers (Downloads, Drive, etc.)
     * return URIs that don't map cleanly to a path and would require
     * touchHLE to be rewritten on top of ContentResolver, which it isn't.
     *
     * Returns null if the URI is from an unsupported provider.
     */
    private static String resolveTreeUriToFilesystemPath(Uri uri) {
        if (!"com.android.externalstorage.documents".equals(uri.getAuthority())) {
            return null;
        }
        String docId;
        try {
            docId = DocumentsContract.getTreeDocumentId(uri);
        } catch (Exception e) {
            return null;
        }
        // docId is like "primary:Music/Sub" for internal storage or
        // "1234-5678:Music" for an SD card.
        int colon = docId.indexOf(':');
        String volume = colon < 0 ? docId : docId.substring(0, colon);
        String rel = colon < 0 ? "" : docId.substring(colon + 1);
        String volumePath = "primary".equalsIgnoreCase(volume)
            ? "/storage/emulated/0"
            : "/storage/" + volume;
        return rel.isEmpty() ? volumePath : volumePath + "/" + rel;
    }

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "touchHLE"
        };
    }
}
