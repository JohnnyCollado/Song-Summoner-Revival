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
import android.os.Bundle;
import android.system.Os;
import android.util.Log;

import org.libsdl.app.SDLActivity;
import org.touchhle.android.BuildConfig;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

/**
 * A wrapper class over SDLActivity that points touchHLE at its public
 * user-data folder and, in the wrapper flavor, at the game IPA.
 *
 * All interactive setup (storage permission, music-folder picker) happens
 * earlier, in {@link SetupActivity}: touchHLE exits the process whenever it
 * loses focus, so nothing here may open another activity.
 */
public class MainActivity extends SDLActivity {

    private static final String TAG = "touchHLE";

    /** Env var read by paths.rs::resolve_android_user_data_path to override
     *  the default /sdcard/touchHLE location. Set from {@link #onCreate}
     *  before super so the Rust side picks it up on first lookup. */
    private static final String ENV_USER_DATA_BASE_PATH = "TOUCHHLE_USER_DATA_BASE_PATH";

    /** Resolved public user-data folder for this flavor. */
    private String userDataDir = SetupActivity.resolveUserDataDir();

    /**
     * Absolute path to the wrapper IPA on public storage. Resolved once
     * in {@link #onCreate} (before super, so {@link #getArguments} can
     * return it as argv[1] when SDL fires up the native thread). Null in
     * the non-wrapper flavor or when the file is missing.
     */
    private String wrapperIpaPath = null;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // CRITICAL: do wrapper setup BEFORE super.onCreate(). SDL's
        // SDLActivity.onCreate spins up the native thread which calls
        // getArguments() and starts the Rust main shortly after; if
        // wrapperIpaPath isn't set yet, touchHLE will fall back to the
        // app-picker, and if TOUCHHLE_USER_DATA_BASE_PATH isn't set yet,
        // paths.rs will cache the default /sdcard/touchHLE location and
        // never re-check.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            //noinspection ResultOfMethodCallIgnored
            new File(userDataDir).mkdirs();
            try {
                Os.setenv(ENV_USER_DATA_BASE_PATH, userDataDir, true);
                Log.i(TAG, "wrapper: " + ENV_USER_DATA_BASE_PATH + "=" + userDataDir);
            } catch (Exception e) {
                Log.w(TAG, "wrapper: couldn't set " + ENV_USER_DATA_BASE_PATH + ": " + e);
            }
            wrapperIpaPath = resolveExternalWrapperIpa();
            if (wrapperIpaPath == null) {
                // Never show the bare touchHLE engine (its app picker) in
                // the wrapper: send the user back through setup, which
                // imports the IPA. SDL only starts its native thread once
                // the activity resumes, so finishing straight after
                // super.onCreate (which Android requires us to call) never
                // starts touchHLE.
                startActivity(new Intent(this, SetupActivity.class));
                super.onCreate(savedInstanceState);
                finish();
                return;
            }
        }
        // Extract the album-art placeholder bundled in assets to the
        // user-data folder so the Rust runtime can fs::read it. Safe to
        // call every launch -- it's a no-op when the file is already
        // present at the right size.
        ensureAlbumPlaceholderUnpacked();
        super.onCreate(savedInstanceState);
    }

    /**
     * In wrapper-flavor builds (BuildConfig.WRAPPER_AUTO_LAUNCH = true)
     * with the IPA present at {@link #wrapperIpaPath}, return that path
     * so touchHLE's Rust main skips the app picker and launches the
     * game directly.
     *
     * In the generic touchhle flavor we return {@code super.getArguments()}
     * (an empty array), preserving the "show app picker" behaviour. The
     * wrapper never gets here without its IPA: onCreate bounces it back to
     * SetupActivity instead.
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
        Log.w(TAG, "wrapper: IPA not found at " + ipa.getAbsolutePath()
            + " -- returning to setup.");
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

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "touchHLE"
        };
    }
}
