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
        self.add_file(id, name, |dest| fs::copy(src, dest).map(|_| ()))
    }

    /// Creates an empty file named `name` in the store and attaches it to
    /// task `id`; see [`new_file_name`] for how the name is read. Like an
    /// attached file, it may share its name with another of the task's files.
    pub fn create_file(&self, id: Ulid, name: &str) -> Result<Attachment> {
        let name = new_file_name(name)?;
        self.add_file(id, name, |dest| fs::write(dest, b""))
    }

    /// Makes the store's copy of a file named `name` with `fill`, then
    /// attaches it to task `id`. The copy is removed if attaching fails.
    fn add_file(
        &self,
        id: Ulid,
        name: String,
        fill: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<Attachment> {
        let file_id = Ulid::generate();
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let rel = format!("{FILES_DIR}/{file_id}{ext}");
        let dest = self.dir.join(&rel);
        fill(&dest)?;

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

    /// Replaces the contents of task `id`'s file `file_id` and marks the task
    /// updated. The file is swapped in whole, so readers never see it half
    /// written.
    pub fn write_file(&self, id: Ulid, file_id: Ulid, contents: &[u8]) -> Result<()> {
        // Written under the board's lock so the file can't be detached meanwhile.
        self.update(|b| {
            let task = b
                .tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            let att = task
                .files
                .iter()
                .find(|f| f.id == file_id)
                .ok_or_else(|| Error::NotFound(file_id.to_string()))?;
            let mut tmp = tempfile::NamedTempFile::new_in(self.dir.join(FILES_DIR))?;
            tmp.write_all(contents)?;
            tmp.as_file().sync_all()?;
            tmp.persist(self.file_path(att))
                .map_err(|e| Error::Io(e.error))?;
            task.updated = Utc::now();
            Ok(())
        })
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

/// The extension a new file gets when its name has none.
pub const DEFAULT_FILE_EXTENSION: &str = "md";

/// Reads the name typed for a new file: trimmed of spaces and trailing dots,
/// and given a `.md` extension if it has none. Dotfiles like `.gitignore`
/// are kept as typed. It must be a plain file name, not a path, and not an
/// image, since an empty one can't be edited.
pub fn new_file_name(typed: &str) -> Result<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(Error::Invalid("the file needs a name".into()));
    }
    if typed.contains(['/', '\\']) {
        return Err(Error::Invalid(format!(
            "'{typed}' is a path; give just the file's name"
        )));
    }
    let name = typed.trim_end_matches('.');
    if name.is_empty() {
        return Err(Error::Invalid(format!("'{typed}' isn't a file name")));
    }
    // A leading dot starts a hidden file's name rather than an extension.
    let name = if name.starts_with('.') || Path::new(name).extension().is_some() {
        name.to_string()
    } else {
        format!("{name}.{DEFAULT_FILE_EXTENSION}")
    };
    if FileKind::from_name(&name) == FileKind::Image {
        return Err(Error::Invalid(format!(
            "'{name}' would be an empty image; attach an image instead"
        )));
    }
    Ok(name)
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
    fn new_file_names_default_to_markdown() {
        assert_eq!(new_file_name(" notes ").unwrap(), "notes.md");
        assert_eq!(new_file_name("data.json").unwrap(), "data.json");
        assert_eq!(new_file_name("plan.").unwrap(), "plan.md");
        assert_eq!(new_file_name("notes.md.").unwrap(), "notes.md");
        assert_eq!(new_file_name(".gitignore").unwrap(), ".gitignore");
        assert_eq!(new_file_name(".env.").unwrap(), ".env");
        assert_eq!(new_file_name("v1.2 notes.txt").unwrap(), "v1.2 notes.txt");
        for bad in ["", "  ", "a/b.md", "a\\b", "..", ".", "shot.png"] {
            assert!(new_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn created_files_can_be_written_and_bump_updated() {
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

        let att = store.create_file(id, "notes").unwrap();
        assert_eq!(att.name, "notes.md");
        assert_eq!(att.kind, FileKind::Markdown);
        assert!(att.path.ends_with(".md"));
        assert_eq!(fs::read_to_string(store.file_path(&att)).unwrap(), "");
        // Names may repeat, as with attached files.
        let again = store.create_file(id, "notes.md").unwrap();
        assert_ne!(again.path, att.path);
        assert!(store.create_file(id, "../escape.md").is_err());

        let before = store.load().unwrap().get(id).unwrap().updated;
        store.write_file(id, att.id, b"# plan\n").unwrap();
        assert_eq!(
            fs::read_to_string(store.file_path(&att)).unwrap(),
            "# plan\n"
        );
        let task = store.load().unwrap().get(id).unwrap().clone();
        assert!(task.updated >= before);
        assert_eq!(task.files.len(), 2);

        // Neither creating on a missing task nor writing a missing file
        // leaves anything behind.
        assert!(store.create_file(Ulid::generate(), "x").is_err());
        assert!(store.write_file(id, Ulid::generate(), b"x").is_err());
        let stored = fs::read_dir(dir.path().join(FILES_DIR)).unwrap().count();
        assert_eq!(stored, 2);
    }

    #[test]
    fn write_file_replaces_contents_without_leaving_temp_files() {
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
        let src = dir.path().join("long.txt");
        fs::write(&src, "a much longer original text\n").unwrap();
        let att = store.attach(id, &src).unwrap();

        store.write_file(id, att.id, b"short").unwrap();
        store.write_file(id, att.id, b"shorter").unwrap();
        assert_eq!(
            fs::read_to_string(store.file_path(&att)).unwrap(),
            "shorter"
        );
        let stored: Vec<_> = fs::read_dir(dir.path().join(FILES_DIR))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(stored, [store.file_path(&att)]);
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
