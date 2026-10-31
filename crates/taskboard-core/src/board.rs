//! Pure, in-memory operations on a [`Board`]. Nothing here touches the disk;
//! [`crate::Store`] wraps these in locked read-modify-write cycles.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use ulid::Ulid;

use crate::model::{Board, Link, Project, Status, Task};
use crate::settings::NewTaskPosition;
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
    /// Top or bottom of its status column.
    pub position: NewTaskPosition,
    pub epic: bool,
    /// The epic it's a child of, which must be in the same project.
    pub parent: Option<Ulid>,
}

/// Changes to apply to an existing task; `None` leaves a field alone.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    /// Moves the task to another project's absolute directory.
    pub project: Option<PathBuf>,
    pub description: Option<String>,
    pub status: Option<Status>,
    /// Sets or, with `Some(None)`, clears the task's epic.
    pub parent: Option<Option<Ulid>>,
}

/// How far through its child tasks an epic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub closed: usize,
    pub total: usize,
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

    /// Like [`Board::resolve`], for a task that has to be an epic.
    pub fn resolve_epic(&self, query: &str) -> Result<Ulid> {
        let id = self.resolve(query)?;
        match self.get(id) {
            Some(task) if !task.epic => {
                Err(Error::Invalid(format!("'{}' isn't an epic", task.title)))
            }
            _ => Ok(id),
        }
    }

    /// Tasks with the given status, in board order.
    pub fn column(&self, status: Status) -> Vec<&Task> {
        let mut col: Vec<&Task> = self.tasks.iter().filter(|t| t.status == status).collect();
        col.sort_by_key(|t| (t.order, t.id));
        col
    }

    /// The epic `task` is a child of. A `parent` that isn't an epic in the
    /// task's project, as a hand-edited file can leave behind, counts as none.
    pub fn parent_of(&self, task: &Task) -> Option<&Task> {
        let parent = self.get(task.parent?)?;
        (!task.epic && parent.epic && parent.project == task.project).then_some(parent)
    }

    /// The child tasks of the epic `id`, in board order.
    pub fn children(&self, id: Ulid) -> Vec<&Task> {
        let mut children: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.parent == Some(id) && self.parent_of(t).is_some())
            .collect();
        children.sort_by_key(|t| (t.status, t.order, t.id));
        children
    }

    pub fn progress(&self, id: Ulid) -> Progress {
        let children = self.children(id);
        Progress {
            closed: children
                .iter()
                .filter(|t| t.status == Status::Closed)
                .count(),
            total: children.len(),
        }
    }

    /// The epics in the project at `path`, in board order.
    pub fn epics(&self, path: &Path) -> Vec<&Task> {
        let mut epics: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.epic && t.project == path)
            .collect();
        epics.sort_by_key(|t| (t.status, t.order, t.id));
        epics
    }

    /// Checks that a task in `project` can be a child of `parent`. Epics are
    /// one level deep: only they can be parents, and they can't have one.
    fn check_parent(&self, epic: bool, parent: Ulid, project: &Path) -> Result<()> {
        if epic {
            return Err(Error::Invalid("an epic can't have a parent".into()));
        }
        let parent = self
            .get(parent)
            .ok_or_else(|| Error::NotFound(parent.to_string()))?;
        if !parent.epic {
            return Err(Error::Invalid(format!(
                "'{}' isn't an epic; promote it first",
                parent.title
            )));
        }
        if parent.project != project {
            return Err(Error::Invalid(format!(
                "a task must be in its epic's project, and '{}' is in {}",
                parent.title,
                self.project_name(&parent.project)
            )));
        }
        Ok(())
    }

    /// Adds a task at the top or bottom of its status column.
    pub fn create(&mut self, new: NewTask) -> Result<Ulid> {
        let title = new.title.trim().to_string();
        if title.is_empty() {
            return Err(Error::Invalid("title must not be empty".into()));
        }
        if let Some(parent) = new.parent {
            self.check_parent(new.epic, parent, &new.project)?;
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
            order: 0, // set by renumbering below
            created: now,
            updated: now,
            closed: (status == Status::Closed).then_some(now),
            links: new.links,
            files: Vec::new(),
            epic: new.epic,
            parent: new.parent,
        };
        let id = task.id;
        let mut ids: Vec<Ulid> = self.column(status).iter().map(|t| t.id).collect();
        match new.position {
            NewTaskPosition::Top => ids.insert(0, id),
            NewTaskPosition::Bottom => ids.push(id),
        }
        self.tasks.push(task);
        self.renumber(&ids);
        Ok(id)
    }

    /// Changes a task. A child can't leave its epic's project unless the same
    /// edit clears or replaces its epic; an epic takes its children with it.
    pub fn edit(&mut self, id: Ulid, patch: TaskPatch) -> Result<()> {
        let task = self
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let project = patch.project.as_deref().unwrap_or(&task.project);
        let parent = patch
            .parent
            .unwrap_or_else(|| self.parent_of(task).map(|epic| epic.id));
        if patch.parent.is_none()
            && let Some(epic) = self.parent_of(task)
            && epic.project != project
        {
            return Err(Error::Invalid(format!(
                "'{}' is a child of '{}' in {}; to move it, leave the epic or pick one in the destination in the same edit",
                task.title,
                epic.title,
                self.project_name(&epic.project)
            )));
        }
        if let Some(parent) = parent {
            self.check_parent(task.epic, parent, project)?;
        }
        let children: Vec<Ulid> = if project == task.project {
            Vec::new()
        } else {
            self.children(id).iter().map(|t| t.id).collect()
        };
        if let Some(project) = &patch.project {
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
        if let Some(parent) = patch.parent {
            task.parent = parent;
        }
        let now = Utc::now();
        task.updated = now;
        let status = patch.status.filter(|s| *s != task.status);
        let project = task.project.clone();
        for child in self.tasks.iter_mut().filter(|t| children.contains(&t.id)) {
            child.project = project.clone();
            child.updated = now;
        }
        match status {
            Some(status) => self.move_to(id, status, None),
            None => Ok(()),
        }
    }

    /// Makes a task an epic, unless it's a child of one.
    pub fn promote(&mut self, id: Ulid) -> Result<()> {
        let task = self
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if task.epic {
            return Ok(());
        }
        if let Some(epic) = self.parent_of(task) {
            return Err(Error::Invalid(format!(
                "an epic can't have a parent; take '{}' out of '{}' first",
                task.title, epic.title
            )));
        }
        let task = self.get_mut(id)?;
        task.epic = true;
        task.parent = None; // only a leftover, as it didn't count as an epic
        task.updated = Utc::now();
        Ok(())
    }

    /// Makes an epic a normal task, unless it still has children.
    pub fn demote(&mut self, id: Ulid) -> Result<()> {
        let children = self.children(id).len();
        let task = self.get_mut(id)?;
        if !task.epic {
            return Ok(());
        }
        if children > 0 {
            return Err(Error::Invalid(format!(
                "'{}' still has child tasks ({children}); move them out first",
                task.title
            )));
        }
        task.epic = false;
        task.updated = Utc::now();
        Ok(())
    }

    /// Makes a task a child of the epic `parent`, or of none.
    pub fn set_parent(&mut self, id: Ulid, parent: Option<Ulid>) -> Result<()> {
        self.edit(
            id,
            TaskPatch {
                parent: Some(parent),
                ..Default::default()
            },
        )
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
    /// An epic's children stay, without a parent.
    pub fn delete(&mut self, id: Ulid) -> Result<Task> {
        let idx = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        let task = self.tasks.remove(idx);
        let now = Utc::now();
        for child in self.tasks.iter_mut().filter(|t| t.parent == Some(id)) {
            child.parent = None;
            child.updated = now;
        }
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
                position: NewTaskPosition::Bottom,
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
    fn new_tasks_go_to_the_top_or_bottom() {
        let mut b = Board::default();
        let new = |title: &str, position| NewTask {
            title: title.into(),
            project: "/p".into(),
            position,
            ..Default::default()
        };
        b.create(new("a", NewTaskPosition::Top)).unwrap();
        b.create(new("b", NewTaskPosition::Top)).unwrap();
        b.create(new("c", NewTaskPosition::Bottom)).unwrap();
        b.create(new("d", NewTaskPosition::default())).unwrap();
        assert_eq!(titles(&b, Status::Open), ["d", "b", "a", "c"]);
        let orders: Vec<u32> = b.column(Status::Open).iter().map(|t| t.order).collect();
        assert_eq!(orders, [0, 1, 2, 3]);
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

    fn epic(board: &mut Board, title: &str) -> Ulid {
        let id = titled(board, title, Status::Backlog);
        board.promote(id).unwrap();
        id
    }

    #[test]
    fn epics_track_their_children() {
        let mut b = Board::default();
        let e = epic(&mut b, "e");
        let x = titled(&mut b, "x", Status::Backlog);
        let y = titled(&mut b, "y", Status::Closed);
        b.set_parent(x, Some(e)).unwrap();
        b.set_parent(y, Some(e)).unwrap();

        assert_eq!(b.parent_of(b.get(x).unwrap()).unwrap().id, e);
        let children: Vec<Ulid> = b.children(e).iter().map(|t| t.id).collect();
        assert_eq!(children, [x, y]);
        assert_eq!(
            b.progress(e),
            Progress {
                closed: 1,
                total: 2
            }
        );

        // Children keep an epic from being demoted, and closing it leaves them.
        assert!(b.demote(e).is_err());
        b.move_to(e, Status::Closed, None).unwrap();
        assert_eq!(b.get(x).unwrap().status, Status::Backlog);
        assert_eq!(b.get(x).unwrap().parent, Some(e));
        b.set_parent(x, None).unwrap();
        b.set_parent(y, None).unwrap();
        b.demote(e).unwrap();
        assert!(!b.get(e).unwrap().epic);
    }

    #[test]
    fn epics_are_one_level_deep_and_stay_in_their_project() {
        let mut b = Board::default();
        let e = epic(&mut b, "e");
        let other = epic(&mut b, "other");
        let x = titled(&mut b, "x", Status::Open);
        let y = titled(&mut b, "y", Status::Open);

        assert!(b.set_parent(x, Some(y)).is_err()); // not an epic
        assert!(b.set_parent(x, Some(x)).is_err());
        assert!(b.set_parent(e, Some(other)).is_err()); // epics have no parent
        assert!(b.set_parent(e, Some(e)).is_err());
        b.set_parent(x, Some(e)).unwrap();
        assert!(b.promote(x).is_err());

        let elsewhere = |parent, epic| NewTask {
            title: "z".into(),
            project: "/q".into(),
            epic,
            parent,
            ..Default::default()
        };
        assert!(b.create(elsewhere(Some(e), false)).is_err());
        assert!(b.create(elsewhere(Some(e), true)).is_err());
        assert_eq!(b.tasks.len(), 4);
        // Nothing is registered by a create that failed.
        assert!(b.project(Path::new("/q")).is_none());
        let q = b.create(elsewhere(None, false)).unwrap();
        assert!(b.set_parent(q, Some(e)).is_err());
    }

    #[test]
    fn moving_projects_keeps_children_with_their_epic() {
        let mut b = Board::default();
        let e = epic(&mut b, "e");
        let x = titled(&mut b, "x", Status::Open);
        b.set_parent(x, Some(e)).unwrap();
        let to = |project: &str, parent| TaskPatch {
            project: Some(project.into()),
            parent,
            ..Default::default()
        };

        // A child can't leave on its own, but saving it unmoved is fine.
        let err = b.edit(x, to("/q", None)).unwrap_err().to_string();
        assert!(err.contains("leave the epic"), "{err}");
        b.edit(x, to("/p", None)).unwrap();

        b.edit(e, to("/q", None)).unwrap();
        assert_eq!(b.get(x).unwrap().project, Path::new("/q"));
        assert_eq!(b.get(x).unwrap().parent, Some(e));

        // Leaving the epic in the same edit lets it go.
        b.edit(x, to("/p", Some(None))).unwrap();
        assert_eq!(b.get(x).unwrap().project, Path::new("/p"));
        assert_eq!(b.get(x).unwrap().parent, None);
    }

    #[test]
    fn deleting_an_epic_keeps_its_children() {
        let mut b = Board::default();
        let e = epic(&mut b, "e");
        let x = titled(&mut b, "x", Status::Open);
        b.set_parent(x, Some(e)).unwrap();
        b.delete(e).unwrap();
        assert_eq!(b.get(x).unwrap().parent, None);
    }

    #[test]
    fn a_dangling_parent_counts_as_none() {
        let mut b = Board::default();
        let x = titled(&mut b, "x", Status::Open);
        b.get_mut(x).unwrap().parent = Some(Ulid::generate());
        assert!(b.parent_of(b.get(x).unwrap()).is_none());

        // It doesn't get in the way of anything, and promoting drops it.
        b.edit(
            x,
            TaskPatch {
                project: Some("/q".into()),
                ..Default::default()
            },
        )
        .unwrap();
        b.promote(x).unwrap();
        assert_eq!(b.get(x).unwrap().parent, None);
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
