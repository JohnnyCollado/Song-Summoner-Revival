/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
package org.touchhle.android;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.provider.Settings;
import android.util.Log;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Launcher activity that runs all one-time Android setup BEFORE touchHLE
 * starts, then hands off to {@link MainActivity}.
 *
 * This has to be a separate activity: touchHLE exits the whole process as
 * soon as it loses focus (uikit.rs's app-will-resign-active handler), so
 * any Settings page or SAF picker opened while SDL is running kills the
 * game and loses the picker's result. Here, nothing emulated is running
 * yet, so we can leave for Settings / the picker and come back normally.
 *
 * Steps, in order:
 *   1. Storage access: MANAGE_EXTERNAL_STORAGE on Android 11+, or the
 *      READ/WRITE_EXTERNAL_STORAGE runtime permissions on Android 6-10.
 *   2. Game IPA (wrapper flavor only): if the IPA isn't in the user-data
 *      folder yet, ask the user to pick their copy with the SAF file
 *      picker and copy it there under the name MainActivity expects.
 *   3. Music folder: if no valid folder is saved, open the SAF folder
 *      picker and write the chosen path to touchHLE_music_library.txt,
 *      which music_library.rs honours.
 *   4. Start MainActivity and finish.
 */
public class SetupActivity extends Activity {

    private static final String TAG = "touchHLE";
    private static final String PREFS_NAME = "touchHLE";
    private static final String PREF_DECLINED_STORAGE = "declined_storage_access";

    /** Default public user-data folder for the generic touchhle flavor. */
    private static final String DEFAULT_USER_DATA_DIR = "/sdcard/touchHLE";

    private static final int REQ_STORAGE_PERMISSIONS = 2001;
    private static final int REQ_PICK_MUSIC_FOLDER = 2002;
    private static final int REQ_PICK_IPA = 2003;

    /** Audio-file extensions counted by the fallback scanner. Must stay in
     *  sync with music_library.rs::SUPPORTED_EXTENSIONS. */
    private static final String[] AUDIO_EXTS = {
        ".mp3", ".m4a", ".aac", ".wav", ".flac", ".ogg", ".oga",
        ".caf", ".aif", ".aiff",
    };

    /** Public user-data folder for this flavor. Shared with MainActivity so
     *  both agree on where the IPA, options and music-library file live. */
    static String resolveUserDataDir() {
        if (BuildConfig.WRAPPER_AUTO_LAUNCH
                && BuildConfig.WRAPPER_USER_DATA_DIR != null
                && !BuildConfig.WRAPPER_USER_DATA_DIR.isEmpty()) {
            return BuildConfig.WRAPPER_USER_DATA_DIR;
        }
        return DEFAULT_USER_DATA_DIR;
    }

    private String userDataDir;
    private String musicLibraryFile;

    /** True while we're away in the All-files-access Settings page, so
     *  onResume knows to re-check and continue. */
    private boolean waitingForSettings = false;
    /** Guards against re-entering the flow while a dialog, permission
     *  request or picker is already outstanding. */
    private boolean busy = false;
    private boolean finished = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        userDataDir = resolveUserDataDir();
        musicLibraryFile = userDataDir + "/touchHLE_music_library.txt";
        advance();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (waitingForSettings) {
            waitingForSettings = false;
            busy = false;
            advance();
        }
    }

    /** Run the next outstanding setup step, or launch the game. */
    private void advance() {
        if (busy || finished) {
            return;
        }

        // Step 1: storage access.
        if (!hasStorageAccess() && !prefs().getBoolean(PREF_DECLINED_STORAGE, false)) {
            busy = true;
            showStorageDialog();
            return;
        }

        // Step 2: game IPA. Needs storage access to write into the public
        // user-data folder; without it MainActivity shows its own
        // "drop the IPA into ..." toast instead.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && hasStorageAccess() && !isWrapperIpaPresent()) {
            busy = true;
            showIpaDialog();
            return;
        }

        // Step 3: music folder. Without storage access the resolved path
        // wouldn't be readable anyway, so skip straight to launch.
        if (hasStorageAccess() && !isSavedMusicLibraryPathValid()) {
            busy = true;
            // The candidate scan can walk thousands of files; keep it off
            // the UI thread.
            new Thread(() -> {
                String fallback = pickBestCandidateMusicFolder();
                runOnUiThread(() -> openMusicFolderPicker(fallback));
            }, "touchHLE-music-scan").start();
            return;
        }

        launchGame();
    }

    private SharedPreferences prefs() {
        return getSharedPreferences(PREFS_NAME, MODE_PRIVATE);
    }

    // ---------------------------------------------------------------------
    // Step 1: storage access
    // ---------------------------------------------------------------------

    private static boolean needsRuntimeStoragePermission() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.M
            && Build.VERSION.SDK_INT < Build.VERSION_CODES.R;
    }

    private boolean hasStorageAccess() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            return Environment.isExternalStorageManager();
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            return checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE)
                    == PackageManager.PERMISSION_GRANTED
                && checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE)
                    == PackageManager.PERMISSION_GRANTED;
        }
        return true;
    }

    private void showStorageDialog() {
        String message = BuildConfig.WRAPPER_AUTO_LAUNCH
            ? BuildConfig.APP_NAME + " needs access to your files to load the game from "
                + userDataDir + " and to find your music."
            : BuildConfig.APP_NAME + " needs access to your files so your apps, settings "
                + "and music can live in " + userDataDir + ", where any file manager can "
                + "reach them.";
        new AlertDialog.Builder(this)
            .setTitle("Storage access")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Continue", (d, w) -> requestStorageAccess())
            .setNegativeButton(BuildConfig.WRAPPER_AUTO_LAUNCH ? "Exit" : "Not now", (d, w) -> {
                // The generic flavor works from its private folder, so
                // remember the choice. The wrapper can't find its IPA
                // without access, so keep asking on every launch.
                if (!BuildConfig.WRAPPER_AUTO_LAUNCH) {
                    prefs().edit().putBoolean(PREF_DECLINED_STORAGE, true).apply();
                }
                skipOrExit();
            })
            .show();
    }

    private void requestStorageAccess() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            waitingForSettings = true;
            if (!openAllFilesAccessSettings()) {
                Log.w(TAG, "No Settings page for MANAGE_EXTERNAL_STORAGE.");
                waitingForSettings = false;
                skipOrExit();
            }
        } else if (needsRuntimeStoragePermission()) {
            requestPermissions(new String[] {
                Manifest.permission.READ_EXTERNAL_STORAGE,
                Manifest.permission.WRITE_EXTERNAL_STORAGE,
            }, REQ_STORAGE_PERMISSIONS);
        } else {
            busy = false;
            advance();
        }
    }

    private boolean openAllFilesAccessSettings() {
        try {
            Intent intent = new Intent(
                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:" + getPackageName())
            );
            startActivity(intent);
            return true;
        } catch (Exception primary) {
            Log.w(TAG, "App-specific MANAGE_EXTERNAL_STORAGE page unavailable: " + primary);
            try {
                startActivity(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                return true;
            } catch (Exception secondary) {
                Log.w(TAG, "Generic page also unavailable: " + secondary);
                return false;
            }
        }
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
                                           int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode != REQ_STORAGE_PERMISSIONS) {
            return;
        }
        if (!hasStorageAccess()) {
            Log.i(TAG, "Storage permission denied.");
            showStorageDialog();
            return;
        }
        busy = false;
        advance();
    }

    // ---------------------------------------------------------------------
    // Step 2: game IPA (wrapper flavor)
    // ---------------------------------------------------------------------

    private File wrapperIpaFile() {
        return new File(userDataDir, BuildConfig.WRAPPER_IPA_FILENAME);
    }

    private boolean isWrapperIpaPresent() {
        File ipa = wrapperIpaFile();
        return ipa.isFile() && ipa.length() > 0;
    }

    private void showIpaDialog() {
        new AlertDialog.Builder(this)
            .setTitle("Game file needed")
            .setMessage(BuildConfig.APP_NAME + " needs your copy of the game's .ipa file. "
                + "Select it and it will be copied to " + userDataDir + ".")
            .setCancelable(false)
            .setPositiveButton("Select file", (d, w) -> openIpaPicker())
            .setNegativeButton("Exit", (d, w) -> skipOrExit())
            .show();
    }

    private void openIpaPicker() {
        try {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            // .ipa has no registered MIME type; providers report it as
            // application/octet-stream, application/zip or something else
            // entirely, so accept anything and validate the contents.
            intent.setType("*/*");
            startActivityForResult(intent, REQ_PICK_IPA);
        } catch (Exception e) {
            Log.w(TAG, "Couldn't launch ACTION_OPEN_DOCUMENT: " + e);
            showIpaError("Couldn't open the file picker on this device.");
        }
    }

    private void onIpaPicked(Uri uri) {
        String name = queryDisplayName(uri);
        if (name != null && !name.toLowerCase().endsWith(".ipa")) {
            showIpaError("\"" + name + "\" doesn't look like an .ipa file.");
            return;
        }
        final long size = querySize(uri);

        ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        bar.setMax(1000);
        bar.setIndeterminate(size <= 0);
        TextView label = new TextView(this);
        label.setText("Copying game file...");
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        int pad = (int) (24 * getResources().getDisplayMetrics().density);
        layout.setPadding(pad, pad, pad, pad);
        layout.addView(label);
        layout.addView(bar);
        AlertDialog progress = new AlertDialog.Builder(this)
            .setTitle("Importing game")
            .setView(layout)
            .setCancelable(false)
            .show();

        new Thread(() -> {
            String error = copyIpa(uri, size, (done) -> runOnUiThread(() -> {
                bar.setProgress((int) (done * 1000 / size));
                label.setText("Copying game file... " + (done >> 20) + " / "
                    + (size >> 20) + " MB");
            }));
            runOnUiThread(() -> {
                progress.dismiss();
                if (error != null) {
                    showIpaError(error);
                } else {
                    busy = false;
                    advance();
                }
            });
        }, "touchHLE-ipa-import").start();
    }

    private interface ProgressCallback {
        void onProgress(long bytesCopied);
    }

    /**
     * Copy the picked document to the wrapper IPA path via a temporary
     * ".part" file, so an interrupted copy never leaves a truncated IPA
     * that MainActivity would try to boot. Returns null on success or a
     * user-facing error message.
     */
    private String copyIpa(Uri uri, long size, ProgressCallback progress) {
        File dest = wrapperIpaFile();
        File part = new File(dest.getPath() + ".part");
        File parent = dest.getParentFile();
        if (parent != null) {
            //noinspection ResultOfMethodCallIgnored
            parent.mkdirs();
        }
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(part)) {
            if (in == null) {
                throw new IOException("the file provider returned no data");
            }
            byte[] buf = new byte[1 << 20];
            long done = 0;
            long lastReport = 0;
            boolean first = true;
            int n;
            while ((n = in.read(buf)) > 0) {
                // An IPA is a zip archive, so it must start with "PK".
                if (first) {
                    first = false;
                    if (n < 2 || buf[0] != 'P' || buf[1] != 'K') {
                        throw new NotAnIpaException();
                    }
                }
                out.write(buf, 0, n);
                done += n;
                if (size > 0 && done - lastReport >= (4 << 20)) {
                    lastReport = done;
                    progress.onProgress(done);
                }
            }
        } catch (IOException e) {
            //noinspection ResultOfMethodCallIgnored
            part.delete();
            Log.e(TAG, "IPA import failed: " + e);
            return e instanceof NotAnIpaException
                ? "That file isn't a valid .ipa (it's not a zip archive)."
                : "Couldn't copy the file: " + e.getMessage();
        }
        //noinspection ResultOfMethodCallIgnored
        dest.delete();
        if (!part.renameTo(dest)) {
            //noinspection ResultOfMethodCallIgnored
            part.delete();
            return "Couldn't move the copied file into place at " + dest.getPath() + ".";
        }
        Log.i(TAG, "Imported IPA to " + dest.getAbsolutePath()
            + " (" + dest.length() + " bytes).");
        return null;
    }

    private static class NotAnIpaException extends IOException {
    }

    private void showIpaError(String message) {
        new AlertDialog.Builder(this)
            .setTitle("Couldn't import game")
            .setMessage(message)
            .setCancelable(false)
            .setPositiveButton("Try again", (d, w) -> openIpaPicker())
            .setNegativeButton("Exit", (d, w) -> skipOrExit())
            .show();
    }

    private String queryDisplayName(Uri uri) {
        try (Cursor c = getContentResolver().query(
                uri, new String[] { OpenableColumns.DISPLAY_NAME }, null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) {
                return c.getString(0);
            }
        } catch (Exception e) {
            Log.w(TAG, "Couldn't query display name for " + uri + ": " + e);
        }
        return null;
    }

    private long querySize(Uri uri) {
        try (Cursor c = getContentResolver().query(
                uri, new String[] { OpenableColumns.SIZE }, null, null, null)) {
            if (c != null && c.moveToFirst() && !c.isNull(0)) {
                return c.getLong(0);
            }
        } catch (Exception e) {
            Log.w(TAG, "Couldn't query size for " + uri + ": " + e);
        }
        return -1;
    }

    // ---------------------------------------------------------------------
    // Step 3: music folder
    // ---------------------------------------------------------------------

    /**
     * Returns true iff {@link #musicLibraryFile} exists and its first line
     * names a directory that currently exists.
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
            return !trimmed.isEmpty() && new File(trimmed).isDirectory();
        } catch (Exception e) {
            Log.w(TAG, "Couldn't read " + musicLibraryFile + ": " + e);
            return false;
        }
    }

    private void openMusicFolderPicker(String fallback) {
        if (finished) {
            return;
        }
        // Save the best auto-detected folder first, so cancelling the
        // picker (or the process dying) still leaves a working library
        // and we don't re-prompt next launch.
        writeMusicLibraryFile(fallback);
        try {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Uri musicHint = Uri.parse(
                    "content://com.android.externalstorage.documents/document/primary%3AMusic"
                );
                intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, musicHint);
            }
            Log.i(TAG, "No valid music folder set yet -- opening SAF folder picker.");
            startActivityForResult(intent, REQ_PICK_MUSIC_FOLDER);
        } catch (Exception e) {
            Log.w(TAG, "Couldn't launch ACTION_OPEN_DOCUMENT_TREE: " + e);
            launchGame();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQ_PICK_IPA) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                onIpaPicked(data.getData());
            } else {
                Log.i(TAG, "IPA picker cancelled.");
                showIpaDialog();
            }
            return;
        }
        if (requestCode != REQ_PICK_MUSIC_FOLDER) {
            return;
        }
        if (resultCode == RESULT_OK && data != null && data.getData() != null) {
            Uri treeUri = data.getData();
            String resolved = resolveTreeUriToFilesystemPath(treeUri);
            if (resolved != null) {
                Log.i(TAG, "Music folder picked: " + resolved);
                writeMusicLibraryFile(resolved);
            } else {
                Log.w(TAG, "Couldn't resolve picked folder URI " + treeUri
                    + " (probably a cloud or Downloads provider, which doesn't map "
                    + "to a real /storage path). Keeping the auto-detected folder.");
            }
        } else {
            Log.i(TAG, "Music folder picker cancelled; keeping the auto-detected folder.");
        }
        launchGame();
    }

    /**
     * Choose the music folder to pre-seed before opening SAF: the
     * well-known location with the most audio files, or (if none have
     * any) a freshly created <userDataDir>/Music.
     *
     * Mirrors the Rust-side candidate set in
     * music_library.rs::prompt_for_folder so both sides agree.
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
            // Strictly greater so earlier candidates win ties -- matches Rust.
            if (n > bestCount) {
                best = raw;
                bestCount = n;
            }
        }
        if (best != null) {
            return best;
        }
        File seed = new File(userDataDir + "/Music");
        //noinspection ResultOfMethodCallIgnored
        seed.mkdirs();
        return seed.getAbsolutePath();
    }

    /** Recursive count of audio files under {@code dir}, capped in depth and
     *  total count like the Rust-side count_audio_files_recursive. */
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

    /** Best-effort write of an absolute path into {@link #musicLibraryFile}. */
    private void writeMusicLibraryFile(String absolutePath) {
        try {
            File parent = new File(musicLibraryFile).getParentFile();
            if (parent != null && !parent.isDirectory()) {
                //noinspection ResultOfMethodCallIgnored
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

    /**
     * Convert a SAF tree URI from the external-storage DocumentsProvider into
     * the corresponding /storage filesystem path. Other providers (Downloads,
     * Drive, etc.) don't map to a real path and return null.
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
            ? Environment.getExternalStorageDirectory().getAbsolutePath()
            : "/storage/" + volume;
        return rel.isEmpty() ? volumePath : volumePath + "/" + rel;
    }

    // ---------------------------------------------------------------------
    // Step 4: hand off to touchHLE
    // ---------------------------------------------------------------------

    /**
     * The user declined a setup step. The generic flavor can still run
     * (touchHLE shows its app picker); the wrapper can't do anything useful
     * without storage access and the IPA, and must never expose the bare
     * touchHLE engine, so it closes instead.
     */
    private void skipOrExit() {
        if (BuildConfig.WRAPPER_AUTO_LAUNCH) {
            exitApp();
        } else {
            launchGame();
        }
    }

    private void exitApp() {
        finished = true;
        finishAndRemoveTask();
    }

    private void launchGame() {
        if (finished) {
            return;
        }
        // Belt and braces: the wrapper only ever starts touchHLE with its
        // IPA in place, so users never land in the engine's app picker.
        if (BuildConfig.WRAPPER_AUTO_LAUNCH && !isWrapperIpaPresent()) {
            Log.w(TAG, "wrapper: refusing to start touchHLE without the IPA.");
            exitApp();
            return;
        }
        finished = true;
        startActivity(new Intent(this, MainActivity.class));
        finish();
    }
}
