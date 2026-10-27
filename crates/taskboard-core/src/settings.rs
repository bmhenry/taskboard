//! User settings. Global settings apply to every project and live in
//! `$XDG_CONFIG_HOME/tb/settings.json` (falling back to `~/.config`); project
//! settings live in the project's `.tb/settings.json`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::Status;
use crate::{Error, Result};

pub const MIN_FONT_SIZE: u16 = 10;
pub const MAX_FONT_SIZE: u16 = 28;
pub const DEFAULT_FONT_SIZE: u16 = 14;

/// The GUI's color scheme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    /// Size of normal body text in the GUI; everything else scales with it.
    pub font_size: u16,
    /// Draw board columns for statuses with no tasks as thin strips.
    pub collapse_empty_columns: bool,
    /// Light or dark colors in the GUI.
    pub theme: Theme,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            font_size: DEFAULT_FONT_SIZE,
            collapse_empty_columns: true,
            theme: Theme::Dark,
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
        load_json(path).map(GlobalSettings::clamped)
    }

    /// Writes settings to `path` atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(self, path)
    }

    /// Pulls out-of-range values (e.g. from a hand-edited file) into range.
    pub fn clamped(mut self) -> GlobalSettings {
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self
    }
}

/// Settings for one project, kept in its `.tb/settings.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// Board columns the user collapsed (`true`) or expanded (`false`) by
    /// hand. Columns not listed follow `GlobalSettings::collapse_empty_columns`.
    pub collapsed_columns: BTreeMap<Status, bool>,
}

impl ProjectSettings {
    /// Reads settings from `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<ProjectSettings> {
        load_json(path)
    }

    /// Writes settings to `path` atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(self, path)
    }
}

fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Error::Invalid(format!("invalid {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_json<T: Serialize>(value: &T, path: &Path) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut tmp, value)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
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
            theme: Theme::Light,
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
        // Files from before the theme setting keep loading, as dark.
        fs::write(
            &path,
            r#"{"font_size": 16, "collapse_empty_columns": false}"#,
        )
        .unwrap();
        assert_eq!(GlobalSettings::load(&path).unwrap().theme, Theme::Dark);
        fs::write(&path, r#"{"font_size": 300}"#).unwrap();
        assert_eq!(
            GlobalSettings::load(&path).unwrap().font_size,
            MAX_FONT_SIZE
        );
    }

    #[test]
    fn project_settings_round_trip_with_status_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = ProjectSettings::default();
        s.collapsed_columns.insert(Status::InProgress, true);
        s.save(&path).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"in_progress\": true")
        );
        assert_eq!(ProjectSettings::load(&path).unwrap(), s);
    }
}
