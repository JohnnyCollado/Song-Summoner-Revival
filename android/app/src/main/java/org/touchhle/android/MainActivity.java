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
import android.system.Os;
import android.util.Log;
import android.widget.Toast;

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
 *      can live in a public folder (default /sdcard/touchHLE/, or
 *      /sdcard/SongSummoner/ in the wrapper flavor) instead of the
 *      Android/data sandbox.
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

    /** Default public user-data folder for the generic touchhle flavor.
     *  The wrapper flavor overrides this via BuildConfig.WRAPPER_USER_DATA_DIR. */
    private static final String DEFAULT_USER_DATA_DIR = "/sdcard/touchHLE";

    /** Env var read by paths.rs::resolve_android_user_data_path to override
     *  the default /sdcard/touchHLE location. Set from {@link #onCreate}
     *  before super so the Rust side picks it up on first lookup. */
    private static final String ENV_USER_DATA_BASE_PATH = "TOUCHHLE_USER_DATA_BASE_PATH";

    private static final int REQ_PICK_MUSIC_FOLDER = 1001;

    /** Resolved public user-data folder for this flavor. Cached in onCreate
     *  so the music-library path, album-placeholder targets and the
     *  TOUCHHLE_USER_DATA_BASE_PATH env var all agree. */
    private String userDataDir = DEFAULT_USER_DATA_DIR;

    /** Path where music_library.rs reads the chosen music folder from.
     *  Resolved from {@link #userDataDir} in onCreate. */
    private String musicLibraryFile =
        DEFAULT_USER_DATA_DIR + "/touchHLE_music_library.txt";

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
     * Absolute path to the wrapper IPA on public storage. Resolved once
     * in {@link #onCreate} (before super, so {@link #getArguments} can
     * return it as argv[1] when SDL fires up the native thread). Null in
     * the non-wrapper flavor or when the file is missing.
     */
    private String wrapperIpaPath = null;

    /**
     * Set to a user-facing reason ("IPA not found at ...") when the
     * wrapper flavor can't resolve its IPA. Shown as a Toast once
     * super.onCreate has set up the window. Null when everything's fine.
     */
    private String wrapperMissingIpaReason = null;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // CRITICAL: do wrapper setup BEFORE super.onCreate(). SDL's
        // SDLActivity.onCreate spins up the native thread which calls
        // getArguments() and starts the Rust main shortly after; if
        // wrapperIpaPath isn't set yet, touchHLE will fall back to the
        // app-picker, and if TOUCHHLE_USER_DATA_BASE_PATH isn't set yet,
        // paths.rs will cache the default /sdcard/touchHLE location and
        // never re-check.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH
                && BuildConfig.WRAPPER_USER_DATA_DIR != null
                && !BuildConfig.WRAPPER_USER_DATA_DIR.isEmpty()) {
            userDataDir = BuildConfig.WRAPPER_USER_DATA_DIR;
            musicLibraryFile = userDataDir + "/touchHLE_music_library.txt";
            // Make sure the folder exists so the user can drop the IPA in
            // even before granting MANAGE_EXTERNAL_STORAGE has fully
            // propagated -- best effort, ignore failure.
            //noinspection ResultOfMethodCallIgnored
            new File(userDataDir).mkdirs();
            // Tell the Rust side to use this folder for its user data.
            // Must be set before super.onCreate so getenv() on the native
            // thread sees it on first lookup.
            try {
                Os.setenv(ENV_USER_DATA_BASE_PATH, userDataDir, true);
                Log.i(TAG, "wrapper: " + ENV_USER_DATA_BASE_PATH + "=" + userDataDir);
            } catch (Exception e) {
                Log.w(TAG, "wrapper: couldn't set " + ENV_USER_DATA_BASE_PATH + ": " + e);
            }
            wrapperIpaPath = resolveExternalWrapperIpa();
        }
        // Extract the album-art placeholder bundled in assets to the
        // user-data folder so the Rust runtime can fs::read it. Safe to
        // call every launch -- it's a no-op when the file is already
        // present at the right size.
        ensureAlbumPlaceholderUnpacked();
        super.onCreate(savedInstanceState);
        // If the wrapper couldn't find its IPA, surface that as a Toast
        // now that the activity window is up. We still let SDL/touchHLE
        // start so the user can see something (the app picker as a
        // fallback) instead of an opaque black screen.
        if (wrapperMissingIpaReason != null) {
            final String msg = wrapperMissingIpaReason;
            new Handler(Looper.getMainLooper()).post(() ->
                Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
            );
        }
        new Handler(Looper.getMainLooper()).postDelayed(
            this::runFirstLaunchSetup,
            SETUP_PROMPT_DELAY_MS
        );
    }

    /**
     * In wrapper-flavor builds (BuildConfig.WRAPPER_AUTO_LAUNCH = true)
     * with the IPA present at {@link #wrapperIpaPath}, return that path
     * so touchHLE's Rust main skips the app picker and launches the
     * game directly.
     *
     * In the generic touchhle flavor -- or in the wrapper flavor when
     * the IPA is missing from the public user-data folder -- we return
     * {@code super.getArguments()} (an empty array), preserving the
     * "show app picker" behaviour.
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
     * Look for the wrapper's IPA at {@code <WRAPPER_USER_DATA_DIR>/<WRAPPER_IPA_FILENAME>}
     * on public storage. The user is expected to have copied it there
     * themselves (we no longer bundle the ~260 MB blob inside the APK).
     *
     * Returns the absolute path on success, or null if the file is missing.
     * On null, {@link #wrapperMissingIpaReason} is set to a user-facing
     * message that {@link #onCreate} will surface as a Toast.
     */
    private String resolveExternalWrapperIpa() {
        String fname = BuildConfig.WRAPPER_IPA_FILENAME;
        if (fname == null || fname.isEmpty()) {
            return null;
        }
        File ipa = new File(userDataDir, fname);
        if (ipa.isFile() && ipa.length() > 0) {
            Log.i(TAG, "wrapper: using IPA at " + ipa.getAbsolutePath()
                + " (" + ipa.length() + " bytes).");
            return ipa.getAbsolutePath();
        }
        wrapperMissingIpaReason =
            "Drop " + fname + " into " + userDataDir + " and relaunch.";
        Log.w(TAG, "wrapper: IPA not found at " + ipa.getAbsolutePath()
            + " -- falling back to app picker. " + wrapperMissingIpaReason);
        return null;
    }

    /**
     * Copy assets/album_placeholder.png into the user-data folder under
     * res/album_placeholder.png so touchHLE's Rust loader (which does a
     * plain {@code fs::read("res/album_placeholder.png")} keyed off
     * user_data_base_path on Android) can find it. We write to BOTH
     * candidate locations the Rust paths logic might resolve to:
     *   - <userDataDir>/res/      (preferred; only writable once the
     *                              user grants MANAGE_EXTERNAL_STORAGE)
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
        targets.add(new File(userDataDir + "/res/" + ASSET));
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
                // Permission-denied on the public user-data dir before
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
     * Returns true iff {@link #musicLibraryFile} exists, can be read, and
     * its trimmed contents name a directory that currently exists. Anything
     * else (file missing, empty, non-absolute path, deleted folder) counts
     * as "no valid path" and triggers a re-prompt.
     */
    private boolean isSavedMusicLibraryPathValid() {
        File libFile = new File(musicLibraryFile);
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
            Log.w(TAG, "Couldn't read " + musicLibraryFile + ": " + e);
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
        // CRITICAL: write a fallback path to musicLibraryFile *before*
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
     * return (and create) <userDataDir>/Music so there's a writable
     * destination for the user to drop files into later.
     *
     * Mirrors the Rust-side candidate set in
     * music_library.rs::prompt_for_folder so picker-side and Rust-side
     * fallbacks agree on which folder "wins".
     */
    private String pickBestCandidateMusicFolder() {
        String[] candidates = new String[] {
            "/sdcard/Music",
            userDataDir + "/Music",
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
        // Nothing useful found — still create <userDataDir>/Music so the
        // user has somewhere obvious to drop files later.
        File seed = new File(userDataDir + "/Music");
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

    /** Best-effort write of an absolute path into {@link #musicLibraryFile}.
     *  Logs (and swallows) any I/O error: the worst case is the prompt
     *  fires again next launch, which is no worse than before. */
    private void writeMusicLibraryFile(String absolutePath) {
        try {
            File parent = new File(musicLibraryFile).getParentFile();
            if (parent != null && !parent.isDirectory()) {
                parent.mkdirs();
            }
            try (FileOutputStream out = new FileOutputStream(musicLibraryFile)) {
                out.write(absolutePath.getBytes("UTF-8"));
            }
            Log.i(TAG, "Wrote music folder to " + musicLibraryFile + ": " + absolutePath);
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
