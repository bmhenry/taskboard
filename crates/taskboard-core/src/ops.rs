//! Every change the GUI makes to the store, as a serializable [`Command`],
//! and [`execute`], which applies one and runs the user's hooks. The desktop
//! app and the web daemon both run commands through here, so they behave the
//! same.
//!
//! On the wire every enum here is internally tagged with a snake_case
//! `"type"` field, e.g. `{"type": "move", "id": "…", "status": "closed"}`,
//! and file contents are base64 strings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::board::{NewTask, TaskPatch};
use crate::hooks::{self, TaskEvent};
use crate::model::{Attachment, Board, Link, LinkKind, Status};
use crate::settings::{GlobalSettings, NewTaskPosition};
use crate::store::{ProjectInput, Store, create_project_dir, project_path, resolve_project};
use crate::{Error, Result};

/// A change to the store.
///
/// Paths are on the machine that runs [`execute`]: for the web UI, the
/// daemon's, not the browser's. Commands that read files from that machine
/// are trusted, local-only operations; see [`Command::is_remote_safe`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Adds a task with its links, then attaches its files. A file that
    /// can't be attached doesn't undo the task; see
    /// [`Outcome::attach_failures`].
    Create {
        title: String,
        /// The project as typed: a project's name or a directory, which is
        /// created if it doesn't exist; see [`resolve_project`].
        project: String,
        #[serde(default)]
        description: String,
        status: Option<Status>,
        #[serde(default)]
        links: Vec<Link>,
        /// Files to attach. [`FileSource::Path`] reads a file on the machine
        /// running [`execute`], so it's only for trusted, local callers.
        #[serde(default)]
        files: Vec<FileSource>,
        /// Top or bottom of the column; `None` uses
        /// [`GlobalSettings::new_task_position`].
        position: Option<NewTaskPosition>,
        #[serde(default)]
        epic: bool,
        parent: Option<Ulid>,
        /// The new task's id, chosen by the caller so it can show the task
        /// before this runs; a new one if `None`. It must be unused.
        id: Option<Ulid>,
    },
    /// Changes a task's title, project or description; `None` leaves one alone.
    Edit {
        id: Ulid,
        title: Option<String>,
        /// The project as typed, as for [`Command::Create`].
        project: Option<String>,
        description: Option<String>,
    },
    /// Moves a task to `position` in `status`'s column (its end if `None`).
    Move {
        id: Ulid,
        status: Status,
        position: Option<usize>,
    },
    /// Deletes a task and its files.
    Delete {
        id: Ulid,
    },
    /// Makes a task an epic.
    Promote {
        id: Ulid,
    },
    /// Makes an epic a normal task.
    Demote {
        id: Ulid,
    },
    /// Makes a task a child of the epic `parent`, or of none.
    SetParent {
        id: Ulid,
        parent: Option<Ulid>,
    },
    /// Adds a link; the URL is trimmed, and a blank one is ignored.
    AddLink {
        task: Ulid,
        url: String,
        #[serde(default)]
        kind: LinkKind,
    },
    /// Removes the task's link at `index` (0-based).
    RemoveLink {
        task: Ulid,
        index: usize,
    },
    /// Attaches a file. [`FileSource::Path`] reads a file on the machine
    /// running [`execute`], so it's only for trusted, local callers.
    Attach {
        task: Ulid,
        source: FileSource,
    },
    Detach {
        task: Ulid,
        file: Ulid,
    },
    /// Creates an empty file and attaches it; see [`crate::new_file_name`]
    /// for how `name` is read.
    NewFile {
        task: Ulid,
        name: String,
    },
    /// Replaces the contents of one of the task's files.
    WriteFile {
        task: Ulid,
        file: Ulid,
        text: String,
    },
    /// Points a project and its tasks at the directory typed in `to`
    /// (normalized with [`project_path`]), which must exist.
    RelocateProject {
        from: PathBuf,
        to: String,
    },
    /// Deletes a project with all its tasks and the store's copies of their
    /// files, which can't be undone. It removes nothing outside the store:
    /// the project's own directory is left alone.
    DeleteProject {
        path: PathBuf,
    },
}

impl Command {
    /// Whether the command is safe to take from a remote client: it doesn't
    /// read files from the machine running [`execute`] (no
    /// [`FileSource::Path`]). The web daemon refuses the rest.
    pub fn is_remote_safe(&self) -> bool {
        match self {
            Command::Create { files, .. } => files.iter().all(FileSource::is_remote_safe),
            Command::Attach { source, .. } => source.is_remote_safe(),
            _ => true,
        }
    }
}

/// Where a file to attach comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileSource {
    /// A file on the machine running [`execute`], copied into the store.
    /// Trusted, local callers only: a remote client could use it to read any
    /// file the daemon can.
    Path { path: PathBuf },
    /// A file's contents, e.g. a browser upload; base64 on the wire. `name`
    /// must be a plain file name; see [`Store::attach_bytes`].
    Bytes {
        name: String,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
}

impl FileSource {
    /// The name the file is shown by.
    pub fn name(&self) -> String {
        match self {
            FileSource::Path { path } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            FileSource::Bytes { name, .. } => name.clone(),
        }
    }

    /// See [`Command::is_remote_safe`].
    pub fn is_remote_safe(&self) -> bool {
        matches!(self, FileSource::Bytes { .. })
    }

    fn attach(&self, store: &Store, task: Ulid) -> Result<Attachment> {
        match self {
            FileSource::Path { path } => store.attach(task, path),
            FileSource::Bytes { name, bytes } => store.attach_bytes(task, name, bytes),
        }
    }
}

/// What a command produced, beyond the change to the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Output {
    Done,
    /// The new task, from [`Command::Create`].
    Created {
        id: Ulid,
    },
    /// The file added by [`Command::Attach`] or [`Command::NewFile`].
    File {
        file: Attachment,
    },
    /// The project's new directory, from [`Command::RelocateProject`].
    Relocated {
        path: PathBuf,
    },
    /// How many tasks [`Command::DeleteProject`] deleted.
    ProjectDeleted {
        tasks: usize,
    },
}

/// The result of a command that succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub output: Output,
    /// Tasks the command created or moved to another status; hooks were
    /// started for these.
    pub events: Vec<TaskEvent>,
    /// A message for each hook that couldn't be started.
    pub hook_failures: Vec<String>,
    /// For [`Command::Create`], each file that couldn't be attached, as
    /// "name (error)".
    pub attach_failures: Vec<String>,
}

impl Outcome {
    /// What to show the user about hooks that didn't start and files that
    /// weren't attached, if anything.
    pub fn message(&self) -> Option<String> {
        let mut parts = self.hook_failures.clone();
        if !self.attach_failures.is_empty() {
            parts.push(format!(
                "Couldn't attach: {}",
                self.attach_failures.join(", ")
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Applies `command` to the store, then starts the hooks in `settings`
/// matching what it did. If it fails, the store is left as it was (a
/// project directory it created is removed again) and no hooks run.
pub fn execute(store: &Store, settings: &GlobalSettings, command: Command) -> Result<Outcome> {
    let mut attach_failures = Vec::new();
    let (output, events) = match command {
        Command::Create {
            title,
            project,
            description,
            status,
            links,
            files,
            position,
            epic,
            parent,
            id,
        } => {
            let position = position.unwrap_or(settings.new_task_position);
            let mut made = MadeDirs::default();
            let res = store.update_with_events(|b| {
                // Checked before the project, so a bad title doesn't leave
                // behind a newly created directory.
                check_title(&title)?;
                let project = task_project(b, &project, &mut made)?;
                b.create(NewTask {
                    title,
                    project,
                    description,
                    status,
                    links,
                    position,
                    epic,
                    parent,
                    id,
                })
            });
            let (id, events) = made.keep_if(res)?;
            // Hooks run once the files are attached, so they see the whole task.
            for file in &files {
                if let Err(e) = file.attach(store, id) {
                    attach_failures.push(format!("{} ({e})", file.name()));
                }
            }
            (Output::Created { id }, events)
        }
        Command::Edit {
            id,
            title,
            project,
            description,
        } => {
            let mut made = MadeDirs::default();
            let res = store.update(|b| {
                if let Some(title) = &title {
                    check_title(title)?;
                }
                let project = match project {
                    Some(text) => Some(task_project(b, &text, &mut made)?),
                    None => None,
                };
                b.edit(
                    id,
                    TaskPatch {
                        title,
                        project,
                        description,
                        ..Default::default()
                    },
                )
            });
            made.keep_if(res)?;
            no_events(Output::Done)
        }
        Command::Move {
            id,
            status,
            position,
        } => {
            let ((), events) = store.update_with_events(|b| b.move_to(id, status, position))?;
            (Output::Done, events)
        }
        Command::Delete { id } => {
            store.delete(id)?;
            no_events(Output::Done)
        }
        Command::Promote { id } => {
            store.update(|b| b.promote(id))?;
            no_events(Output::Done)
        }
        Command::Demote { id } => {
            store.update(|b| b.demote(id))?;
            no_events(Output::Done)
        }
        Command::SetParent { id, parent } => {
            store.update(|b| b.set_parent(id, parent))?;
            no_events(Output::Done)
        }
        Command::AddLink { task, url, kind } => {
            let url = url.trim().to_string();
            if !url.is_empty() {
                let link = Link {
                    url,
                    label: None,
                    kind,
                };
                store.update(|b| b.add_link(task, link))?;
            }
            no_events(Output::Done)
        }
        Command::RemoveLink { task, index } => {
            store.update(|b| b.remove_link(task, index))?;
            no_events(Output::Done)
        }
        Command::Attach { task, source } => no_events(Output::File {
            file: source.attach(store, task)?,
        }),
        Command::Detach { task, file } => {
            store.detach(task, file)?;
            no_events(Output::Done)
        }
        Command::NewFile { task, name } => no_events(Output::File {
            file: store.create_file(task, &name)?,
        }),
        Command::WriteFile { task, file, text } => {
            store.write_file(task, file, text.as_bytes())?;
            no_events(Output::Done)
        }
        Command::RelocateProject { from, to } => {
            let to = to.trim();
            if to.is_empty() {
                return Err(Error::Invalid("choose the project's new folder".into()));
            }
            let to = project_path(Path::new(to));
            if !to.is_dir() {
                return Err(Error::Invalid(format!("no folder at {}", to.display())));
            }
            store.update(|b| b.relocate_project(&from, to.clone()))?;
            no_events(Output::Relocated { path: to })
        }
        Command::DeleteProject { path } => {
            let tasks = store.delete_project(&path)?;
            no_events(Output::ProjectDeleted { tasks })
        }
    };
    let hook_failures = hooks::run(&settings.hooks, &events);
    Ok(Outcome {
        output,
        events,
        hook_failures,
        attach_failures,
    })
}

fn no_events(output: Output) -> (Output, Vec<TaskEvent>) {
    (output, Vec::new())
}

fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(Error::Invalid("title must not be empty".into()));
    }
    Ok(())
}

/// The directory of the project typed in `text`, creating it if it doesn't
/// exist and recording what was created in `made`.
fn task_project(board: &Board, text: &str, made: &mut MadeDirs) -> Result<PathBuf> {
    match resolve_project(board, text) {
        ProjectInput::Empty => Err(Error::Invalid("choose a project for the task".into())),
        ProjectInput::Known(path) | ProjectInput::New(path) => Ok(path),
        ProjectInput::Missing(path) => {
            // Deepest first, so they can be removed in order.
            made.0 = path
                .ancestors()
                .take_while(|dir| !dir.exists())
                .map(Path::to_path_buf)
                .collect();
            create_project_dir(&path)
        }
    }
}

/// Directories a command created for a new project, removed again if the
/// command fails.
#[derive(Default)]
struct MadeDirs(Vec<PathBuf>);

impl MadeDirs {
    fn keep_if<T>(self, res: Result<T>) -> Result<T> {
        if res.is_err() {
            for dir in &self.0 {
                // Only ever removes an empty directory.
                let _ = std::fs::remove_dir(dir);
            }
        }
        res
    }
}

/// Serializes bytes as a base64 string.
mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::hooks::{Hook, HookEvent};
    use crate::model::FileKind;

    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        settings: GlobalSettings,
        /// A project directory that exists, canonical.
        project: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let project = dir.path().join("proj");
        fs::create_dir(&project).unwrap();
        Fixture {
            store,
            settings: GlobalSettings::default(),
            project: fs::canonicalize(project).unwrap(),
            dir,
        }
    }

    impl Fixture {
        fn run(&self, command: Command) -> Result<Outcome> {
            execute(&self.store, &self.settings, command)
        }

        fn create_in(&self, title: &str, project: &str) -> Result<Ulid> {
            match self.run(create(title, project))?.output {
                Output::Created { id } => Ok(id),
                other => panic!("expected Created, got {other:?}"),
            }
        }

        fn create(&self, title: &str) -> Ulid {
            self.create_in(title, &self.project.display().to_string())
                .unwrap()
        }

        fn board(&self) -> Board {
            self.store.load().unwrap()
        }

        /// The store's files, sorted.
        fn stored_files(&self) -> Vec<PathBuf> {
            let mut files: Vec<_> = fs::read_dir(self.store.dir().join("files"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            files.sort();
            files
        }
    }

    fn create(title: &str, project: &str) -> Command {
        Command::Create {
            title: title.into(),
            project: project.into(),
            description: String::new(),
            status: None,
            links: Vec::new(),
            files: Vec::new(),
            position: None,
            epic: false,
            parent: None,
            id: None,
        }
    }

    fn file(out: &Outcome) -> Attachment {
        match &out.output {
            Output::File { file } => file.clone(),
            other => panic!("expected File, got {other:?}"),
        }
    }

    fn bytes(name: &str) -> FileSource {
        FileSource::Bytes {
            name: name.into(),
            bytes: b"x".to_vec(),
        }
    }

    #[test]
    fn create_saves_links_and_files_and_reports_bad_ones() {
        let f = fixture();
        let src = f.dir.path().join("notes.md");
        fs::write(&src, "# notes").unwrap();
        let link = Link {
            url: "https://example.com".into(),
            label: None,
            kind: LinkKind::Context,
        };
        let out = f
            .run(Command::Create {
                title: " first ".into(),
                project: f.project.display().to_string(),
                description: "desc".into(),
                status: Some(Status::Backlog),
                links: vec![link.clone()],
                files: vec![
                    FileSource::Path { path: src },
                    FileSource::Bytes {
                        name: "shot.png".into(),
                        bytes: vec![1, 2, 3],
                    },
                    FileSource::Path {
                        path: f.dir.path().join("gone.txt"),
                    },
                ],
                position: None,
                epic: false,
                parent: None,
                id: None,
            })
            .unwrap();
        let Output::Created { id } = out.output else {
            panic!("{out:?}");
        };
        assert_eq!(out.attach_failures.len(), 1);
        assert!(out.attach_failures[0].starts_with("gone.txt ("));
        let msg = out.message().unwrap();
        assert!(msg.starts_with("Couldn't attach: gone.txt ("), "{msg}");

        let board = f.board();
        let task = board.get(id).unwrap();
        assert_eq!(task.title, "first");
        assert_eq!(task.project, f.project);
        assert_eq!(task.description, "desc");
        assert_eq!(task.status, Status::Backlog);
        assert_eq!(task.links, [link]);
        let names: Vec<_> = task.files.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["notes.md", "shot.png"]);
        assert_eq!(task.files[1].kind, FileKind::Image);
        assert_eq!(
            fs::read(f.store.file_path(&task.files[1])).unwrap(),
            [1, 2, 3]
        );
    }

    #[test]
    fn create_uses_the_id_given_unless_taken() {
        let f = fixture();
        let project = f.project.display().to_string();
        let chosen = Ulid::generate();
        let mut cmd = create("chosen", &project);
        if let Command::Create { id, .. } = &mut cmd {
            *id = Some(chosen);
        }
        let out = f.run(cmd.clone()).unwrap();
        assert_eq!(out.output, Output::Created { id: chosen });
        assert_eq!(f.board().get(chosen).unwrap().title, "chosen");

        // Taken: refused, leaving the store as it was.
        let before = f.board();
        let err = f.run(cmd).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(f.board(), before);
    }

    #[test]
    fn new_tasks_follow_the_position_setting_unless_given_one() {
        let mut f = fixture();
        let first = f.create("first");
        let column = |f: &Fixture| -> Vec<Ulid> {
            f.board()
                .column(Status::Open)
                .iter()
                .map(|t| t.id)
                .collect()
        };
        let top = f.create("top");
        assert_eq!(column(&f), [top, first]);

        f.settings.new_task_position = NewTaskPosition::Bottom;
        let bottom = f.create("bottom");
        assert_eq!(column(&f), [top, first, bottom]);

        let mut cmd = create("explicit top", &f.project.display().to_string());
        if let Command::Create { position, .. } = &mut cmd {
            *position = Some(NewTaskPosition::Top);
        }
        let Output::Created { id } = f.run(cmd).unwrap().output else {
            panic!();
        };
        assert_eq!(column(&f), [id, top, first, bottom]);
    }

    #[test]
    fn projects_are_resolved_like_the_editor_does() {
        let f = fixture();
        let a = f.create("a");

        // By name.
        let by_name = f.create_in("b", "proj").unwrap();
        assert_eq!(f.board().get(by_name).unwrap().project, f.project);

        // Through a symlink, stored as the real directory.
        let link = f.dir.path().join("link");
        std::os::unix::fs::symlink(&f.project, &link).unwrap();
        let linked = f.create_in("c", &link.display().to_string()).unwrap();
        assert_eq!(f.board().get(linked).unwrap().project, f.project);

        // A relative path is taken from the current directory, whatever it is.
        let rel = f.create_in("d", "./").unwrap();
        assert_eq!(
            f.board().get(rel).unwrap().project,
            fs::canonicalize(std::env::current_dir().unwrap()).unwrap()
        );

        // A blank project is refused with the editor's message.
        for blank in ["", "  "] {
            let err = f.create_in("e", blank).unwrap_err();
            assert_eq!(err.to_string(), "choose a project for the task");
            let err = f
                .run(Command::Edit {
                    id: a,
                    title: None,
                    project: Some(blank.into()),
                    description: None,
                })
                .unwrap_err();
            assert_eq!(err.to_string(), "choose a project for the task");
        }

        // A missing directory is created, nested ones included.
        let new_dir = f.dir.path().join("new/nested");
        let id = f
            .create_in("f", &format!(" {} ", new_dir.display()))
            .unwrap();
        assert!(new_dir.is_dir());
        assert_eq!(
            f.board().get(id).unwrap().project,
            fs::canonicalize(&new_dir).unwrap()
        );

        let other = f.dir.path().join("other");
        f.run(Command::Edit {
            id: a,
            title: Some("renamed".into()),
            project: Some(other.display().to_string()),
            description: Some("d".into()),
        })
        .unwrap();
        let board = f.board();
        let task = board.get(a).unwrap();
        assert_eq!(
            (
                task.title.as_str(),
                &task.project,
                task.description.as_str()
            ),
            ("renamed", &fs::canonicalize(&other).unwrap(), "d")
        );

        // Fields left as None are kept.
        f.run(Command::Edit {
            id: a,
            title: None,
            project: None,
            description: Some("e".into()),
        })
        .unwrap();
        assert_eq!(f.board().get(a).unwrap().title, "renamed");
    }

    #[test]
    fn a_failed_command_removes_the_project_dir_it_created() {
        let f = fixture();
        let not_epic = f.create("not an epic");
        let tasks_json = fs::read(f.store.tasks_path()).unwrap();
        let top = f.dir.path().join("made");
        let new_dir = top.join("a/b");
        let typed = new_dir.display().to_string();

        // The board refuses the task only after the directory was made.
        let mut bad_parent = create("child", &typed);
        if let Command::Create { parent, .. } = &mut bad_parent {
            *parent = Some(not_epic);
        }
        let bad_edit = Command::Edit {
            id: Ulid::generate(),
            title: None,
            project: Some(typed.clone()),
            description: None,
        };
        for cmd in [bad_parent, bad_edit] {
            let desc = format!("{cmd:?}");
            assert!(f.run(cmd).is_err(), "{desc}");
            assert!(!top.exists(), "{desc}");
            assert_eq!(fs::read(f.store.tasks_path()).unwrap(), tasks_json);
        }

        // A directory that was already there is kept.
        fs::create_dir_all(&top).unwrap();
        let mut bad_parent = create("child", &typed);
        if let Command::Create { parent, .. } = &mut bad_parent {
            *parent = Some(not_epic);
        }
        assert!(f.run(bad_parent).is_err());
        assert!(top.is_dir() && !top.join("a").exists());
    }

    #[test]
    fn move_delete_and_epic_commands_apply() {
        let f = fixture();
        let a = f.create("a");
        let b = f.create("b");
        f.run(Command::Move {
            id: a,
            status: Status::Closed,
            position: None,
        })
        .unwrap();
        let board = f.board();
        assert_eq!(board.get(a).unwrap().status, Status::Closed);
        assert!(board.get(a).unwrap().closed.is_some());

        f.run(Command::Promote { id: a }).unwrap();
        assert!(f.board().get(a).unwrap().epic);
        f.run(Command::SetParent {
            id: b,
            parent: Some(a),
        })
        .unwrap();
        assert_eq!(f.board().get(b).unwrap().parent, Some(a));
        f.run(Command::SetParent {
            id: b,
            parent: None,
        })
        .unwrap();
        f.run(Command::Demote { id: a }).unwrap();
        assert!(!f.board().get(a).unwrap().epic);

        let att = file(
            &f.run(Command::Attach {
                task: b,
                source: bytes("x.txt"),
            })
            .unwrap(),
        );
        f.run(Command::Delete { id: b }).unwrap();
        assert!(f.board().get(b).is_none());
        assert!(!f.store.file_path(&att).exists());
    }

    #[test]
    fn link_commands_apply() {
        let f = fixture();
        let id = f.create("t");
        // Blank URLs are ignored, as in the GUI.
        f.run(Command::AddLink {
            task: id,
            url: "  ".into(),
            kind: LinkKind::Context,
        })
        .unwrap();
        assert!(f.board().get(id).unwrap().links.is_empty());
        for url in [" https://a ", "https://b"] {
            f.run(Command::AddLink {
                task: id,
                url: url.into(),
                kind: LinkKind::Source,
            })
            .unwrap();
        }
        f.run(Command::RemoveLink { task: id, index: 0 }).unwrap();
        let links = f.board().get(id).unwrap().links.clone();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (links[0].url.as_str(), links[0].kind),
            ("https://b", LinkKind::Source)
        );
    }

    #[test]
    fn file_commands_apply() {
        let f = fixture();
        let id = f.create("t");
        let src = f.dir.path().join("a.txt");
        fs::write(&src, "from disk").unwrap();
        let from_path = file(
            &f.run(Command::Attach {
                task: id,
                source: FileSource::Path { path: src },
            })
            .unwrap(),
        );
        let from_bytes = file(
            &f.run(Command::Attach {
                task: id,
                source: bytes("b.json"),
            })
            .unwrap(),
        );
        assert_eq!(from_bytes.kind, FileKind::Text);
        assert_eq!(
            fs::read_to_string(f.store.file_path(&from_path)).unwrap(),
            "from disk"
        );

        let new = file(
            &f.run(Command::NewFile {
                task: id,
                name: "plan".into(),
            })
            .unwrap(),
        );
        assert_eq!(new.name, "plan.md");
        f.run(Command::WriteFile {
            task: id,
            file: new.id,
            text: "# plan".into(),
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(f.store.file_path(&new)).unwrap(),
            "# plan"
        );

        f.run(Command::Detach {
            task: id,
            file: from_path.id,
        })
        .unwrap();
        assert!(!f.store.file_path(&from_path).exists());
        let ids: Vec<_> = f
            .board()
            .get(id)
            .unwrap()
            .files
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, [from_bytes.id, new.id]);
    }

    #[test]
    fn attach_bytes_takes_only_plain_file_names() {
        let f = fixture();
        let id = f.create("t");
        for bad in [
            "", " ", ".", "..", "a/b", "/a", "a/", "a\\b", "a:b", "a\nb", "a\0b", "a\u{7f}",
        ] {
            assert!(f.store.attach_bytes(id, bad, b"x").is_err(), "{bad:?}");
        }
        assert!(f.stored_files().is_empty());
        for good in ["a.txt", ".env", "with space.md", "ünï.txt"] {
            let att = f.store.attach_bytes(id, good, b"x").unwrap();
            assert_eq!(att.name, good);
        }
    }

    #[test]
    fn project_commands_apply() {
        let f = fixture();
        let a = f.create("a");
        let att = file(
            &f.run(Command::NewFile {
                task: a,
                name: "n".into(),
            })
            .unwrap(),
        );
        let moved = f.dir.path().join("moved");
        fs::create_dir(&moved).unwrap();
        let out = f
            .run(Command::RelocateProject {
                from: f.project.clone(),
                to: format!(" {} ", moved.display()),
            })
            .unwrap();
        let moved = project_path(&moved);
        assert_eq!(
            out.output,
            Output::Relocated {
                path: moved.clone()
            }
        );
        assert_eq!(f.board().get(a).unwrap().project, moved);

        let out = f.run(Command::DeleteProject { path: moved }).unwrap();
        assert_eq!(out.output, Output::ProjectDeleted { tasks: 1 });
        assert!(f.board().tasks.is_empty());
        assert!(!f.store.file_path(&att).exists());
    }

    #[test]
    fn events_match_the_gui_and_start_hooks() {
        let mut f = fixture();
        let log = f.dir.path().join("log");
        f.settings.hooks = [HookEvent::Created, HookEvent::Moved]
            .into_iter()
            .map(|event| Hook {
                event,
                command: format!("sh -c 'echo $0 $1 >> {}' {event:?}", log.display()),
                pass_task_id: true,
            })
            .collect();
        let event = |event, task| TaskEvent {
            event,
            task,
            project: f.project.clone(),
        };

        let out = f
            .run(create("a", &f.project.display().to_string()))
            .unwrap();
        let Output::Created { id: a } = out.output else {
            panic!("{out:?}");
        };
        assert_eq!(out.events, [event(HookEvent::Created, a)]);
        assert_eq!(out.message(), None);
        let b = f.create("b");
        // Reordering within a column isn't a move.
        let out = f
            .run(Command::Move {
                id: b,
                status: Status::Open,
                position: Some(1),
            })
            .unwrap();
        assert!(out.events.is_empty());
        let out = f
            .run(Command::Move {
                id: a,
                status: Status::Closed,
                position: None,
            })
            .unwrap();
        assert_eq!(out.events, [event(HookEvent::Moved, a)]);
        // Nothing else produces events.
        let out = f
            .run(Command::Edit {
                id: b,
                title: Some("bb".into()),
                project: None,
                description: None,
            })
            .unwrap();
        assert!(out.events.is_empty());

        // Hooks run concurrently, so compare the lines in any order.
        let mut want = [
            format!("Created {a}"),
            format!("Created {b}"),
            format!("Moved {a}"),
        ];
        let mut got = Vec::new();
        for _ in 0..200 {
            got = fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>();
            if got.len() >= want.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        got.sort();
        want.sort();
        assert_eq!(got, want);

        // A hook that can't start is reported, and the command still succeeds.
        f.settings.hooks = vec![Hook {
            event: HookEvent::Created,
            command: "/nonexistent/hook".into(),
            pass_task_id: false,
        }];
        let mut cmd = create("c", &f.project.display().to_string());
        if let Command::Create { files, .. } = &mut cmd {
            files.push(bytes("../bad"));
        }
        let out = f.run(cmd).unwrap();
        assert_eq!(out.hook_failures.len(), 1);
        let msg = out.message().unwrap();
        assert!(
            msg.starts_with("Hook `/nonexistent/hook` failed to start: ")
                && msg.contains("; Couldn't attach: ../bad ("),
            "{msg}"
        );
    }

    #[test]
    fn errors_leave_the_store_unchanged() {
        let mut f = fixture();
        let a = f.create("a");
        let epic = f.create("epic");
        f.run(Command::Promote { id: epic }).unwrap();
        f.run(Command::SetParent {
            id: a,
            parent: Some(epic),
        })
        .unwrap();
        let att = file(
            &f.run(Command::NewFile {
                task: a,
                name: "n".into(),
            })
            .unwrap(),
        );
        f.run(Command::WriteFile {
            task: a,
            file: att.id,
            text: "kept".into(),
        })
        .unwrap();

        let missing = Ulid::generate();
        let new_dir = f.dir.path().join("never");
        let typed_new = new_dir.display().to_string();
        let log = f.dir.path().join("log");
        f.settings.hooks = [HookEvent::Created, HookEvent::Moved]
            .map(|event| Hook {
                event,
                command: format!("sh -c 'echo ran >> {}'", log.display()),
                pass_task_id: false,
            })
            .to_vec();
        let tasks_json = fs::read(f.store.tasks_path()).unwrap();
        let files = f.stored_files();

        let mut blank_title = create(" ", &typed_new);
        if let Command::Create { files, .. } = &mut blank_title {
            files.push(bytes("x.txt"));
        }
        let mut child_of_task = create("child", &f.project.display().to_string());
        if let Command::Create { parent, .. } = &mut child_of_task {
            *parent = Some(a);
        }
        let failing = [
            blank_title,
            child_of_task,
            create("no project", " "),
            Command::Edit {
                id: a,
                title: Some("".into()),
                project: Some(typed_new.clone()),
                description: None,
            },
            Command::Edit {
                id: missing,
                title: Some("x".into()),
                project: None,
                description: None,
            },
            Command::Move {
                id: missing,
                status: Status::Closed,
                position: None,
            },
            Command::Delete { id: missing },
            Command::Promote { id: a },
            Command::Demote { id: epic },
            Command::SetParent {
                id: epic,
                parent: Some(a),
            },
            Command::AddLink {
                task: missing,
                url: "https://x".into(),
                kind: LinkKind::Context,
            },
            Command::RemoveLink { task: a, index: 0 },
            Command::Attach {
                task: missing,
                source: bytes("x.txt"),
            },
            Command::Attach {
                task: a,
                source: bytes("../x.txt"),
            },
            Command::Attach {
                task: a,
                source: FileSource::Path {
                    path: f.dir.path().join("gone.txt"),
                },
            },
            Command::Detach {
                task: a,
                file: missing,
            },
            Command::NewFile {
                task: missing,
                name: "n".into(),
            },
            Command::NewFile {
                task: a,
                name: "shot.png".into(),
            },
            Command::WriteFile {
                task: missing,
                file: att.id,
                text: "lost".into(),
            },
            Command::WriteFile {
                task: a,
                file: missing,
                text: "lost".into(),
            },
            Command::RelocateProject {
                from: f.project.clone(),
                to: " ".into(),
            },
            Command::RelocateProject {
                from: f.project.clone(),
                to: typed_new.clone(),
            },
            Command::DeleteProject {
                path: f.dir.path().join("unknown"),
            },
        ];
        for cmd in failing {
            let desc = format!("{cmd:?}");
            assert!(f.run(cmd).is_err(), "{desc}");
            assert_eq!(
                fs::read(f.store.tasks_path()).unwrap(),
                tasks_json,
                "{desc}"
            );
            assert_eq!(f.stored_files(), files, "{desc}");
        }
        assert!(!new_dir.exists());
        assert_eq!(fs::read_to_string(f.store.file_path(&att)).unwrap(), "kept");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!log.exists(), "no hooks run for failed commands");
    }

    #[test]
    fn only_local_file_sources_are_unsafe_remotely() {
        let id = Ulid::generate();
        let path = || FileSource::Path {
            path: "/etc/passwd".into(),
        };
        let mut with_path = create("t", "/p");
        if let Command::Create { files, .. } = &mut with_path {
            files.extend([bytes("a"), path()]);
        }
        assert!(!with_path.is_remote_safe());
        assert!(
            !Command::Attach {
                task: id,
                source: path()
            }
            .is_remote_safe()
        );
        assert!(
            Command::Attach {
                task: id,
                source: bytes("a")
            }
            .is_remote_safe()
        );
        assert!(create("t", "/p").is_remote_safe());
        assert!(Command::DeleteProject { path: "/p".into() }.is_remote_safe());
    }

    /// Checks `value` serializes to `golden` and back.
    fn golden<T>(value: T, golden: serde_json::Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(&value).unwrap(), golden);
        assert_eq!(serde_json::from_value::<T>(golden).unwrap(), value);
    }

    #[test]
    fn commands_have_a_stable_wire_shape() {
        let id: Ulid = "01M69ST9AGGGYR0TPT2Q3SH85N".parse().unwrap();
        let file: Ulid = "01M69ST9AGGGYR0TPT2Q3SH85P".parse().unwrap();
        let (i, f) = (id.to_string(), file.to_string());
        golden(
            Command::Create {
                title: "t".into(),
                project: "~/p".into(),
                description: "d".into(),
                status: Some(Status::InProgress),
                links: vec![Link {
                    url: "https://x".into(),
                    label: Some("x".into()),
                    kind: LinkKind::Source,
                }],
                files: vec![
                    FileSource::Path {
                        path: "/tmp/a.md".into(),
                    },
                    FileSource::Bytes {
                        name: "b.png".into(),
                        bytes: vec![0, 255, 1],
                    },
                ],
                position: Some(NewTaskPosition::Bottom),
                epic: true,
                parent: Some(file),
                id: Some(id),
            },
            json!({
                "type": "create", "title": "t", "project": "~/p", "description": "d",
                "status": "in_progress",
                "links": [{"url": "https://x", "label": "x", "kind": "source"}],
                "files": [
                    {"type": "path", "path": "/tmp/a.md"},
                    {"type": "bytes", "name": "b.png", "bytes": "AP8B"},
                ],
                "position": "bottom", "epic": true, "parent": f, "id": i,
            }),
        );
        golden(
            Command::Edit {
                id,
                title: Some("t".into()),
                project: None,
                description: Some(String::new()),
            },
            json!({"type": "edit", "id": i, "title": "t", "project": null, "description": ""}),
        );
        golden(
            Command::Move {
                id,
                status: Status::Closed,
                position: Some(2),
            },
            json!({"type": "move", "id": i, "status": "closed", "position": 2}),
        );
        golden(Command::Delete { id }, json!({"type": "delete", "id": i}));
        golden(Command::Promote { id }, json!({"type": "promote", "id": i}));
        golden(Command::Demote { id }, json!({"type": "demote", "id": i}));
        golden(
            Command::SetParent { id, parent: None },
            json!({"type": "set_parent", "id": i, "parent": null}),
        );
        golden(
            Command::AddLink {
                task: id,
                url: "u".into(),
                kind: LinkKind::Context,
            },
            json!({"type": "add_link", "task": i, "url": "u", "kind": "context"}),
        );
        golden(
            Command::RemoveLink { task: id, index: 3 },
            json!({"type": "remove_link", "task": i, "index": 3}),
        );
        golden(
            Command::Attach {
                task: id,
                source: FileSource::Bytes {
                    name: "a.txt".into(),
                    bytes: b"hi".to_vec(),
                },
            },
            json!({"type": "attach", "task": i,
                   "source": {"type": "bytes", "name": "a.txt", "bytes": "aGk="}}),
        );
        golden(
            Command::Detach { task: id, file },
            json!({"type": "detach", "task": i, "file": f}),
        );
        golden(
            Command::NewFile {
                task: id,
                name: "n".into(),
            },
            json!({"type": "new_file", "task": i, "name": "n"}),
        );
        golden(
            Command::WriteFile {
                task: id,
                file,
                text: "x".into(),
            },
            json!({"type": "write_file", "task": i, "file": f, "text": "x"}),
        );
        golden(
            Command::RelocateProject {
                from: "/a".into(),
                to: "~/b".into(),
            },
            json!({"type": "relocate_project", "from": "/a", "to": "~/b"}),
        );
        golden(
            Command::DeleteProject { path: "/a".into() },
            json!({"type": "delete_project", "path": "/a"}),
        );

        // Optional fields can be left out...
        let cmd: Command =
            serde_json::from_value(json!({"type": "create", "title": "t", "project": "/p"}))
                .unwrap();
        assert_eq!(cmd, create("t", "/p"));
        let cmd: Command = serde_json::from_value(json!({"type": "edit", "id": i})).unwrap();
        assert_eq!(
            cmd,
            Command::Edit {
                id,
                title: None,
                project: None,
                description: None
            }
        );
        // ...but unknown ones and bad base64 are refused.
        for bad in [
            json!({"type": "delete", "id": i, "force": true}),
            json!({"type": "create", "title": "t", "project": "/p", "postion": "top"}),
            json!({"type": "attach", "task": i,
                   "source": {"type": "bytes", "name": "a", "bytes": "aGk=", "x": 1}}),
            json!({"type": "attach", "task": i,
                   "source": {"type": "bytes", "name": "a", "bytes": "not base64!"}}),
            json!({"type": "nope"}),
        ] {
            assert!(
                serde_json::from_value::<Command>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn outcomes_have_a_stable_wire_shape() {
        let id: Ulid = "01M69ST9AGGGYR0TPT2Q3SH85N".parse().unwrap();
        let i = id.to_string();
        golden(Output::Done, json!({"type": "done"}));
        golden(Output::Created { id }, json!({"type": "created", "id": i}));
        golden(
            Output::File {
                file: Attachment {
                    id,
                    name: "a.md".into(),
                    kind: FileKind::Markdown,
                    path: format!("files/{id}.md"),
                },
            },
            json!({"type": "file", "file": {
                "id": i, "name": "a.md", "kind": "markdown", "path": format!("files/{i}.md"),
            }}),
        );
        golden(
            Output::Relocated { path: "/b".into() },
            json!({"type": "relocated", "path": "/b"}),
        );
        golden(
            Output::ProjectDeleted { tasks: 2 },
            json!({"type": "project_deleted", "tasks": 2}),
        );
        golden(
            Outcome {
                output: Output::Created { id },
                events: vec![TaskEvent {
                    event: HookEvent::Created,
                    task: id,
                    project: "/p".into(),
                }],
                hook_failures: vec!["h".into()],
                attach_failures: Vec::new(),
            },
            json!({
                "output": {"type": "created", "id": i},
                "events": [{"event": "created", "task": i, "project": "/p"}],
                "hook_failures": ["h"],
                "attach_failures": [],
            }),
        );
        // serde enforces deny_unknown_fields on struct variants only; the unit
        // variant `Done` ignores extra fields.
        assert!(
            serde_json::from_value::<Output>(json!({"type": "created", "id": i, "x": 1})).is_err()
        );
        assert!(serde_json::from_value::<Output>(json!({"type": "done", "x": 1})).is_ok());
    }
}
