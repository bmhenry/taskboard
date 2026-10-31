use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use ulid::Ulid;

use crate::hooks::{self, TaskEvent};
use crate::model::{Attachment, Board, FileKind, SCHEMA_VERSION};
use crate::{Error, Result};

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
            Ok(text) => parse_board(&text),
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

/// Parses `tasks.json`, migrating older schemas to the current one.
pub fn parse_board(text: &str) -> Result<Board> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    let version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::Invalid("tasks.json has no schema_version".into()))?
        as u32;
    if version > SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema(version));
    }
    // Migrations from older versions go here, each upgrading `value` by one step.
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::NewTask;

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
        let err = parse_board(r#"{"schema_version": 999, "tasks": []}"#).unwrap_err();
        assert!(matches!(err, Error::UnsupportedSchema(999)));
    }
}
