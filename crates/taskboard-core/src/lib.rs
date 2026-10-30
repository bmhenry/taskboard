//! Shared core for `taskboard` (GUI) and `tb` (CLI): the task model, pure
//! board operations, search filters, and the on-disk `.tb/` store.

pub mod board;
pub mod filter;
pub mod hooks;
pub mod model;
pub mod settings;
pub mod store;

pub use board::{NewTask, TaskPatch};
pub use filter::{DateField, Filter, parse_date};
pub use hooks::{Hook, HookEvent, TaskEvent};
pub use model::{
    Attachment, Board, FileKind, Link, LinkKind, Status, Task, default_link_kind, short_id,
};
pub use settings::{GlobalSettings, ProjectSettings};
pub use store::{Store, project_root};
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
