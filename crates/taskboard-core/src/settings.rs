//! User settings. [`GlobalSettings`] apply to every project, are shared by
//! the GUI and `tb`, and live in `$XDG_CONFIG_HOME/tb/settings.json`
//! (falling back to `~/.config`). [`DeviceSettings`] are how the GUI looks on
//! one device, so a phone can use a bigger font than the desktop.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Ulid;
use crate::hooks::Hook;
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

/// Where a new task goes in its status column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewTaskPosition {
    #[default]
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalSettings {
    /// Draw board columns for statuses with no tasks as thin strips.
    pub collapse_empty_columns: bool,
    /// Whether tasks created here or with tb go at the top or bottom of
    /// their column.
    pub new_task_position: NewTaskPosition,
    /// Commands run when tasks are created or moved, in every project.
    pub hooks: Vec<Hook>,
    /// Board columns the user collapsed (`true`) or expanded (`false`) by
    /// hand. Columns not listed follow `collapse_empty_columns`.
    pub collapsed_columns: BTreeMap<Status, bool>,
    /// Epics whose open and backlog children the board leaves out. Only how
    /// the board looks: the children are still there for `tb` and the list.
    pub collapsed_epics: BTreeSet<Ulid>,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            collapse_empty_columns: true,
            new_task_position: NewTaskPosition::Top,
            hooks: Vec::new(),
            collapsed_columns: BTreeMap::new(),
            collapsed_epics: BTreeSet::new(),
        }
    }
}

impl GlobalSettings {
    /// `$XDG_CONFIG_HOME/tb/settings.json`, or `~/.config/tb/settings.json`.
    /// `None` if neither variable is set.
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::config_dir()?.join("settings.json"))
    }

    /// Reads settings from `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<GlobalSettings> {
        load_json(path)
    }

    /// Writes settings to `path` atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(self, path)
    }
}

/// How the GUI looks on one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    /// Size of normal body text in the GUI; everything else scales with it.
    pub font_size: u16,
    /// Light or dark colors in the GUI.
    pub theme: Theme,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        DeviceSettings {
            font_size: DEFAULT_FONT_SIZE,
            theme: Theme::Dark,
        }
    }
}

impl DeviceSettings {
    /// `$XDG_CONFIG_HOME/tb/device.json`, or `~/.config/tb/device.json`.
    /// `None` if neither variable is set.
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::config_dir()?.join("device.json"))
    }

    /// Reads settings from `path`. Until it exists, they're taken from the
    /// [`GlobalSettings`] file at `global`, which held them before they were
    /// split out, and saved to `path` so they outlive that file's next save.
    pub fn load(path: &Path, global: Option<&Path>) -> Result<DeviceSettings> {
        if path.exists() {
            return load_json(path).map(DeviceSettings::clamped);
        }
        let Some(global) = global else {
            return Ok(DeviceSettings::default());
        };
        let old = load_json::<DeviceSettings>(global)?.clamped();
        if old != DeviceSettings::default() {
            // If this can't be written, likely neither can `global`, so the
            // old values stay there to move next time.
            let _ = old.save(path);
        }
        Ok(old)
    }

    /// Writes settings to `path` atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(self, path)
    }

    /// Pulls out-of-range values (e.g. from a hand-edited file) into range.
    pub fn clamped(mut self) -> DeviceSettings {
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self
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
    use crate::hooks::HookEvent;

    #[test]
    fn missing_file_is_default_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tb/settings.json");
        assert_eq!(
            GlobalSettings::load(&path).unwrap(),
            GlobalSettings::default()
        );

        let s = GlobalSettings {
            collapse_empty_columns: false,
            new_task_position: NewTaskPosition::Bottom,
            hooks: vec![Hook {
                event: HookEvent::Moved,
                command: "notify-send moved".into(),
                pass_task_id: true,
            }],
            collapsed_columns: BTreeMap::from([(Status::InProgress, true)]),
            collapsed_epics: BTreeSet::from([Ulid::from(7u128)]),
        };
        s.save(&path).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"in_progress\": true")
        );
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
        // Files from before the new task position setting load it as top.
        fs::write(
            &path,
            r#"{"font_size": 16, "collapse_empty_columns": false}"#,
        )
        .unwrap();
        assert_eq!(
            GlobalSettings::load(&path).unwrap().new_task_position,
            NewTaskPosition::Top
        );
        fs::write(&path, r#"{"new_task_position": "bottom"}"#).unwrap();
        assert_eq!(
            GlobalSettings::load(&path).unwrap().new_task_position,
            NewTaskPosition::Bottom
        );
    }

    #[test]
    fn device_settings_round_trip_and_clamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        let global = dir.path().join("settings.json");
        assert_eq!(
            DeviceSettings::load(&path, Some(&global)).unwrap(),
            DeviceSettings::default()
        );
        // With nothing to move over, nothing is written.
        assert!(!path.exists());

        let s = DeviceSettings {
            font_size: 18,
            theme: Theme::Light,
        };
        s.save(&path).unwrap();
        assert_eq!(DeviceSettings::load(&path, Some(&global)).unwrap(), s);
        fs::write(&path, r#"{"font_size": 300}"#).unwrap();
        let loaded = DeviceSettings::load(&path, Some(&global)).unwrap();
        assert_eq!(loaded.font_size, MAX_FONT_SIZE);
        // Files from before the theme setting load it as dark.
        assert_eq!(loaded.theme, Theme::Dark);
    }

    #[test]
    fn device_settings_move_out_of_the_global_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        let global = dir.path().join("settings.json");
        // As the GUI saved it before the split.
        fs::write(
            &global,
            r#"{"font_size": 18, "theme": "light", "new_task_position": "bottom"}"#,
        )
        .unwrap();
        let want = DeviceSettings {
            font_size: 18,
            theme: Theme::Light,
        };
        assert_eq!(DeviceSettings::load(&path, Some(&global)).unwrap(), want);
        let settings = GlobalSettings::load(&global).unwrap();
        assert_eq!(settings.new_task_position, NewTaskPosition::Bottom);

        // Saving global settings drops the old values, but they were kept.
        settings.save(&global).unwrap();
        assert!(!fs::read_to_string(&global).unwrap().contains("font_size"));
        assert_eq!(DeviceSettings::load(&path, Some(&global)).unwrap(), want);
    }
}
