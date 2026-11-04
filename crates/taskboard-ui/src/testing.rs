//! A [`Platform`] for tests, running commands in-process on a temporary
//! [`Store`], and a [`Harness`] that runs an [`App`]'s effects on it.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use iced::Subscription;
use taskboard_core::ops::{self, Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, DeviceSettings, GlobalSettings, Store, Ulid};

use crate::app::{Action, App, Effect};
use crate::platform::{BoxFuture, Platform, ResolvedPath, WatchEvent};

#[derive(Clone)]
pub struct TestPlatform {
    pub store: Store,
    /// Where settings are saved; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// The settings last saved, which commands run hooks from.
    settings: Arc<Mutex<GlobalSettings>>,
    /// Where device settings are saved; `None` keeps them in memory only.
    pub device_path: Option<PathBuf>,
    /// Every batch of paths asked about, oldest first.
    pub path_checks: Arc<Mutex<Vec<Vec<PathBuf>>>>,
    /// While set, every command fails with this error without running, as
    /// when the store can't be written or a web request is refused.
    pub failure: Arc<Mutex<Option<String>>>,
    /// [`Platform::attaches_paths`]; off acts like the web client.
    pub attaches_paths: bool,
}

impl TestPlatform {
    /// Device settings are saved in `device.json` beside `settings_path`.
    pub fn new(store: Store, settings_path: Option<PathBuf>) -> TestPlatform {
        TestPlatform {
            device_path: settings_path
                .as_ref()
                .map(|p| p.with_file_name("device.json")),
            store,
            settings_path,
            settings: Arc::default(),
            path_checks: Arc::default(),
            failure: Arc::default(),
            attaches_paths: true,
        }
    }
}

impl Platform for TestPlatform {
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>> {
        let store = self.store.clone();
        let settings = self.settings.lock().unwrap().clone();
        let failure = self.failure.lock().unwrap().clone();
        Box::pin(async move {
            match failure {
                Some(e) => Err(e),
                None => ops::execute(&store, &settings, command).map_err(|e| e.to_string()),
            }
        })
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
        Subscription::none()
    }

    fn pick_file(&self) -> BoxFuture<Result<Option<FileSource>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn pick_folder(&self, _title: &str) -> BoxFuture<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn open_url(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }

    fn attaches_paths(&self) -> bool {
        self.attaches_paths
    }

    fn resolve_paths(&self, paths: Vec<PathBuf>) -> BoxFuture<Vec<ResolvedPath>> {
        let resolved = paths.iter().map(|path| ResolvedPath::of(path)).collect();
        self.path_checks.lock().unwrap().push(paths);
        Box::pin(async move { resolved })
    }

    fn load_settings(&self) -> BoxFuture<Result<GlobalSettings, String>> {
        let loaded = match &self.settings_path {
            Some(path) => GlobalSettings::load(path).map_err(|e| e.to_string()),
            None => Ok(GlobalSettings::default()),
        };
        if let Ok(settings) = &loaded {
            *self.settings.lock().unwrap() = settings.clone();
        }
        Box::pin(async move { loaded })
    }

    fn save_settings(&self, settings: GlobalSettings) -> BoxFuture<Result<(), String>> {
        *self.settings.lock().unwrap() = settings.clone();
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
        let loaded = match &self.device_path {
            Some(path) => {
                DeviceSettings::load(path, self.settings_path.as_deref()).map_err(|e| e.to_string())
            }
            None => Ok(DeviceSettings::default()),
        };
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

/// The output of one of [`TestPlatform`]'s futures, which never wait.
pub fn now<T>(mut future: BoxFuture<T>) -> T {
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("a test platform future waited"),
    }
}

/// An [`App`] whose effects run as soon as it applies an action, as if the
/// GUI ran them at once.
pub struct Harness {
    pub app: App,
    pub platform: TestPlatform,
}

impl Harness {
    pub fn new(platform: TestPlatform) -> Harness {
        let settings = now(platform.load_settings());
        let device = now(platform.load_device_settings());
        let board = now(platform.load_board());
        let mut harness = Harness {
            app: App::new(settings, device, board),
            platform,
        };
        harness.run_effects();
        harness
    }

    /// Applies `action` and runs what it queued; see [`Harness::settle`].
    pub fn apply(&mut self, action: Action) {
        self.app.apply(action);
        self.settle();
    }

    /// Runs the app's queued effects in order, then reloads the board, as
    /// `Gui` does.
    pub fn settle(&mut self) {
        self.run_effects();
        self.reload();
    }

    /// Loads the board, as when a change made elsewhere is noticed, and
    /// runs what that queued, like checking new projects' folders.
    pub fn reload(&mut self) {
        self.app.loaded(now(self.platform.load_board()));
        self.run_effects();
    }

    /// Runs the app's queued effects in order, including any they queue.
    pub fn run_effects(&mut self) {
        while let Some(effect) = self.app.outbox.pop_front() {
            match effect {
                Effect::Run(command) => {
                    let result = now(self.platform.run(command.clone()));
                    self.app.finished(&command, result);
                }
                Effect::SaveSettings(settings) => {
                    let result = now(self.platform.save_settings(settings));
                    self.app.settings_saved(result);
                }
                Effect::SaveDeviceSettings(settings) => {
                    let result = now(self.platform.save_device_settings(settings));
                    self.app.settings_saved(result);
                }
                Effect::CheckPaths(check) => {
                    let resolved = now(self.platform.resolve_paths(check.paths()));
                    self.app.checked(check, resolved);
                }
            }
        }
    }
}

impl Deref for Harness {
    type Target = App;

    fn deref(&self) -> &App {
        &self.app
    }
}

impl DerefMut for Harness {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}
