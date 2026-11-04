//! What the UI needs from the program hosting it: the store, file dialogs,
//! a browser, what's at paths and settings. The desktop app implements [`Platform`] with the
//! local filesystem; the web client will implement it over HTTP.

use std::path::PathBuf;

use iced::Subscription;
use taskboard_core::ops::{Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, DeviceSettings, GlobalSettings, Ulid};

/// A boxed future the UI can run as an `iced::Task`: `Send` natively, but
/// not on the web, where nothing is.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<T> = iced::futures::future::BoxFuture<'static, T>;
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<T> = iced::futures::future::LocalBoxFuture<'static, T>;

/// What a [`Platform::board_changes`] subscription reports to the UI.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// The board changed, maybe in another process (like `tb`).
    Changed,
    /// Watching couldn't start, for the reason given; live updates are off.
    Failed(String),
}

pub use taskboard_core::api::{PathKind, ResolvedPath};

/// The program hosting the UI. Errors are strings, ready to show, since on
/// the web they come over the network.
pub trait Platform {
    /// Runs a command against the store. [`crate::Gui`] runs one at a time,
    /// in the order they were made.
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>>;

    fn load_board(&self) -> BoxFuture<Result<Board, String>>;

    /// The contents of `file`, one of `task`'s attachments.
    fn read_file(&self, task: Ulid, file: &Attachment) -> BoxFuture<Result<Vec<u8>, String>>;

    /// Reports changes to the board made outside the UI, so they show up live.
    fn board_changes(&self) -> Subscription<WatchEvent>;

    /// Asks the user for a file to attach: a path on the desktop, its name
    /// and contents on the web. `None` if they cancelled, an error if the
    /// file can't be taken (like one too big to upload).
    fn pick_file(&self) -> BoxFuture<Result<Option<FileSource>, String>>;

    /// Asks the user for a folder, with the dialog titled `title`. Always
    /// `None` on the web, where folders are typed.
    fn pick_folder(&self, title: &str) -> BoxFuture<Option<PathBuf>>;

    /// Whether [`Platform::pick_folder`] can ask at all. If not, the UI
    /// has no buttons for it, and folders are only typed.
    fn picks_folders(&self) -> bool {
        true
    }

    /// Whether a file can be attached by typing its path, which is read on
    /// the machine holding the store. If not (on the web, where that's the
    /// daemon's machine), files are only attached with
    /// [`Platform::pick_file`], which uploads them.
    fn attaches_paths(&self) -> bool {
        true
    }

    /// Opens `url` in the user's browser.
    fn open_url(&self, url: &str) -> Result<(), String>;

    /// Resolves each of `paths`, in order, on the machine holding the store,
    /// as [`ResolvedPath::of`] does: where a typed path leads (`~/`,
    /// relative paths and symlinks are read there, not where the UI runs)
    /// and what's there, such as whether projects' directories still exist.
    /// Paths left without an answer, as when the request failed, are left
    /// off the end.
    fn resolve_paths(&self, paths: Vec<PathBuf>) -> BoxFuture<Vec<ResolvedPath>>;

    fn load_settings(&self) -> BoxFuture<Result<GlobalSettings, String>>;

    /// Saves settings. Commands run after this use them, e.g. for hooks.
    fn save_settings(&self, settings: GlobalSettings) -> BoxFuture<Result<(), String>>;

    /// Where settings are saved, to show in the settings dialog; `None` if
    /// they can't be.
    fn settings_location(&self) -> Option<String>;

    /// Settings for this device only, like the font size: a file on the
    /// desktop, the browser's storage on the web.
    fn load_device_settings(&self) -> BoxFuture<Result<DeviceSettings, String>>;

    fn save_device_settings(&self, settings: DeviceSettings) -> BoxFuture<Result<(), String>>;

    /// Where device settings are saved, like [`Platform::settings_location`].
    fn device_settings_location(&self) -> Option<String>;
}
