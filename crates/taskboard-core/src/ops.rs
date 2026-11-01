//! Every change the GUI makes to the store, as a serializable [`Command`],
//! and [`execute`], which applies one and runs the user's hooks. The desktop
//! app and the web daemon both run commands through here, so they behave the
//! same.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::board::{NewTask, TaskPatch};
use crate::hooks::{self, Hook, TaskEvent};
use crate::model::{Attachment, Link, LinkKind, Status};
use crate::settings::NewTaskPosition;
use crate::store::{Store, create_project_dir, project_path};
use crate::{Error, Result};

/// A change to the store.
///
/// Paths are on the machine that runs [`execute`]: for the web UI, the
/// daemon's, not the browser's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Adds a task with its links, then attaches its files. A file that
    /// can't be attached doesn't undo the task; see
    /// [`Outcome::attach_failures`].
    Create {
        title: String,
        /// The project's absolute directory.
        project: PathBuf,
        /// Create `project`'s directory first (the task's title is checked
        /// before, so a bad one leaves no directory behind).
        #[serde(default)]
        create_project_dir: bool,
        #[serde(default)]
        description: String,
        #[serde(default)]
        status: Option<Status>,
        #[serde(default)]
        links: Vec<Link>,
        #[serde(default)]
        files: Vec<FileSource>,
        #[serde(default)]
        position: NewTaskPosition,
        #[serde(default)]
        epic: bool,
        #[serde(default)]
        parent: Option<Ulid>,
    },
    /// Changes a task's title, project or description; `None` leaves one alone.
    Edit {
        id: Ulid,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        project: Option<PathBuf>,
        /// Create `project`'s directory first, as for [`Command::Create`].
        #[serde(default)]
        create_project_dir: bool,
        #[serde(default)]
        description: Option<String>,
    },
    /// Moves a task to `position` in `status`'s column (its end if `None`).
    Move {
        id: Ulid,
        status: Status,
        #[serde(default)]
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
    /// (`~/` is expanded), which must exist.
    RelocateProject {
        from: PathBuf,
        to: String,
    },
    /// Deletes a project with all its tasks and their files.
    DeleteProject {
        path: PathBuf,
    },
}

/// Where a file to attach comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSource {
    /// A file on the machine running [`execute`], copied into the store.
    Path(PathBuf),
    /// A file's contents, e.g. a browser upload. `name` must be a plain file
    /// name, not a path.
    Bytes { name: String, bytes: Vec<u8> },
}

impl FileSource {
    /// The name the file is shown by.
    pub fn name(&self) -> String {
        match self {
            FileSource::Path(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            FileSource::Bytes { name, .. } => name.clone(),
        }
    }

    fn attach(&self, store: &Store, task: Ulid) -> Result<Attachment> {
        match self {
            FileSource::Path(path) => store.attach(task, path),
            FileSource::Bytes { name, bytes } => store.attach_bytes(task, name, bytes),
        }
    }
}

/// What a command produced, beyond the change to the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Output {
    Done,
    /// The new task's ID, from [`Command::Create`].
    Created(Ulid),
    /// The file added by [`Command::Attach`] or [`Command::NewFile`].
    File(Attachment),
    /// The project's new directory, from [`Command::RelocateProject`].
    Relocated(PathBuf),
    /// How many tasks [`Command::DeleteProject`] deleted.
    ProjectDeleted(usize),
}

/// The result of a command that succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// Applies `command` to the store, then starts the `hooks` matching what it
/// did. If it fails, the board is left as it was and no hooks run.
pub fn execute(store: &Store, hooks: &[Hook], command: Command) -> Result<Outcome> {
    let mut attach_failures = Vec::new();
    let (output, events) = match command {
        Command::Create {
            title,
            project,
            create_project_dir: create_dir,
            description,
            status,
            links,
            files,
            position,
            epic,
            parent,
        } => {
            // Checked before the project, so a bad title doesn't leave
            // behind a newly created directory.
            check_title(&title)?;
            let project = project_dir(project, create_dir)?;
            let new = NewTask {
                title,
                project,
                description,
                status,
                links,
                position,
                epic,
                parent,
            };
            let (id, events) = store.update_with_events(|b| b.create(new))?;
            // Hooks run once the files are attached, so they see the whole task.
            for file in &files {
                if let Err(e) = file.attach(store, id) {
                    attach_failures.push(format!("{} ({e})", file.name()));
                }
            }
            (Output::Created(id), events)
        }
        Command::Edit {
            id,
            title,
            project,
            create_project_dir: create_dir,
            description,
        } => {
            if let Some(title) = &title {
                check_title(title)?;
            }
            let project = project.map(|p| project_dir(p, create_dir)).transpose()?;
            let patch = TaskPatch {
                title,
                project,
                description,
                ..Default::default()
            };
            done(store.update_with_events(|b| b.edit(id, patch))?)
        }
        Command::Move {
            id,
            status,
            position,
        } => done(store.update_with_events(|b| b.move_to(id, status, position))?),
        Command::Delete { id } => {
            store.delete(id)?;
            (Output::Done, Vec::new())
        }
        Command::Promote { id } => done(store.update_with_events(|b| b.promote(id))?),
        Command::Demote { id } => done(store.update_with_events(|b| b.demote(id))?),
        Command::SetParent { id, parent } => {
            done(store.update_with_events(|b| b.set_parent(id, parent))?)
        }
        Command::AddLink { task, url, kind } => {
            let url = url.trim().to_string();
            if url.is_empty() {
                (Output::Done, Vec::new())
            } else {
                let link = Link {
                    url,
                    label: None,
                    kind,
                };
                done(store.update_with_events(|b| b.add_link(task, link))?)
            }
        }
        Command::RemoveLink { task, index } => {
            let (_, events) = store.update_with_events(|b| b.remove_link(task, index))?;
            (Output::Done, events)
        }
        Command::Attach { task, source } => (Output::File(source.attach(store, task)?), Vec::new()),
        Command::Detach { task, file } => {
            store.detach(task, file)?;
            (Output::Done, Vec::new())
        }
        Command::NewFile { task, name } => {
            (Output::File(store.create_file(task, &name)?), Vec::new())
        }
        Command::WriteFile { task, file, text } => {
            store.write_file(task, file, text.as_bytes())?;
            (Output::Done, Vec::new())
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
            let ((), events) =
                store.update_with_events(|b| b.relocate_project(&from, to.clone()))?;
            (Output::Relocated(to), events)
        }
        Command::DeleteProject { path } => {
            let deleted = store.delete_project(&path)?;
            (Output::ProjectDeleted(deleted), Vec::new())
        }
    };
    let hook_failures = hooks::run(hooks, &events);
    Ok(Outcome {
        output,
        events,
        hook_failures,
        attach_failures,
    })
}

fn done(((), events): ((), Vec<TaskEvent>)) -> (Output, Vec<TaskEvent>) {
    (Output::Done, events)
}

fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(Error::Invalid("title must not be empty".into()));
    }
    Ok(())
}

fn project_dir(project: PathBuf, create: bool) -> Result<PathBuf> {
    if create {
        create_project_dir(&project)
    } else {
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::hooks::HookEvent;
    use crate::model::{Board, FileKind};

    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        /// A project directory that exists.
        project: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let project = dir.path().join("proj");
        fs::create_dir(&project).unwrap();
        Fixture {
            dir,
            store,
            project,
        }
    }

    impl Fixture {
        fn run(&self, command: Command) -> Result<Outcome> {
            execute(&self.store, &[], command)
        }

        fn create(&self, title: &str) -> Ulid {
            match self.run(create(title, &self.project)).unwrap().output {
                Output::Created(id) => id,
                other => panic!("expected Created, got {other:?}"),
            }
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

    fn create(title: &str, project: &Path) -> Command {
        Command::Create {
            title: title.into(),
            project: project.to_path_buf(),
            create_project_dir: false,
            description: String::new(),
            status: None,
            links: Vec::new(),
            files: Vec::new(),
            position: NewTaskPosition::Top,
            epic: false,
            parent: None,
        }
    }

    fn file(out: &Outcome) -> Attachment {
        match &out.output {
            Output::File(att) => att.clone(),
            other => panic!("expected File, got {other:?}"),
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
                project: f.project.clone(),
                create_project_dir: false,
                description: "desc".into(),
                status: Some(Status::Backlog),
                links: vec![link.clone()],
                files: vec![
                    FileSource::Path(src),
                    FileSource::Bytes {
                        name: "shot.png".into(),
                        bytes: vec![1, 2, 3],
                    },
                    FileSource::Path(f.dir.path().join("gone.txt")),
                ],
                position: NewTaskPosition::Top,
                epic: false,
                parent: None,
            })
            .unwrap();
        let Output::Created(id) = out.output else {
            panic!("{out:?}");
        };
        assert_eq!(out.attach_failures.len(), 1);
        assert!(out.attach_failures[0].starts_with("gone.txt ("));

        let board = f.board();
        let task = board.get(id).unwrap();
        assert_eq!(task.title, "first");
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
    fn create_and_edit_can_create_the_project_dir() {
        let f = fixture();
        let new_dir = f.dir.path().join("new/proj");
        let mut cmd = create("t", &new_dir);
        if let Command::Create {
            create_project_dir, ..
        } = &mut cmd
        {
            *create_project_dir = true;
        }
        let id = match f.run(cmd).unwrap().output {
            Output::Created(id) => id,
            other => panic!("{other:?}"),
        };
        assert!(new_dir.is_dir());
        assert_eq!(f.board().get(id).unwrap().project, new_dir);

        let other = f.dir.path().join("other");
        f.run(Command::Edit {
            id,
            title: Some("renamed".into()),
            project: Some(other.clone()),
            create_project_dir: true,
            description: Some("d".into()),
        })
        .unwrap();
        let board = f.board();
        let task = board.get(id).unwrap();
        assert!(other.is_dir());
        assert_eq!(
            (
                task.title.as_str(),
                &task.project,
                task.description.as_str()
            ),
            ("renamed", &other, "d")
        );

        // Fields left as None are kept.
        f.run(Command::Edit {
            id,
            title: None,
            project: None,
            create_project_dir: false,
            description: Some("e".into()),
        })
        .unwrap();
        assert_eq!(f.board().get(id).unwrap().title, "renamed");
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
                source: FileSource::Bytes {
                    name: "x.txt".into(),
                    bytes: b"x".to_vec(),
                },
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
                source: FileSource::Path(src),
            })
            .unwrap(),
        );
        let from_bytes = file(
            &f.run(Command::Attach {
                task: id,
                source: FileSource::Bytes {
                    name: "b.json".into(),
                    bytes: b"{}".to_vec(),
                },
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
        assert_eq!(out.output, Output::Relocated(moved.clone()));
        assert_eq!(f.board().get(a).unwrap().project, moved);

        let out = f.run(Command::DeleteProject { path: moved }).unwrap();
        assert_eq!(out.output, Output::ProjectDeleted(1));
        assert!(f.board().tasks.is_empty());
        assert!(!f.store.file_path(&att).exists());
    }

    #[test]
    fn events_match_the_gui_and_start_hooks() {
        let f = fixture();
        let log = f.dir.path().join("log");
        let hooks: Vec<Hook> = [HookEvent::Created, HookEvent::Moved]
            .into_iter()
            .map(|event| Hook {
                event,
                command: format!("sh -c 'echo $0 $1 >> {}' {event:?}", log.display()),
                pass_task_id: true,
            })
            .collect();
        let run = |cmd| execute(&f.store, &hooks, cmd).unwrap();
        let event = |event, task| TaskEvent {
            event,
            task,
            project: f.project.clone(),
        };

        let out = run(create("a", &f.project));
        let Output::Created(a) = out.output else {
            panic!("{out:?}");
        };
        assert_eq!(out.events, [event(HookEvent::Created, a)]);
        assert!(out.hook_failures.is_empty());
        let b = match run(create("b", &f.project)).output {
            Output::Created(b) => b,
            other => panic!("{other:?}"),
        };
        // Reordering within a column isn't a move.
        let out = run(Command::Move {
            id: b,
            status: Status::Open,
            position: Some(1),
        });
        assert!(out.events.is_empty());
        let out = run(Command::Move {
            id: a,
            status: Status::Closed,
            position: None,
        });
        assert_eq!(out.events, [event(HookEvent::Moved, a)]);
        // Nothing else produces events.
        let out = run(Command::Edit {
            id: b,
            title: Some("bb".into()),
            project: None,
            create_project_dir: false,
            description: None,
        });
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
        let broken = [Hook {
            event: HookEvent::Created,
            command: "/nonexistent/hook".into(),
            pass_task_id: false,
        }];
        let out = execute(&f.store, &broken, create("c", &f.project)).unwrap();
        assert_eq!(out.hook_failures.len(), 1);
    }

    #[test]
    fn errors_leave_the_store_unchanged() {
        let f = fixture();
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
        let log = f.dir.path().join("log");
        let hooks = [HookEvent::Created, HookEvent::Moved].map(|event| Hook {
            event,
            command: format!("sh -c 'echo ran >> {}'", log.display()),
            pass_task_id: false,
        });
        let tasks_json = fs::read(f.store.tasks_path()).unwrap();
        let files = f.stored_files();

        let failing = [
            Command::Create {
                title: " ".into(),
                project: new_dir.clone(),
                create_project_dir: true,
                description: String::new(),
                status: None,
                links: Vec::new(),
                files: vec![FileSource::Bytes {
                    name: "x.txt".into(),
                    bytes: b"x".to_vec(),
                }],
                position: NewTaskPosition::Top,
                epic: false,
                parent: None,
            },
            Command::Create {
                title: "child of a non-epic".into(),
                project: f.project.clone(),
                create_project_dir: false,
                description: String::new(),
                status: None,
                links: Vec::new(),
                files: Vec::new(),
                position: NewTaskPosition::Top,
                epic: false,
                parent: Some(a),
            },
            Command::Edit {
                id: a,
                title: Some("".into()),
                project: Some(new_dir.clone()),
                create_project_dir: true,
                description: None,
            },
            Command::Edit {
                id: missing,
                title: Some("x".into()),
                project: None,
                create_project_dir: false,
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
                source: FileSource::Bytes {
                    name: "x.txt".into(),
                    bytes: b"x".to_vec(),
                },
            },
            Command::Attach {
                task: a,
                source: FileSource::Bytes {
                    name: "../x.txt".into(),
                    bytes: b"x".to_vec(),
                },
            },
            Command::Attach {
                task: a,
                source: FileSource::Path(f.dir.path().join("gone.txt")),
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
                to: new_dir.display().to_string(),
            },
            Command::DeleteProject {
                path: f.dir.path().join("unknown"),
            },
        ];
        for cmd in failing {
            let desc = format!("{cmd:?}");
            assert!(execute(&f.store, &hooks, cmd).is_err(), "{desc}");
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
    fn commands_round_trip_through_json() {
        let id = Ulid::generate();
        let file = Ulid::generate();
        let commands = [
            Command::Create {
                title: "t".into(),
                project: "/p".into(),
                create_project_dir: true,
                description: "d".into(),
                status: Some(Status::InProgress),
                links: vec![Link {
                    url: "https://x".into(),
                    label: Some("x".into()),
                    kind: LinkKind::Source,
                }],
                files: vec![
                    FileSource::Path("/tmp/a.md".into()),
                    FileSource::Bytes {
                        name: "b.png".into(),
                        bytes: vec![0, 255],
                    },
                ],
                position: NewTaskPosition::Bottom,
                epic: true,
                parent: Some(file),
            },
            Command::Edit {
                id,
                title: Some("t".into()),
                project: None,
                create_project_dir: false,
                description: Some(String::new()),
            },
            Command::Move {
                id,
                status: Status::Closed,
                position: Some(2),
            },
            Command::Delete { id },
            Command::Promote { id },
            Command::Demote { id },
            Command::SetParent { id, parent: None },
            Command::AddLink {
                task: id,
                url: "u".into(),
                kind: LinkKind::Source,
            },
            Command::RemoveLink { task: id, index: 3 },
            Command::Attach {
                task: id,
                source: FileSource::Path("/a".into()),
            },
            Command::Detach { task: id, file },
            Command::NewFile {
                task: id,
                name: "n".into(),
            },
            Command::WriteFile {
                task: id,
                file,
                text: "x".into(),
            },
            Command::RelocateProject {
                from: "/a".into(),
                to: "~/b".into(),
            },
            Command::DeleteProject { path: "/a".into() },
        ];
        for cmd in commands {
            let json = serde_json::to_string(&cmd).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, cmd, "{json}");
        }

        // Optional fields can be left out.
        let json = r#"{"op": "create", "title": "t", "project": "/p"}"#;
        let cmd: Command = serde_json::from_str(json).unwrap();
        assert_eq!(cmd, create("t", Path::new("/p")));

        let outcome = Outcome {
            output: Output::Created(id),
            events: vec![TaskEvent {
                event: crate::HookEvent::Created,
                task: id,
                project: "/p".into(),
            }],
            hook_failures: vec!["h".into()],
            attach_failures: Vec::new(),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(serde_json::from_str::<Outcome>(&json).unwrap(), outcome);
    }
}
