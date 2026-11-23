//! Loading and saving the TOML config file.

use anyhow::{Context, Result};
use dj_core::AppConfig;
use std::path::{Path, PathBuf};

/// Default config file location, for example `%APPDATA%\Kaldor\config.toml`
/// on Windows.
pub fn default_path() -> Option<PathBuf> {
    crate::paths::app_dir().map(|d| d.join("config.toml"))
}

/// Loads the config from `path` (or the default path). A missing file gives
/// the defaults; a malformed file is an error, so settings are never lost
/// silently.
pub fn load(path: Option<&Path>) -> Result<AppConfig> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => match default_path() {
            Some(p) => p,
            None => return Ok(AppConfig::default()),
        },
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn parse(text: &str) -> Result<AppConfig> {
    Ok(toml::from_str(text)?)
}

/// Copies an existing config file to `<name>.bak` once (when there is no
/// backup yet), before this version saves it. Older versions cannot read
/// key bindings added later, so the backup lets you go back.
pub fn backup_once(path: &Path) -> Result<Option<PathBuf>> {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    if !path.exists() || bak.exists() {
        return Ok(None);
    }
    std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    Ok(Some(bak))
}

/// Saves the config, creating the directory if needed.
pub fn save(path: &Path, cfg: &AppConfig) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(cfg)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_routing_preset() {
        let cfg = parse(
            "[audio.routing]\nmaster = { left = 0, right = 1 }\ncue = { left = 2, right = 3 }\n",
        )
        .unwrap();
        assert_eq!(cfg.audio.routing, dj_core::OutputRouting::ns4fx());
    }

    #[test]
    fn malformed_is_an_error() {
        assert!(parse("[audio\n").is_err());
    }
}

#[cfg(test)]
mod backup_tests {
    use super::*;

    #[test]
    fn backup_is_made_once() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-config-backup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        // No file yet: nothing to back up.
        assert!(backup_once(&cfg).unwrap().is_none());
        std::fs::write(&cfg, "[ui]\ndark_theme = false\n").unwrap();
        let bak = backup_once(&cfg).unwrap().expect("backup made");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "[ui]\ndark_theme = false\n"
        );
        // Never overwritten.
        std::fs::write(&cfg, "[ui]\ndark_theme = true\n").unwrap();
        assert!(backup_once(&cfg).unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "[ui]\ndark_theme = false\n"
        );
    }
}
