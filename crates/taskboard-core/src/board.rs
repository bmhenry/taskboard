//! Pure, in-memory operations on a [`Board`]. Nothing here touches the disk;
//! [`crate::Store`] wraps these in locked read-modify-write cycles.

use chrono::Utc;
use ulid::Ulid;

use crate::model::{Board, Link, Status, Task};
use crate::{Error, Result};

/// Fields for a new task. Anything left unset gets a sensible default.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    pub description: String,
    pub status: Option<Status>,
    pub links: Vec<Link>,
}

/// Changes to apply to an existing task; `None` leaves a field alone.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<Status>,
}

impl Board {
    pub fn get(&self, id: Ulid) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn get_mut(&mut self, id: Ulid) -> Result<&mut Task> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }

    /// Resolves a full ULID or a unique suffix of one (such as [`Task::short_id`]),
    /// case-insensitively.
    pub fn resolve(&self, query: &str) -> Result<Ulid> {
        let q = query.trim().to_uppercase();
        if q.is_empty() {
            return Err(Error::NotFound(query.to_string()));
        }
        let matches: Vec<Ulid> = self
            .tasks
            .iter()
            .map(|t| t.id)
            .filter(|id| id.to_string().ends_with(&q))
            .collect();
        match matches.as_slice() {
            [id] => Ok(*id),
            [] => Err(Error::NotFound(query.to_string())),
            _ => Err(Error::Ambiguous(query.to_string(), matches.len())),
        }
    }

    /// Tasks with the given status, in board order.
    pub fn column(&self, status: Status) -> Vec<&Task> {
        let mut col: Vec<&Task> = self.tasks.iter().filter(|t| t.status == status).collect();
        col.sort_by_key(|t| (t.order, t.id));
        col
    }

    /// Adds a task at the bottom of its status column.
    pub fn create(&mut self, new: NewTask) -> Result<Ulid> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err(Error::Invalid("title must not be empty".into()));
        }
        let status = new.status.unwrap_or(Status::Open);
        let now = Utc::now();
        let task = Task {
            id: Ulid::generate(),
            title,
            description: new.description,
            status,
            order: self.column(status).len() as u32,
            created: now,
            updated: now,
            closed: (status == Status::Closed).then_some(now),
            links: new.links,
            files: Vec::new(),
        };
        let id = task.id;
        self.tasks.push(task);
        Ok(id)
    }

    pub fn edit(&mut self, id: Ulid, patch: TaskPatch) -> Result<()> {
        let task = self.get_mut(id)?;
        if let Some(title) = patch.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(Error::Invalid("title must not be empty".into()));
            }
            task.title = title;
        }
        if let Some(desc) = patch.description {
            task.description = desc;
        }
        task.updated = Utc::now();
        match patch.status {
            Some(status) if status != task.status => self.move_to(id, status, None),
            _ => Ok(()),
        }
    }

    /// Moves a task to `position` within `status` (end of the column if `None`
    /// or out of range). Works for both changing status and reordering in place.
    pub fn move_to(&mut self, id: Ulid, status: Status, position: Option<usize>) -> Result<()> {
        let now = Utc::now();
        let task = self.get_mut(id)?;
        let old_status = task.status;
        if old_status != status {
            task.status = status;
            task.updated = now;
            task.closed = (status == Status::Closed).then_some(now);
        }

        let mut ids: Vec<Ulid> = self
            .column(status)
            .into_iter()
            .map(|t| t.id)
            .filter(|t| *t != id)
            .collect();
        let pos = position.unwrap_or(ids.len()).min(ids.len());
        ids.insert(pos, id);
        self.renumber(&ids);
        if old_status != status {
            let rest: Vec<Ulid> = self.column(old_status).iter().map(|t| t.id).collect();
            self.renumber(&rest);
        }
        Ok(())
    }

    fn renumber(&mut self, ordered: &[Ulid]) {
        for (i, id) in ordered.iter().enumerate() {
            if let Ok(t) = self.get_mut(*id) {
                t.order = i as u32;
            }
        }
    }

    /// Removes a task and returns it, so the caller can clean up its files.
    pub fn delete(&mut self, id: Ulid) -> Result<Task> {
        let idx = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let task = self.tasks.remove(idx);
        let rest: Vec<Ulid> = self.column(task.status).iter().map(|t| t.id).collect();
        self.renumber(&rest);
        Ok(task)
    }

    pub fn add_link(&mut self, id: Ulid, link: Link) -> Result<()> {
        let task = self.get_mut(id)?;
        task.links.push(link);
        task.updated = Utc::now();
        Ok(())
    }

    /// Removes the link at `index` (0-based) and returns it.
    pub fn remove_link(&mut self, id: Ulid, index: usize) -> Result<Link> {
        let task = self.get_mut(id)?;
        if index >= task.links.len() {
            return Err(Error::NotFound(format!("link #{index}")));
        }
        task.updated = Utc::now();
        Ok(task.links.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titled(board: &mut Board, title: &str, status: Status) -> Ulid {
        board
            .create(NewTask {
                title: title.into(),
                status: Some(status),
                ..Default::default()
            })
            .unwrap()
    }

    fn titles(board: &Board, status: Status) -> Vec<String> {
        board
            .column(status)
            .iter()
            .map(|t| t.title.clone())
            .collect()
    }

    #[test]
    fn move_reorders_and_changes_status() {
        let mut b = Board::default();
        let a = titled(&mut b, "a", Status::Backlog);
        titled(&mut b, "b", Status::Backlog);
        let c = titled(&mut b, "c", Status::Backlog);

        b.move_to(c, Status::Backlog, Some(0)).unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["c", "a", "b"]);

        b.move_to(a, Status::Closed, None).unwrap();
        assert_eq!(titles(&b, Status::Backlog), ["c", "b"]);
        assert_eq!(b.column(Status::Backlog)[1].order, 1);
        assert!(b.get(a).unwrap().closed.is_some());

        b.move_to(a, Status::InProgress, None).unwrap();
        assert!(b.get(a).unwrap().closed.is_none());
    }

    #[test]
    fn resolve_by_suffix() {
        let mut b = Board::default();
        let a = titled(&mut b, "a", Status::Open);
        let short = b.get(a).unwrap().short_id().to_lowercase();
        assert_eq!(b.resolve(&short).unwrap(), a);
        assert!(b.resolve("nope").is_err());
    }
}
