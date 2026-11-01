//! The web daemon's (`taskboard-web`) HTTP API: its routes, and the JSON
//! types it and the web client both use. Requests and responses are JSON
//! unless noted.
//!
//! - `GET` [`BOARD`] → [`BoardResponse`].
//! - `POST` [`COMMANDS`] with a [`Command`] → the [`Outcome`] of
//!   [`crate::ops::execute`]. Commands that read files on the daemon's
//!   machine are refused; see [`Command::is_remote_safe`].
//! - `GET` [`FILES`]`/{task}/{file}` (ids) → the attachment's bytes, as a
//!   download named after it.
//! - `POST` [`UPLOADS`]`?task=…&name=…` ([`UploadQuery`]) with a file's raw
//!   bytes, sent as `application/octet-stream` → the [`Outcome`] of attaching
//!   it to the task, as a [`Command::Attach`] with [`FileSource::Bytes`]
//!   would, without base64.
//! - `POST` [`PATH_KINDS`] with a list of paths → a [`PathKind`] for each.
//! - `GET` [`SETTINGS`] → [`SettingsResponse`]; `PUT` [`SETTINGS`] with
//!   [`GlobalSettings`] saves them.
//! - `GET` [`EVENTS`] → server-sent events, each a [`BoardEvent`] in its
//!   `data`.
//!
//! When one of these fails, including when the request is malformed, the
//! response has a 4xx or 5xx status and an [`ErrorBody`].
//!
//! [`Command`]: crate::Command
//! [`Command::Attach`]: crate::Command::Attach
//! [`Command::is_remote_safe`]: crate::Command::is_remote_safe
//! [`FileSource::Bytes`]: crate::FileSource::Bytes
//! [`Outcome`]: crate::Outcome

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::model::Board;
use crate::settings::GlobalSettings;

pub const BOARD: &str = "/api/board";
pub const COMMANDS: &str = "/api/commands";
pub const FILES: &str = "/api/files";
pub const UPLOADS: &str = "/api/uploads";
pub const PATH_KINDS: &str = "/api/path-kinds";
pub const SETTINGS: &str = "/api/settings";
pub const EVENTS: &str = "/api/events";

/// What `GET` [`BOARD`] returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardResponse {
    pub board: Board,
    /// The board's projects whose directories aren't on the daemon's
    /// machine any more.
    pub missing_projects: Vec<PathBuf>,
    /// The project new tasks go in by default, if the daemon has one.
    pub default_project: Option<PathBuf>,
}

/// The query of `POST` [`UPLOADS`]; the body is the file's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadQuery {
    /// The task to attach the file to.
    pub task: Ulid,
    /// The file's name, which must be a plain file name; see
    /// [`crate::Store::attach_bytes`].
    pub name: String,
}

/// What `GET` [`SETTINGS`] returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub settings: GlobalSettings,
    /// Where the daemon saves them, to show in the settings dialog; `None`
    /// if it can't.
    pub location: Option<String>,
}

/// What's at a path on the machine holding the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    File,
    Dir,
    /// Nothing, or something that can't be read.
    Missing,
}

impl PathKind {
    /// What's at `path` on this machine.
    pub fn of(path: &Path) -> PathKind {
        match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => PathKind::Dir,
            Ok(_) => PathKind::File,
            Err(_) => PathKind::Missing,
        }
    }
}

/// One of the server-sent events from [`EVENTS`]: a JSON object tagged with
/// a snake_case `"type"`, e.g. `{"type": "changed"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoardEvent {
    /// The board may have changed; it's also sent first on connecting, for
    /// changes made since the board was loaded.
    Changed,
    /// The daemon can't watch the store, for the reason given, so live
    /// updates are off.
    Failed { reason: String },
}

/// The body of a failed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What went wrong, ready to show.
    pub error: String,
}
