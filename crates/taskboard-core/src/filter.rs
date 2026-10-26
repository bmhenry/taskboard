use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::Error;
use crate::model::{Status, Task};

/// Which timestamp a date range filters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateField {
    #[default]
    Created,
    Updated,
    Closed,
}

impl DateField {
    pub fn of(self, task: &Task) -> Option<DateTime<Utc>> {
        match self {
            DateField::Created => Some(task.created),
            DateField::Updated => Some(task.updated),
            DateField::Closed => task.closed,
        }
    }
}

impl FromStr for DateField {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "created" => Ok(DateField::Created),
            "updated" => Ok(DateField::Updated),
            "closed" => Ok(DateField::Closed),
            _ => Err(Error::Invalid(format!("unknown date field '{s}'"))),
        }
    }
}

/// Search and filter criteria. The default matches every task.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Case-insensitive text matched against title, description and links.
    pub text: String,
    /// Allowed statuses; empty allows all.
    pub statuses: Vec<Status>,
    pub date_field: DateField,
    /// Inclusive lower bound on `date_field`.
    pub after: Option<DateTime<Utc>>,
    /// Exclusive upper bound on `date_field`.
    pub before: Option<DateTime<Utc>>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.statuses.is_empty()
            && self.after.is_none()
            && self.before.is_none()
    }

    pub fn matches(&self, task: &Task) -> bool {
        if !self.statuses.is_empty() && !self.statuses.contains(&task.status) {
            return false;
        }
        if self.after.is_some() || self.before.is_some() {
            let Some(when) = self.date_field.of(task) else {
                return false;
            };
            if self.after.is_some_and(|a| when < a) || self.before.is_some_and(|b| when >= b) {
                return false;
            }
        }
        let needle = self.text.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let hit = |s: &str| s.to_lowercase().contains(&needle);
        hit(&task.title)
            || hit(&task.description)
            || task
                .links
                .iter()
                .any(|l| hit(&l.url) || l.label.as_deref().is_some_and(hit))
    }
}

/// Parses a date as RFC 3339 or a plain `YYYY-MM-DD` (midnight UTC).
pub fn parse_date(s: &str) -> Result<DateTime<Utc>, Error> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        .map_err(|_| Error::Invalid(format!("invalid date '{s}' (use YYYY-MM-DD or RFC 3339)")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::NewTask;
    use crate::model::{Board, Link, LinkKind};

    #[test]
    fn filters_by_text_status_and_date() {
        let mut b = Board::default();
        let id = b
            .create(NewTask {
                title: "Fix login".into(),
                status: Some(Status::Backlog),
                links: vec![Link {
                    url: "https://example.com/issues/42".into(),
                    label: None,
                    kind: LinkKind::Source,
                }],
                ..Default::default()
            })
            .unwrap();
        let task = b.get(id).unwrap();

        let by_link = Filter {
            text: "ISSUES/42".into(),
            ..Default::default()
        };
        assert!(by_link.matches(task));

        let wrong_status = Filter {
            statuses: vec![Status::Closed],
            ..Default::default()
        };
        assert!(!wrong_status.matches(task));

        let future = Filter {
            after: Some(parse_date("2999-01-01").unwrap()),
            ..Default::default()
        };
        assert!(!future.matches(task));
    }
}
