//! The desktop [`Platform`]: the task store on the local filesystem, native
//! file dialogs, and the system's browser.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iced::Subscription;
use taskboard_core::ops::{self, Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, DeviceSettings, GlobalSettings, Store, Ulid};
use taskboard_ui::{BoxFuture, PathKind, Platform, WatchEvent};

use crate::watch;

#[derive(Clone)]
pub struct Desktop {
    store: Store,
    /// Where settings are saved; `None` keeps them in memory only.
    settings_path: Option<PathBuf>,
    /// The settings last loaded or saved, which commands run hooks from.
    settings: Arc<Mutex<GlobalSettings>>,
    /// Where device settings are saved; `None` keeps them in memory only.
    device_path: Option<PathBuf>,
}

impl Desktop {
    pub fn new(
        store: Store,
        settings_path: Option<PathBuf>,
        device_path: Option<PathBuf>,
    ) -> Desktop {
        Desktop {
            store,
            settings_path,
            settings: Arc::default(),
            device_path,
        }
    }

    /// Reads settings, as [`Platform::load_settings`] does but without
    /// waiting, for starting up.
    pub fn load_settings_now(&self) -> Result<GlobalSettings, String> {
        let loaded = match &self.settings_path {
            Some(path) => GlobalSettings::load(path).map_err(|e| e.to_string())?,
            None => GlobalSettings::default(),
        };
        *self.settings() = loaded.clone();
        Ok(loaded)
    }

    /// Reads device settings, as [`Platform::load_device_settings`] does
    /// but without waiting, for starting up.
    pub fn load_device_settings_now(&self) -> Result<DeviceSettings, String> {
        match &self.device_path {
            Some(path) => {
                DeviceSettings::load(path, self.settings_path.as_deref()).map_err(|e| e.to_string())
            }
            None => Ok(DeviceSettings::default()),
        }
    }

    fn settings(&self) -> std::sync::MutexGuard<'_, GlobalSettings> {
        // Settings are only ever replaced whole, so a panic can't leave
        // them half changed.
        self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The store does its work as the futures here are polled, on iced's
/// executor rather than the UI thread.
impl Platform for Desktop {
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>> {
        let store = self.store.clone();
        let settings = self.settings().clone();
        Box::pin(async move { ops::execute(&store, &settings, command).map_err(|e| e.to_string()) })
    }

    fn load_board(&self) -> BoxFuture<Result<Board, String>> {
        let store = self.store.clone();
        Box::pin(async move { store.load().map_err(|e| e.to_string()) })
    }

    fn read_file(&self, _task: Ulid, file: &Attachment) -> BoxFuture<Result<Vec<u8>, String>> {
        let path = self.store.file_path(file);
        Box::pin(async move { std::fs::read(path).map_err(|e| e.to_string()) })
    }

    fn board_changes(&self) -> Subscription<WatchEvent> {
        watch::subscription(self.store.dir(), &self.store.tasks_path())
    }

    /// rfd gives `None` both when cancelled and when there's no portal (or
    /// zenity) to show a dialog, so the latter fails silently.
    fn pick_file(&self) -> BoxFuture<Option<FileSource>> {
        Box::pin(async {
            let file = rfd::AsyncFileDialog::new()
                .set_title("Attach a file")
                .pick_file()
                .await?;
            Some(FileSource::Path {
                path: file.path().to_path_buf(),
            })
        })
    }

    fn pick_folder(&self, title: &str) -> BoxFuture<Option<PathBuf>> {
        let title = title.to_string();
        Box::pin(async move {
            rfd::AsyncFileDialog::new()
                .set_title(title)
                .pick_folder()
                .await
                .map(|f| f.path().to_path_buf())
        })
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        let mut cmd = std::process::Command::new("open");
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }

    fn path_kinds(&self, paths: Vec<PathBuf>) -> BoxFuture<Vec<PathKind>> {
        Box::pin(async move { paths.iter().map(|path| PathKind::of(path)).collect() })
    }

    fn load_settings(&self) -> BoxFuture<Result<GlobalSettings, String>> {
        let loaded = self.load_settings_now();
        Box::pin(async move { loaded })
    }

    fn save_settings(&self, settings: GlobalSettings) -> BoxFuture<Result<(), String>> {
        // Hooks use them even if they can't be saved.
        *self.settings() = settings.clone();
        let path = self.settings_path.clone();
        Box::pin(async move {
            match path {
                Some(path) => settings.save(&path).map_err(|e| e.to_string()),
                None => Ok(()),
            }
        })
    }

    fn settings_location(&self) -> Option<String> {
        self.settings_path.as_ref().map(|p| p.display().to_string())
    }

    fn load_device_settings(&self) -> BoxFuture<Result<DeviceSettings, String>> {
        let loaded = self.load_device_settings_now();
        Box::pin(async move { loaded })
    }

    fn save_device_settings(&self, settings: DeviceSettings) -> BoxFuture<Result<(), String>> {
        let path = self.device_path.clone();
        Box::pin(async move {
            match path {
                Some(path) => settings.save(&path).map_err(|e| e.to_string()),
                None => Ok(()),
            }
        })
    }

    fn device_settings_location(&self) -> Option<String> {
        self.device_path.as_ref().map(|p| p.display().to_string())
    }
}
