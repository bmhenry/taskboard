use std::fmt;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::Error;

/// Current version of the on-disk `tasks.json` schema.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole contents of `tasks.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub schema_version: u32,
    pub tasks: Vec<Task>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            schema_version: SCHEMA_VERSION,
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: Ulid,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: Status,
    /// Position within the task's current status column; lower comes first.
    pub order: u32,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub closed: Option<DateTime<Utc>>,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default)]
    pub files: Vec<Attachment>,
}

impl Task {
    /// Short, human-friendly id: the tail of the ULID (its random part), which
    /// stays distinct for tasks created in the same millisecond.
    pub fn short_id(&self) -> String {
        short_id(&self.id)
    }

    /// The kind a newly added link should get unless the user picks one:
    /// the first source link is the most likely, then context after that.
    pub fn default_link_kind(&self) -> LinkKind {
        if self.links.iter().any(|l| l.kind == LinkKind::Source) {
            LinkKind::Context
        } else {
            LinkKind::Source
        }
    }
}

pub fn short_id(id: &Ulid) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Exists but is not ready to work.
    Open,
    /// Ready to work.
    Backlog,
    InProgress,
    NeedsReview,
    InReview,
    Closed,
}

impl Status {
    pub const ALL: [Status; 6] = [
        Status::Open,
        Status::Backlog,
        Status::InProgress,
        Status::NeedsReview,
        Status::InReview,
        Status::Closed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "Open",
            Status::Backlog => "Backlog",
            Status::InProgress => "In Progress",
            Status::NeedsReview => "Needs Review",
            Status::InReview => "In Review",
            Status::Closed => "Closed",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Backlog => "backlog",
            Status::InProgress => "in_progress",
            Status::NeedsReview => "needs_review",
            Status::InReview => "in_review",
            Status::Closed => "closed",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Status {
    type Err = Error;

    /// Accepts the key or label in any case, with `-`, `_` or spaces between words.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm: String = s
            .trim()
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .flat_map(char::to_lowercase)
            .collect();
        Status::ALL
            .into_iter()
            .find(|st| st.key().replace('_', "") == norm)
            .ok_or_else(|| Error::Invalid(format!("unknown status '{s}'")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Where the task came from (an issue, a message, ...).
    Source,
    /// Supporting material.
    #[default]
    Context,
}

impl FromStr for LinkKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "source" => Ok(LinkKind::Source),
            "context" => Ok(LinkKind::Context),
            _ => Err(Error::Invalid(format!("unknown link kind '{s}'"))),
        }
    }
}

impl fmt::Display for LinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LinkKind::Source => "source",
            LinkKind::Context => "context",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub url: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub kind: LinkKind,
}

impl Link {
    pub fn display_text(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Image,
    Markdown,
    Text,
}

impl FileKind {
    pub fn from_name(name: &str) -> FileKind {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => FileKind::Image,
            "md" | "markdown" => FileKind::Markdown,
            _ => FileKind::Text,
        }
    }
}

/// A file copied into `.tb/files/`, stored under its ULID so names never collide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Ulid,
    /// The file's original name, shown in the UI.
    pub name: String,
    /// Path relative to the `.tb/` directory.
    pub path: String,
    pub kind: FileKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parses_loose_spellings() {
        assert_eq!("In Progress".parse::<Status>().unwrap(), Status::InProgress);
        assert_eq!(
            "needs-review".parse::<Status>().unwrap(),
            Status::NeedsReview
        );
        assert!("doing".parse::<Status>().is_err());
    }

    #[test]
    fn default_link_kind_is_source_until_one_exists() {
        let mut board = crate::Board::default();
        let id = board
            .create(crate::NewTask {
                title: "t".into(),
                ..Default::default()
            })
            .unwrap();
        let link = |kind| Link {
            url: "https://example.com".into(),
            label: None,
            kind,
        };
        board.add_link(id, link(LinkKind::Context)).unwrap();
        assert_eq!(board.get(id).unwrap().default_link_kind(), LinkKind::Source);
        board.add_link(id, link(LinkKind::Source)).unwrap();
        assert_eq!(board.get(id).unwrap().default_link_kind(), LinkKind::Context);
    }
}
