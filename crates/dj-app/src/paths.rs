//! Per-user application directory for config and library files.
//!
//! - Windows: `%APPDATA%\Kaldor`
//! - macOS: `~/Library/Application Support/Kaldor`
//! - Linux and other Unix: `$XDG_CONFIG_HOME/Kaldor`, else `~/.config/Kaldor`
//!
//! Before the rename the folder was called `djapp`. [`migrate`] renames it
//! once; until then (or when the rename fails) the old folder is used.

use std::path::{Path, PathBuf};

/// Folder name of the app directory.
const DIR_NAME: &str = "Kaldor";
/// Folder name used before the app was called Kaldor.
const LEGACY_DIR_NAME: &str = "djapp";

/// The directory, or `None` if the platform gives no home or app-data path.
pub fn app_dir() -> Option<PathBuf> {
    base_dir().map(|b| pick(&b))
}

/// Renames the old `djapp` folder to `Kaldor` if only the old one exists.
/// Called once when the UI starts (not by tests or command-line modes).
pub fn migrate() {
    let Some(base) = base_dir() else { return };
    let (new, old) = (base.join(DIR_NAME), base.join(LEGACY_DIR_NAME));
    if new.exists() || !old.is_dir() {
        return;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => tracing::info!("moved {} to {}", old.display(), new.display()),
        Err(e) => tracing::warn!(
            "could not rename {} to {} ({e}); using the old folder",
            old.display(),
            new.display()
        ),
    }
}

/// The new folder, unless only the old one exists.
fn pick(base: &Path) -> PathBuf {
    let new = base.join(DIR_NAME);
    let old = base.join(LEGACY_DIR_NAME);
    if !new.exists() && old.is_dir() {
        old
    } else {
        new
    }
}

#[cfg(windows)]
fn base_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn base_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

#[cfg(not(any(windows, unix)))]
fn base_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folder_is_used_until_the_new_one_exists() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-paths")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert_eq!(pick(&base), base.join("Kaldor"), "fresh install");
        std::fs::create_dir_all(base.join("djapp")).unwrap();
        assert_eq!(pick(&base), base.join("djapp"), "not migrated yet");
        std::fs::create_dir_all(base.join("Kaldor")).unwrap();
        assert_eq!(pick(&base), base.join("Kaldor"), "migrated");
        let _ = std::fs::remove_dir_all(&base);
    }
}
