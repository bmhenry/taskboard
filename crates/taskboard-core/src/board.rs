//! Pure, in-memory operations on a [`Board`]. Nothing here touches the disk;
//! [`crate::Store`] wraps these in locked read-modify-write cycles.

use std::path::{Path, PathBuf};

use chrono::Utc;
use ulid::Ulid;

use crate::model::{Board, Link, Project, Status, Task};
use crate::{Error, Result};

/// Fields for a new task. Anything left unset gets a sensible default.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    /// The project's absolute directory; required.
    pub project: PathBuf,
    pub description: String,
    pub status: Option<Status>,
    pub links: Vec<Link>,
}

/// Changes to apply to an existing task; `None` leaves a field alone.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    /// Moves the task to another project's absolute directory.
    pub project: Option<PathBuf>,
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

    pub fn project(&self, path: &Path) -> Option<&Project> {
        self.projects.iter().find(|p| p.path == path)
    }

    /// The name shown for the project at `path`, even if it isn't registered:
    /// its directory's name, with as many parent directories in front as it
    /// takes to tell it apart from every other project's name.
    pub fn project_name(&self, path: &Path) -> String {
        let parts: Vec<_> = path.components().collect();
        let others: Vec<Vec<_>> = self
            .projects
            .iter()
            .filter(|p| p.path != path)
            .map(|p| p.path.components().collect())
            .collect();
        let shares_last = |n: usize| {
            others
                .iter()
                .any(|o: &Vec<_>| o.ends_with(&parts[parts.len() - n..]))
        };
        let n = (1..parts.len())
            .find(|&n| !shares_last(n))
            .unwrap_or(parts.len());
        parts[parts.len() - n..]
            .iter()
            .collect::<PathBuf>()
            .display()
            .to_string()
    }

    /// Every project with its name, sorted by name.
    pub fn project_names(&self) -> Vec<(&Project, String)> {
        let mut named: Vec<_> = self
            .projects
            .iter()
            .map(|p| (p, self.project_name(&p.path)))
            .collect();
        named.sort_by_key(|(_, name)| name.to_lowercase());
        named
    }

    /// Registers the project at `path` unless it already is.
    fn add_project(&mut self, path: &Path) -> Result<()> {
        if path.as_os_str().is_empty() {
            return Err(Error::Invalid("a task needs a project".into()));
        }
        if self.project(path).is_none() {
            self.projects.push(Project {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }

    /// Points the project at `from` and its tasks at the directory `to`, as
    /// when the project was moved. If `to` is already a project, the two merge.
    pub fn relocate_project(&mut self, from: &Path, to: PathBuf) -> Result<()> {
        if self.project(from).is_none() {
            return Err(Error::NotFound(from.display().to_string()));
        }
        if from == to {
            return Ok(());
        }
        self.add_project(&to)?;
        self.projects.retain(|p| p.path != from);
        let now = Utc::now();
        for task in self.tasks.iter_mut().filter(|t| t.project == from) {
            task.project = to.clone();
            task.updated = now;
        }
        Ok(())
    }

    /// Removes the project at `path` and all its tasks, returning the tasks so
    /// the caller can clean up their files.
    pub fn remove_project(&mut self, path: &Path) -> Result<Vec<Task>> {
        if self.project(path).is_none() {
            return Err(Error::NotFound(path.display().to_string()));
        }
        self.projects.retain(|p| p.path != path);
        let (removed, kept) = std::mem::take(&mut self.tasks)
            .into_iter()
            .partition(|t| t.project == path);
        self.tasks = kept;
        for status in Status::ALL {
            let rest: Vec<Ulid> = self.column(status).iter().map(|t| t.id).collect();
            self.renumber(&rest);
        }
        Ok(removed)
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
        self.add_project(&new.project)?;
        let status = new.status.unwrap_or(Status::Open);
        let now = Utc::now();
        let task = Task {
            id: Ulid::generate(),
            title,
            project: new.project,
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
        if let Some(project) = &patch.project {
            self.get_mut(id)?;
            self.add_project(project)?;
        }
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
        if let Some(project) = patch.project {
            task.project = project;
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
                project: "/p".into(),
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
    fn projects_are_registered_once_on_create_and_edit() {
        let mut b = Board::default();
        let a = titled(&mut b, "a", Status::Open);
        titled(&mut b, "b", Status::Open);
        assert_eq!(b.projects, [Project { path: "/p".into() }]);
        b.edit(
            a,
            TaskPatch {
                project: Some("/src/app".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(b.get(a).unwrap().project, Path::new("/src/app"));
        assert_eq!(b.project_name(Path::new("/src/app")), "app");
        assert_eq!(b.projects.len(), 2);

        let no_project = b.create(NewTask {
            title: "c".into(),
            ..Default::default()
        });
        assert!(no_project.is_err());
    }

    #[test]
    fn project_names_grow_until_unique() {
        let mut b = Board::default();
        for (i, path) in [
            "/home/me/work/app",
            "/home/me/src/app",
            "/srv/me/src/app",
            "/tool",
        ]
        .into_iter()
        .enumerate()
        {
            b.create(NewTask {
                title: i.to_string(),
                project: path.into(),
                ..Default::default()
            })
            .unwrap();
        }
        let names: Vec<String> = b.project_names().into_iter().map(|(_, n)| n).collect();
        assert_eq!(
            names,
            ["home/me/src/app", "srv/me/src/app", "tool", "work/app"]
        );
        // A path that's the tail of another needs all of it, root included.
        b.add_project(Path::new("/me/src/app")).unwrap();
        assert_eq!(b.project_name(Path::new("/me/src/app")), "/me/src/app");
    }

    #[test]
    fn relocating_and_removing_projects() {
        let mut b = Board::default();
        let a = titled(&mut b, "a", Status::Backlog);
        let other = b
            .create(NewTask {
                title: "o".into(),
                project: "/q".into(),
                status: Some(Status::Backlog),
                ..Default::default()
            })
            .unwrap();
        titled(&mut b, "c", Status::Backlog);

        b.relocate_project(Path::new("/p"), "/moved/p".into())
            .unwrap();
        assert_eq!(b.get(a).unwrap().project, Path::new("/moved/p"));
        assert!(b.project(Path::new("/p")).is_none());
        // Moving onto an existing project merges the two.
        b.relocate_project(Path::new("/q"), "/moved/p".into())
            .unwrap();
        assert_eq!(b.projects.len(), 1);
        assert_eq!(b.get(other).unwrap().project, Path::new("/moved/p"));
        assert!(b.relocate_project(Path::new("/nope"), "/x".into()).is_err());

        titled(&mut b, "d", Status::Backlog); // back in /p
        let removed = b.remove_project(Path::new("/moved/p")).unwrap();
        assert_eq!(removed.len(), 3);
        assert_eq!(titles(&b, Status::Backlog), ["d"]);
        assert_eq!(b.column(Status::Backlog)[0].order, 0);
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
