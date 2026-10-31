use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use ulid::Ulid;

use crate::hooks::{self, TaskEvent};
use crate::model::{Attachment, Board, FileKind, Project, SCHEMA_VERSION};
use crate::{Error, Result};

/// Where projects kept their tasks before the store was shared.
pub const LEGACY_DIR: &str = ".tb";
/// What a project's legacy directory is renamed to once imported.
pub const IMPORTED_DIR: &str = ".tb.imported";
const TASKS_FILE: &str = "tasks.json";
const LOCK_FILE: &str = "lock";
const FILES_DIR: &str = "files";

/// Finds the project root for `start`: the enclosing git repository's root if
/// there is one, otherwise `start` itself.
pub fn project_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(start)
        .to_path_buf()
}

/// The absolute form of a project directory the user named: a leading `~/`
/// is expanded, a relative path is taken from the current directory, and
/// symlinks are resolved if the directory exists.
pub fn project_path(dir: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = match (dir.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => dir.to_path_buf(),
    };
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    fs::canonicalize(&dir).unwrap_or(dir)
}

/// Like [`project_path`], creating the directory first if it doesn't exist.
pub fn create_project_dir(dir: &Path) -> Result<PathBuf> {
    let dir = project_path(dir);
    if !dir.is_dir() {
        fs::create_dir_all(&dir)
            .map_err(|e| Error::Invalid(format!("couldn't create {}: {e}", dir.display())))?;
    }
    Ok(fs::canonicalize(&dir)?)
}

/// Handle on the directory holding every project's tasks.
///
/// Readers never lock: writes replace `tasks.json` with an atomic rename, so a
/// read always sees a whole file. Writers serialize on an advisory lock so
/// concurrent processes (GUI and CLI) can't lose each other's changes.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `$XDG_DATA_HOME/tb`, or `~/.local/share/tb`. `None` if neither
    /// variable is set.
    pub fn default_dir() -> Option<PathBuf> {
        crate::xdg_dir("XDG_DATA_HOME", ".local/share")
    }

    /// Opens the store at [`Store::default_dir`].
    pub fn open_default() -> Result<Store> {
        let dir = Store::default_dir()
            .ok_or_else(|| Error::Invalid("neither XDG_DATA_HOME nor HOME is set".into()))?;
        Store::open(&dir)
    }

    /// Opens the store in `dir`, creating it if needed.
    pub fn open(dir: &Path) -> Result<Store> {
        fs::create_dir_all(dir.join(FILES_DIR))?;
        Ok(Store {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn tasks_path(&self) -> PathBuf {
        self.dir.join(TASKS_FILE)
    }

    pub fn file_path(&self, att: &Attachment) -> PathBuf {
        self.dir.join(&att.path)
    }

    /// Reads the board; a missing file is an empty board.
    pub fn load(&self) -> Result<Board> {
        match fs::read_to_string(self.tasks_path()) {
            Ok(text) => parse_board(&text, None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Board::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Locks, reloads the latest board from disk, applies `f`, and writes the
    /// result atomically. Nothing is written if `f` fails.
    pub fn update<T>(&self, f: impl FnOnce(&mut Board) -> Result<T>) -> Result<T> {
        self.locked_update(f)
    }

    /// Like [`Store::update`], also returning the tasks `f` created or moved
    /// to another status, for running hooks.
    pub fn update_with_events<T>(
        &self,
        f: impl FnOnce(&mut Board) -> Result<T>,
    ) -> Result<(T, Vec<TaskEvent>)> {
        self.locked_update(|board| {
            let before = board.clone();
            let out = f(board)?;
            Ok((out, hooks::events(&before, board)))
        })
    }

    fn locked_update<T>(&self, f: impl FnOnce(&mut Board) -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(LOCK_FILE))?;
        lock.lock()?;
        let mut board = self.load()?;
        let out = f(&mut board)?;
        self.write(&board)?;
        // The lock is released when `lock` drops.
        Ok(out)
    }

    fn write(&self, board: &Board) -> Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(&mut tmp, board)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.tasks_path())
            .map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// Copies `src` into the store's `files/` and attaches it to task `id`.
    pub fn attach(&self, id: Ulid, src: &Path) -> Result<Attachment> {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Invalid(format!("bad file name: {}", src.display())))?
            .to_string();
        let file_id = Ulid::generate();
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let rel = format!("{FILES_DIR}/{file_id}{ext}");
        let dest = self.dir.join(&rel);
        fs::copy(src, &dest)?;

        let att = Attachment {
            id: file_id,
            kind: FileKind::from_name(&name),
            name,
            path: rel,
        };
        let added = att.clone();
        let res = self.update(|b| {
            let task = b
                .tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            task.files.push(added);
            task.updated = Utc::now();
            Ok(())
        });
        if let Err(e) = res {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
        Ok(att)
    }

    /// Detaches file `file_id` from task `id` and deletes its copy.
    pub fn detach(&self, id: Ulid, file_id: Ulid) -> Result<Attachment> {
        let att = self.update(|b| {
            let task = b
                .tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            let idx = task
                .files
                .iter()
                .position(|f| f.id == file_id)
                .ok_or_else(|| Error::NotFound(file_id.to_string()))?;
            task.updated = Utc::now();
            Ok(task.files.remove(idx))
        })?;
        let _ = fs::remove_file(self.file_path(&att));
        Ok(att)
    }

    /// Moves the tasks and files from `project`'s legacy `.tb/` directory into
    /// the store, then renames that directory to `.tb.imported`. Tasks already
    /// in the store are skipped, so an import interrupted before the rename can
    /// simply run again. Returns how many tasks were imported; 0 if there's
    /// nothing to import.
    pub fn import_legacy(&self, project: &Path) -> Result<usize> {
        let legacy = project.join(LEGACY_DIR);
        let text = match fs::read_to_string(legacy.join(TASKS_FILE)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        let project = project_path(project);
        let old = parse_board(&text, Some(&project))?;
        let imported = self.update(|board| {
            let mut tasks: Vec<_> = old
                .tasks
                .into_iter()
                .filter(|t| board.get(t.id).is_none())
                .collect();
            // Imported tasks go below each column's existing ones, in their order.
            tasks.sort_by_key(|t| (t.status, t.order, t.id));
            for mut task in tasks.iter().cloned() {
                for att in &task.files {
                    // Attachment paths are relative, so they carry over as-is.
                    let dest = self.file_path(att);
                    if !dest.exists() {
                        fs::copy(legacy.join(&att.path), dest)?;
                    }
                }
                task.order = board.column(task.status).len() as u32;
                board.tasks.push(task);
            }
            if !tasks.is_empty() && board.project(&project).is_none() {
                board.projects.push(Project {
                    path: project.clone(),
                });
            }
            Ok(tasks.len())
        })?;
        fs::rename(&legacy, project.join(IMPORTED_DIR))?;
        Ok(imported)
    }

    /// Removes the project at `path` along with its tasks and their files.
    /// Returns how many tasks were deleted.
    pub fn delete_project(&self, path: &Path) -> Result<usize> {
        let tasks = self.update(|b| b.remove_project(path))?;
        for att in tasks.iter().flat_map(|t| &t.files) {
            let _ = fs::remove_file(self.file_path(att));
        }
        Ok(tasks.len())
    }

    /// Deletes task `id` along with its attached files.
    pub fn delete(&self, id: Ulid) -> Result<()> {
        let task = self.update(|b| b.delete(id))?;
        for att in &task.files {
            let _ = fs::remove_file(self.file_path(att));
        }
        Ok(())
    }
}

/// Parses `tasks.json`, migrating older schemas to the current one. Version 1
/// boards belonged to a single project, given as `legacy_project`.
pub fn parse_board(text: &str, legacy_project: Option<&Path>) -> Result<Board> {
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    let version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::Invalid("tasks.json has no schema_version".into()))?
        as u32;
    if version > SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema(version));
    }
    // Each migration upgrades `value` by one version.
    if version < 2 {
        let project = legacy_project
            .ok_or_else(|| Error::Invalid("tasks.json is from a single-project board".into()))?;
        let project = serde_json::to_value(project)?;
        if let Some(tasks) = value.get_mut("tasks").and_then(|t| t.as_array_mut()) {
            for task in tasks.iter_mut().filter_map(|t| t.as_object_mut()) {
                task.insert("project".into(), project.clone());
            }
        }
        value["schema_version"] = SCHEMA_VERSION.into();
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::NewTask;
    use crate::model::Status;

    #[test]
    fn update_persists_and_attaches_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let id = store
            .update(|b| {
                b.create(NewTask {
                    title: "first".into(),
                    project: dir.path().to_path_buf(),
                    ..Default::default()
                })
            })
            .unwrap();

        let src = dir.path().join("notes.md");
        fs::write(&src, "# hi").unwrap();
        let att = store.attach(id, &src).unwrap();

        let board = Store::open(dir.path()).unwrap().load().unwrap();
        let task = board.get(id).unwrap();
        assert_eq!(task.files[0].name, "notes.md");
        assert_eq!(task.files[0].kind, FileKind::Markdown);
        assert_eq!(fs::read_to_string(store.file_path(&att)).unwrap(), "# hi");
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        Store::open(&root).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    // Each thread uses its own handle, like separate processes would.
                    let store = Store::open(&root).unwrap();
                    store
                        .update(|b| {
                            b.create(NewTask {
                                title: format!("t{i}"),
                                project: "/p".into(),
                                ..Default::default()
                            })
                        })
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(Store::open(&root).unwrap().load().unwrap().tasks.len(), 8);
    }

    #[test]
    fn rejects_newer_schema() {
        let err = parse_board(r#"{"schema_version": 999, "tasks": []}"#, None).unwrap_err();
        assert!(matches!(err, Error::UnsupportedSchema(999)));
    }

    #[test]
    fn imports_a_legacy_project_board_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data")).unwrap();
        let existing = store
            .update(|b| {
                b.create(NewTask {
                    title: "existing".into(),
                    project: "/elsewhere".into(),
                    status: Some(Status::Backlog),
                    ..Default::default()
                })
            })
            .unwrap();

        // A version 1 board, as `.tb/` held it, with one attached file.
        let project = dir.path().join("app");
        let legacy = project.join(LEGACY_DIR);
        fs::create_dir_all(legacy.join("files")).unwrap();
        fs::write(legacy.join("files/01M5X724HRT6FPK984034NF3M3.md"), "# hi").unwrap();
        fs::write(
            legacy.join("tasks.json"),
            r#"{"schema_version": 1, "tasks": [{
                "id": "01M5X724HRT6FPK984034NF3M2", "title": "old", "status": "backlog",
                "order": 0, "created": "2026-10-26T22:59:53Z", "updated": "2026-10-26T22:59:53Z",
                "files": [{"id": "01M5X724HRT6FPK984034NF3M3", "name": "notes.md",
                           "path": "files/01M5X724HRT6FPK984034NF3M3.md", "kind": "markdown"}]
            }]}"#,
        )
        .unwrap();

        assert_eq!(store.import_legacy(&project).unwrap(), 1);
        assert!(!legacy.exists());
        assert!(project.join(IMPORTED_DIR).join("tasks.json").exists());
        assert_eq!(store.import_legacy(&project).unwrap(), 0);

        let board = store.load().unwrap();
        let ids: Vec<Ulid> = board.column(Status::Backlog).iter().map(|t| t.id).collect();
        let old: Ulid = "01M5X724HRT6FPK984034NF3M2".parse().unwrap();
        assert_eq!(ids, [existing, old]);
        let task = board.get(old).unwrap();
        let project = fs::canonicalize(&project).unwrap();
        assert_eq!(task.project, project);
        assert_eq!(board.project_name(&project), "app");
        assert_eq!(
            fs::read_to_string(store.file_path(&task.files[0])).unwrap(),
            "# hi"
        );
    }
}
