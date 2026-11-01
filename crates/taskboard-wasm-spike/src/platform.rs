//! The spike's [`Platform`]: commands run in the browser against the
//! in-memory store. There's no filesystem, so attached files can't be read,
//! nothing is watched (Phase 2 uses SSE), and settings aren't saved.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use iced::Subscription;
use taskboard_core::ops::{self, Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, GlobalSettings, Store, Ulid};
use taskboard_ui::{BoxFuture, PathKind, Platform, WatchEvent};

pub struct Spike {
    pub store: Store,
    /// The settings last saved, which commands run hooks from.
    settings: Rc<RefCell<GlobalSettings>>,
}

impl Spike {
    pub fn new(store: Store) -> Spike {
        Spike {
            store,
            settings: Rc::default(),
        }
    }
}

impl Platform for Spike {
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>> {
        let store = self.store.clone();
        let settings = self.settings.borrow().clone();
        Box::pin(async move { ops::execute(&store, &settings, command).map_err(|e| e.to_string()) })
    }

    fn load_board(&self) -> BoxFuture<Result<Board, String>> {
        let store = self.store.clone();
        Box::pin(async move { store.load().map_err(|e| e.to_string()) })
    }

    fn read_file(&self, _task: Ulid, _file: &Attachment) -> BoxFuture<Result<Vec<u8>, String>> {
        Box::pin(async { Err("files can't be read in the spike".to_string()) })
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

    fn open_url(&self, url: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("no browser window")?;
        window
            .open_with_url_and_target(url, "_blank")
            .map(|_| ())
            .map_err(|e| format!("couldn't open {url}: {e:?}"))
    }

    fn path_kind(&self, _path: &Path) -> BoxFuture<PathKind> {
        Box::pin(async { PathKind::Missing })
    }

    fn load_settings(&self) -> BoxFuture<Result<GlobalSettings, String>> {
        let settings = self.settings.borrow().clone();
        Box::pin(async move { Ok(settings) })
    }

    fn save_settings(&self, settings: GlobalSettings) -> BoxFuture<Result<(), String>> {
        *self.settings.borrow_mut() = settings;
        Box::pin(async { Ok(()) })
    }

    fn settings_location(&self) -> Option<String> {
        None
    }
}
