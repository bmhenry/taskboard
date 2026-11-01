//! A [`Platform`] for tests, running commands in-process on a temporary
//! [`Store`], and a [`Harness`] that runs an [`App`]'s effects on it.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use iced::Subscription;
use taskboard_core::ops::{self, Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, GlobalSettings, Store, Ulid};

use crate::app::{Action, App, Effect};
use crate::platform::{BoxFuture, PathKind, Platform, WatchEvent};

#[derive(Clone)]
pub struct TestPlatform {
    pub store: Store,
    /// Where settings are saved; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// The settings last saved, which commands run hooks from.
    settings: Arc<Mutex<GlobalSettings>>,
}

impl TestPlatform {
    pub fn new(store: Store, settings_path: Option<PathBuf>) -> TestPlatform {
        TestPlatform {
            store,
            settings_path,
            settings: Arc::default(),
        }
    }
}

impl Platform for TestPlatform {
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>> {
        let store = self.store.clone();
        let settings = self.settings.lock().unwrap().clone();
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
        Subscription::none()
    }

    fn pick_file(&self) -> BoxFuture<Option<FileSource>> {
        Box::pin(async { None })
    }

    fn pick_folder(&self, _title: &str) -> BoxFuture<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn open_url(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }

    fn path_kind(&self, path: &Path) -> BoxFuture<PathKind> {
        let kind = match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => PathKind::Dir,
            Ok(_) => PathKind::File,
            Err(_) => PathKind::Missing,
        };
        Box::pin(async move { kind })
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
        let board = now(platform.load_board());
        Harness {
            app: App::new(settings, board),
            platform,
        }
    }

    /// Applies `action` and runs what it queued; see [`Harness::settle`].
    pub fn apply(&mut self, action: Action) {
        self.app.apply(action);
        self.settle();
    }

    /// Runs the app's queued effects in order, then reloads the board, as
    /// `Gui` does.
    pub fn settle(&mut self) {
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
            }
        }
        self.reload();
    }

    /// Loads the board, as when a change made elsewhere is noticed.
    pub fn reload(&mut self) {
        self.app.loaded(now(self.platform.load_board()));
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
