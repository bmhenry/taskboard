//! Shared core for `taskboard` (GUI), `tb` (CLI) and `taskboard-web`
//! (daemon): the task model, pure board operations, search filters, the
//! on-disk store shared by every project, and the web API's types.

pub mod api;
pub mod board;
pub mod filter;
pub mod hooks;
pub mod model;
pub mod ops;
pub mod settings;
pub mod store;
#[cfg(feature = "watch")]
pub mod watch;

pub use board::{NewTask, Progress, TaskPatch};
pub use filter::{DateField, Filter, parse_date};
pub use hooks::{Hook, HookEvent, TaskEvent};
pub use model::{
    Attachment, Board, FileKind, Link, LinkKind, Project, Status, Task, default_link_kind, short_id,
};
pub use ops::{Command, FileSource, Outcome, Output};
pub use settings::{DeviceSettings, GlobalSettings};
pub use store::{
    ProjectInput, Store, create_project_dir, new_file_name, project_path, project_root,
    resolve_project,
};
pub use ulid::Ulid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid tasks.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no task or item matches '{0}'")]
    NotFound(String),
    #[error("'{0}' matches {1} tasks; use more characters")]
    Ambiguous(String, usize),
    #[error("tasks.json uses schema version {0}, which is newer than this build supports")]
    UnsupportedSchema(u32),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// `$XDG_CONFIG_HOME/tb`, or `~/.config/tb`: where settings and the web
/// daemon's token are kept. `None` if neither variable is set.
pub fn config_dir() -> Option<std::path::PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$<var>/tb`, falling back to `~/<fallback>/tb` as the XDG base directory
/// spec says. `None` if neither variable is set.
fn xdg_dir(var: &str, fallback: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(fallback)))?;
    Some(base.join("tb"))
}
