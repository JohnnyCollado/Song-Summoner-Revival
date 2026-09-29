/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Build helper behind the `cargo dist-windows` / `cargo debug-windows`
//! aliases in `.cargo/config.toml`.
//!
//! Stable Cargo can't put a binary in a folder of our choosing
//! (`--artifact-dir` is nightly-only), and pointing `CARGO_TARGET_DIR` at
//! `dist/` would fill it with gigabytes of intermediates. So this builds
//! touchHLE normally, then assembles a runnable folder from the result:
//!
//! - `cargo dist-windows`  -> release build in `<repo>/dist/windows/`
//! - `cargo debug-windows` -> debug build in `<repo>/debug/windows/`
//!
//! The IPA is never copied: users supply their own (see CLAUDE.md).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Single files from the repo root that go next to the exe. CREDITS.txt
/// carries the credits and the Square Enix notice, readable without
/// starting the game.
const SHIPPED_FILES: &[&str] = &["touchHLE_default_options.txt", "CREDITS.txt"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let release = match args.as_slice() {
        ["windows", "--release"] => true,
        ["windows"] => false,
        _ => {
            eprintln!("usage: cargo dist-windows | cargo debug-windows");
            return ExitCode::FAILURE;
        }
    };
    match build_windows(release) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_windows(release: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let profile = if release { "release" } else { "debug" };

    // Use the same cargo that ran us, so toolchain overrides carry over.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&root)
        .args(["build", "--package", "touchHLE", "--bin", "touchHLE_bin"]);
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("couldn't run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build failed ({status})"));
    }

    // Same lookup the old dev-scripts/stage-dist.ps1 used.
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => root.join(PathBuf::from(dir)),
        None => root.join("target"),
    };
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    // The bin target is touchHLE_bin (see the root Cargo.toml for why);
    // users get it under the familiar touchHLE.exe name.
    let exe = target_dir
        .join(profile)
        .join(format!("touchHLE_bin{exe_suffix}"));
    let exe_name = format!("touchHLE{exe_suffix}");

    let out = root
        .join(if release { "dist" } else { "debug" })
        .join("windows");
    // Deliberately not wiped first: people run the game from this folder,
    // so it can hold their IPA, saves (touchHLE_sandbox) and edited
    // touchHLE_options.txt. We only overwrite the files we ship.
    std::fs::create_dir_all(&out).map_err(|e| io_err("create", &out, e))?;

    copy_file(&exe, &out.join(&exe_name))?;
    copy_dir(&root.join("touchHLE_dylibs"), &out.join("touchHLE_dylibs"))?;
    copy_dir(&root.join("touchHLE_fonts"), &out.join("touchHLE_fonts"))?;
    for name in SHIPPED_FILES {
        copy_file(&root.join(name), &out.join(name))?;
    }
    // touchHLE_options.txt and OPTIONS_HELP.txt are left out on purpose:
    // paths.rs writes them on first launch if missing, and copying them
    // would clobber the user's edits on every build.

    // Runtime images the engine loads relative to its working directory:
    // the virtual-cursor sprites (window.rs), under either naming
    // convention.
    let res_out = out.join("res");
    std::fs::create_dir_all(&res_out).map_err(|e| io_err("create", &res_out, e))?;
    let res_in = root.join("res");
    let entries = std::fs::read_dir(&res_in).map_err(|e| io_err("read", &res_in, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read", &res_in, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let wanted = name.ends_with(".png")
            && (name.starts_with("cursor_") || name.starts_with("Cursor "));
        if wanted {
            copy_file(&entry.path(), &res_out.join(&name))?;
        }
    }

    println!("Windows {profile} build ready in {}", out.display());
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("couldn't copy {} to {}: {e}", from.display(), to.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| io_err("create", to, e))?;
    let entries = std::fs::read_dir(from).map_err(|e| io_err("read", from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read", from, e))?;
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            copy_file(&entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> String {
    format!("couldn't {what} {}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credits_ship_with_the_game() {
        assert!(SHIPPED_FILES.contains(&"CREDITS.txt"));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for name in SHIPPED_FILES {
            assert!(root.join(name).is_file(), "{name} is missing");
        }
    }

    #[test]
    fn the_game_never_ships() {
        assert!(!SHIPPED_FILES.iter().any(|f| f.to_lowercase().ends_with(".ipa")));
    }
}
