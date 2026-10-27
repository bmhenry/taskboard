//! User settings. Global settings apply to every project and live in
//! `$XDG_CONFIG_HOME/tb/settings.json` (falling back to `~/.config`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MIN_FONT_SIZE: u16 = 10;
pub const MAX_FONT_SIZE: u16 = 28;
pub const DEFAULT_FONT_SIZE: u16 = 14;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    /// Size of normal body text in the GUI; everything else scales with it.
    pub font_size: u16,
    /// Draw board columns for statuses with no tasks as thin strips.
    pub collapse_empty_columns: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            font_size: DEFAULT_FONT_SIZE,
            collapse_empty_columns: true,
        }
    }
}

impl GlobalSettings {
    /// `$XDG_CONFIG_HOME/tb/settings.json`, or `~/.config/tb/settings.json`.
    /// `None` if neither variable is set.
    pub fn default_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("tb").join("settings.json"))
    }

    /// Reads settings from `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<GlobalSettings> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<GlobalSettings>(&text)
                .map(GlobalSettings::clamped)
                .map_err(|e| Error::Invalid(format!("invalid {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GlobalSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes settings to `path` atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, self)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// Pulls out-of-range values (e.g. from a hand-edited file) into range.
    pub fn clamped(mut self) -> GlobalSettings {
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tb/settings.json");
        assert_eq!(
            GlobalSettings::load(&path).unwrap(),
            GlobalSettings::default()
        );

        let s = GlobalSettings {
            font_size: 18,
            collapse_empty_columns: false,
        };
        s.save(&path).unwrap();
        assert_eq!(GlobalSettings::load(&path).unwrap(), s);
    }

    #[test]
    fn unknown_and_missing_fields_and_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"later": true}"#).unwrap();
        assert_eq!(
            GlobalSettings::load(&path).unwrap(),
            GlobalSettings::default()
        );
        fs::write(&path, r#"{"font_size": 300}"#).unwrap();
        assert_eq!(
            GlobalSettings::load(&path).unwrap().font_size,
            MAX_FONT_SIZE
        );
    }
}
