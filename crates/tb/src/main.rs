//! `tb`: add and query Taskboard tasks from the command line.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use taskboard_core::{
    Board, DateField, Error, Filter, GlobalSettings, Hook, Link, LinkKind, NewTask, Status, Store,
    Task, TaskEvent, TaskPatch, Ulid, create_project_dir, hooks, parse_date, project_path,
    project_root, short_id,
};

#[derive(Parser, Debug)]
#[command(
    name = "tb",
    version,
    about = "Manage Taskboard tasks for the current project",
    long_about = "Manage Taskboard tasks for the current project.\n\nTasks for every project are kept together in $XDG_DATA_HOME/tb (or ~/.local/share/tb). New tasks go in the current project, and `list` shows its tasks; other commands find a task by ID in any project."
)]
struct Cli {
    /// Print JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,

    /// Project directory (defaults to the current git repo root, or the current directory).
    /// Created if it doesn't exist when adding a task.
    #[arg(long, global = true, value_name = "DIR")]
    project: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a task.
    Add {
        title: String,
        /// Description (Markdown). Use "-" to read it from stdin.
        #[arg(short, long)]
        description: Option<String>,
        /// Initial status [default: open].
        #[arg(short, long)]
        status: Option<Status>,
        /// URL tagged as where the task came from (repeatable).
        #[arg(long, value_name = "URL")]
        source: Vec<String>,
        /// URL tagged as supporting context (repeatable).
        #[arg(long, value_name = "URL")]
        context: Vec<String>,
        /// File to copy in and attach (repeatable).
        #[arg(long, value_name = "PATH")]
        file: Vec<PathBuf>,
    },
    /// List the project's tasks, grouped by status. Closed tasks are hidden unless asked for.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one task in full.
    Show { id: String },
    /// Change a task's title, description, status, or project.
    Edit {
        id: String,
        #[arg(short, long)]
        title: Option<String>,
        /// New description (Markdown). Use "-" to read it from stdin.
        #[arg(short, long)]
        description: Option<String>,
        #[arg(short, long)]
        status: Option<Status>,
        /// Move the task to the project in DIR, creating it if needed.
        #[arg(long, value_name = "DIR")]
        to_project: Option<PathBuf>,
    },
    /// Move a task to a status and/or position (1 = top of the column).
    #[command(alias = "mv")]
    Move {
        id: String,
        status: Option<Status>,
        #[arg(short, long)]
        position: Option<usize>,
    },
    /// Close a task.
    Close { id: String },
    /// Delete a task and its attached files.
    #[command(alias = "rm")]
    Delete { id: String },
    /// Manage a task's links.
    #[command(subcommand)]
    Link(LinkCommand),
    /// Manage a task's attached files.
    #[command(subcommand)]
    File(FileCommand),
    /// List, relocate, or delete projects.
    #[command(subcommand)]
    Project(ProjectCommand),
}

#[derive(Subcommand, Debug)]
enum ProjectCommand {
    /// List projects, flagging any whose folder is missing.
    #[command(alias = "ls")]
    List,
    /// Point a project and its tasks at its new folder, after it moved.
    #[command(alias = "mv")]
    Move { from: PathBuf, to: PathBuf },
    /// Delete a project with all its tasks and their files.
    #[command(alias = "rm")]
    Delete { dir: PathBuf },
}

#[derive(Args, Debug, Default)]
struct ListArgs {
    /// Only these statuses (repeatable).
    #[arg(short, long)]
    status: Vec<Status>,
    /// Include closed tasks.
    #[arg(short, long)]
    all: bool,
    /// List tasks from every project, not just the current one.
    #[arg(short = 'A', long)]
    all_projects: bool,
    /// Search title, description, and links.
    #[arg(short, long)]
    query: Option<String>,
    /// Timestamp that --after/--before apply to: created, updated, or closed.
    #[arg(long, default_value = "created")]
    date: DateFieldArg,
    /// Only tasks on or after this date (YYYY-MM-DD or RFC 3339).
    #[arg(long)]
    after: Option<String>,
    /// Only tasks before this date (YYYY-MM-DD or RFC 3339).
    #[arg(long)]
    before: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct DateFieldArg(DateField);

impl std::str::FromStr for DateFieldArg {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(DateFieldArg)
    }
}

#[derive(Subcommand, Debug)]
enum LinkCommand {
    /// Add a link to a task.
    Add {
        id: String,
        url: String,
        #[arg(short, long)]
        label: Option<String>,
        /// source or context [default: source if the task has no source link yet, else context].
        #[arg(short, long)]
        kind: Option<LinkKind>,
    },
    /// Remove a link by its number (as shown by `tb show`).
    Rm { id: String, number: usize },
}

#[derive(Subcommand, Debug)]
enum FileCommand {
    /// Copy a file into the task store and attach it to a task.
    Add { id: String, path: PathBuf },
    /// Detach and delete a file, by name or file id suffix.
    Rm { id: String, file: String },
    /// Print an attached file's contents, by name or file id suffix.
    Cat { id: String, file: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let project = match &cli.project {
        Some(dir) => project_path(dir),
        None => project_path(&project_root(
            &std::env::current_dir().expect("current directory is accessible"),
        )),
    };
    let json = cli.json;
    // Only commands that create or move tasks can fire hooks.
    let fires_hooks = matches!(
        cli.command,
        Command::Add { .. } | Command::Edit { .. } | Command::Move { .. } | Command::Close { .. }
    );
    let settings = match GlobalSettings::default_path().filter(|_| fires_hooks) {
        Some(path) => GlobalSettings::load(&path).unwrap_or_else(|e| {
            eprintln!("tb: warning: hooks are off: {e}");
            GlobalSettings::default()
        }),
        None => GlobalSettings::default(),
    };
    let result = Store::open_default().and_then(|store| {
        // Bring over tasks kept in the project from before the store was shared.
        match store.import_legacy(&project) {
            Ok(0) => {}
            Ok(n) => eprintln!(
                "tb: moved {n} tasks from {} into {}",
                project.display(),
                store.dir().display()
            ),
            Err(e) => eprintln!(
                "tb: warning: couldn't import {}/.tb: {e}",
                project.display()
            ),
        }
        run(
            cli,
            &store,
            &project,
            &settings.hooks,
            &mut io::stdout().lock(),
        )
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({ "error": e.to_string() }));
            } else {
                eprintln!("tb: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Runs `cli` against `store`, with `project` as the current project's directory.
fn run(
    cli: Cli,
    store: &Store,
    project: &Path,
    hooks: &[Hook],
    out: &mut impl Write,
) -> Result<(), Error> {
    let json = cli.json;
    let fire = |events: Vec<TaskEvent>| {
        for failure in hooks::run(hooks, &events) {
            eprintln!("tb: warning: {failure}");
        }
    };
    match cli.command {
        Command::Add {
            title,
            description,
            status,
            source,
            context,
            file,
        } => {
            let links = source
                .into_iter()
                .map(|url| Link {
                    url,
                    label: None,
                    kind: LinkKind::Source,
                })
                .chain(context.into_iter().map(|url| Link {
                    url,
                    label: None,
                    kind: LinkKind::Context,
                }))
                .collect();
            let new = NewTask {
                title,
                project: create_project_dir(project)?,
                description: description.map(read_text).transpose()?.unwrap_or_default(),
                status,
                links,
            };
            let (id, events) = store.update_with_events(|b| b.create(new))?;
            // Hooks run once the files are attached, so they see the whole task.
            let attached = file
                .iter()
                .try_for_each(|path| store.attach(id, path).map(|_| ()));
            fire(events);
            attached?;
            let board = store.load()?;
            let task = board.get(id).expect("task was just created");
            if json {
                print_json(out, task)?;
            } else {
                writeln!(out, "Created {}  {}", task.short_id(), task.title)?;
            }
        }
        Command::List(args) => {
            let mut filter = list_filter(&args)?;
            if !args.all_projects {
                filter.project = Some(project.to_path_buf());
            }
            let board = store.load()?;
            let tasks: Vec<&Task> = Status::ALL
                .into_iter()
                .flat_map(|s| board.column(s))
                .filter(|t| filter.matches(t))
                .collect();
            if json {
                print_json(out, &tasks)?;
            } else {
                let board = args.all_projects.then_some(&board);
                write_list(out, &tasks, board)?;
            }
        }
        Command::Show { id } => {
            let board = store.load()?;
            let task = board.get(board.resolve(&id)?).expect("resolved id exists");
            if json {
                print_json(out, task)?;
            } else {
                write_task(out, task, &board.project_name(&task.project))?;
            }
        }
        Command::Edit {
            id,
            title,
            description,
            status,
            to_project,
        } => {
            let patch = TaskPatch {
                title,
                project: to_project.as_deref().map(create_project_dir).transpose()?,
                description: description.map(read_text).transpose()?,
                status,
            };
            let (id, events) = store.update_with_events(|b| {
                let id = b.resolve(&id)?;
                b.edit(id, patch).map(|_| id)
            })?;
            fire(events);
            report(out, json, store, id, "Updated")?;
        }
        Command::Move {
            id,
            status,
            position,
        } => {
            let (id, events) = store.update_with_events(|b| {
                let id = b.resolve(&id)?;
                let status = status.unwrap_or(b.get(id).expect("resolved id exists").status);
                b.move_to(id, status, position.map(|p| p.saturating_sub(1)))
                    .map(|_| id)
            })?;
            fire(events);
            report(out, json, store, id, "Moved")?;
        }
        Command::Close { id } => {
            let (id, events) = store.update_with_events(|b| {
                let id = b.resolve(&id)?;
                b.move_to(id, Status::Closed, None).map(|_| id)
            })?;
            fire(events);
            report(out, json, store, id, "Closed")?;
        }
        Command::Delete { id } => {
            let id = store.load()?.resolve(&id)?;
            store.delete(id)?;
            if json {
                print_json(out, &serde_json::json!({ "deleted": id }))?;
            } else {
                writeln!(out, "Deleted {}", short_id(&id))?;
            }
        }
        Command::Link(LinkCommand::Add {
            id,
            url,
            label,
            kind,
        }) => {
            let id = store.update(|b| {
                let id = b.resolve(&id)?;
                let kind = match kind {
                    Some(kind) => kind,
                    None => b
                        .get(id)
                        .ok_or_else(|| Error::NotFound(id.to_string()))?
                        .default_link_kind(),
                };
                b.add_link(id, Link { url, label, kind }).map(|_| id)
            })?;
            report(out, json, store, id, "Linked")?;
        }
        Command::Link(LinkCommand::Rm { id, number }) => {
            let id = store.update(|b| {
                let id = b.resolve(&id)?;
                let index = number
                    .checked_sub(1)
                    .ok_or_else(|| Error::NotFound(format!("link #{number}")))?;
                b.remove_link(id, index).map(|_| id)
            })?;
            report(out, json, store, id, "Unlinked")?;
        }
        Command::File(FileCommand::Add { id, path }) => {
            let id = store.load()?.resolve(&id)?;
            store.attach(id, &path)?;
            report(out, json, store, id, "Attached to")?;
        }
        Command::File(FileCommand::Rm { id, file }) => {
            let board = store.load()?;
            let id = board.resolve(&id)?;
            let file_id = find_file(&board, id, &file)?;
            store.detach(id, file_id)?;
            report(out, json, store, id, "Detached from")?;
        }
        Command::File(FileCommand::Cat { id, file }) => {
            let board = store.load()?;
            let id = board.resolve(&id)?;
            let file_id = find_file(&board, id, &file)?;
            let task = board.get(id).expect("resolved id exists");
            let att = task
                .files
                .iter()
                .find(|f| f.id == file_id)
                .expect("found above");
            let bytes = std::fs::read(store.file_path(att))?;
            out.write_all(&bytes)?;
        }
        Command::Project(ProjectCommand::List) => {
            let board = store.load()?;
            let projects: Vec<_> = board
                .project_names()
                .into_iter()
                .map(|(p, name)| {
                    let tasks = board.tasks.iter().filter(|t| t.project == p.path).count();
                    (p, name, tasks, !p.path.is_dir())
                })
                .collect();
            if json {
                let rows: Vec<_> = projects
                    .iter()
                    .map(|(p, name, tasks, missing)| {
                        serde_json::json!({
                            "path": p.path, "name": name, "tasks": tasks, "missing": missing,
                        })
                    })
                    .collect();
                print_json(out, &rows)?;
            } else if projects.is_empty() {
                writeln!(out, "No projects.")?;
            }
            for (p, name, tasks, missing) in projects.iter().filter(|_| !json) {
                let missing = if *missing { "  (folder missing)" } else { "" };
                writeln!(
                    out,
                    "{name}  {}  [{}]{missing}",
                    p.path.display(),
                    plural(*tasks, "task")
                )?;
            }
        }
        Command::Project(ProjectCommand::Move { from, to }) => {
            let from = project_path(&from);
            let to = project_path(&to);
            if !to.is_dir() {
                return Err(Error::Invalid(format!("no folder at {}", to.display())));
            }
            store.update(|b| b.relocate_project(&from, to.clone()))?;
            if json {
                print_json(out, &serde_json::json!({ "moved": from, "to": to }))?;
            } else {
                writeln!(out, "Moved {} to {}", from.display(), to.display())?;
            }
        }
        Command::Project(ProjectCommand::Delete { dir }) => {
            let dir = project_path(&dir);
            let tasks = store.delete_project(&dir)?;
            if json {
                print_json(out, &serde_json::json!({ "deleted": dir, "tasks": tasks }))?;
            } else {
                writeln!(
                    out,
                    "Deleted {} and {}",
                    dir.display(),
                    plural(tasks, "task")
                )?;
            }
        }
    }
    Ok(())
}

/// Turns "-" into stdin's contents; anything else is used as-is.
fn read_text(arg: String) -> Result<String, Error> {
    if arg == "-" {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(arg)
    }
}

fn list_filter(args: &ListArgs) -> Result<Filter, Error> {
    let statuses = if !args.status.is_empty() {
        args.status.clone()
    } else if args.all {
        Vec::new()
    } else {
        Status::ALL
            .into_iter()
            .filter(|s| *s != Status::Closed)
            .collect()
    };
    Ok(Filter {
        text: args.query.clone().unwrap_or_default(),
        statuses,
        project: None,
        date_field: args.date.0,
        after: args.after.as_deref().map(parse_date).transpose()?,
        before: args.before.as_deref().map(parse_date).transpose()?,
    })
}

/// Finds an attachment on a task by exact name or by file id suffix.
fn find_file(board: &Board, id: Ulid, query: &str) -> Result<Ulid, Error> {
    let task = board.get(id).expect("resolved id exists");
    let q = query.to_uppercase();
    let matches: Vec<Ulid> = task
        .files
        .iter()
        .filter(|f| f.name == query || f.id.to_string().ends_with(&q))
        .map(|f| f.id)
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(Error::NotFound(query.to_string())),
        _ => Err(Error::Ambiguous(query.to_string(), matches.len())),
    }
}

fn report(
    out: &mut impl Write,
    json: bool,
    store: &Store,
    id: Ulid,
    verb: &str,
) -> Result<(), Error> {
    let board = store.load()?;
    let task = board.get(id).expect("task exists after update");
    if json {
        print_json(out, task)
    } else {
        writeln!(
            out,
            "{verb} {}  {} [{}]",
            task.short_id(),
            task.title,
            task.status
        )?;
        Ok(())
    }
}

fn print_json<T: serde::Serialize + ?Sized>(out: &mut impl Write, value: &T) -> Result<(), Error> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

/// Writes `tasks` grouped by status, naming each task's project when given
/// the `board` they're from.
fn write_list(out: &mut impl Write, tasks: &[&Task], board: Option<&Board>) -> io::Result<()> {
    if tasks.is_empty() {
        return writeln!(out, "No tasks.");
    }
    let mut current = None;
    for task in tasks {
        if current != Some(task.status) {
            if current.is_some() {
                writeln!(out)?;
            }
            writeln!(out, "{}", task.status)?;
            current = Some(task.status);
        }
        let mut extras = Vec::new();
        if !task.links.is_empty() {
            extras.push(plural(task.links.len(), "link"));
        }
        if !task.files.is_empty() {
            extras.push(plural(task.files.len(), "file"));
        }
        let extras = if extras.is_empty() {
            String::new()
        } else {
            format!("  ({})", extras.join(", "))
        };
        let project = board.map_or_else(String::new, |b| {
            format!("[{}]  ", b.project_name(&task.project))
        });
        writeln!(
            out,
            "  {}  {project}{}{}",
            task.short_id(),
            task.title,
            extras
        )?;
    }
    Ok(())
}

fn write_task(out: &mut impl Write, task: &Task, project_name: &str) -> io::Result<()> {
    let fmt = |d: chrono::DateTime<chrono::Utc>| {
        d.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    };
    writeln!(out, "{}  {}", task.short_id(), task.title)?;
    writeln!(out, "id:       {}", task.id)?;
    writeln!(out, "project:  {project_name} ({})", task.project.display())?;
    writeln!(out, "status:   {}", task.status)?;
    writeln!(out, "created:  {}", fmt(task.created))?;
    writeln!(out, "updated:  {}", fmt(task.updated))?;
    if let Some(closed) = task.closed {
        writeln!(out, "closed:   {}", fmt(closed))?;
    }
    if !task.links.is_empty() {
        writeln!(out, "\nLinks:")?;
        for (i, link) in task.links.iter().enumerate() {
            match &link.label {
                Some(label) => {
                    writeln!(out, "  {}. [{}] {label} <{}>", i + 1, link.kind, link.url)?
                }
                None => writeln!(out, "  {}. [{}] {}", i + 1, link.kind, link.url)?,
            }
        }
    }
    if !task.files.is_empty() {
        writeln!(out, "\nFiles:")?;
        for f in &task.files {
            writeln!(out, "  {}  {}", short_id(&f.id), f.name)?;
        }
    }
    if !task.description.trim().is_empty() {
        writeln!(out, "\n{}", task.description.trim_end())?;
    }
    Ok(())
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `tb` with the project `project` under the store's directory.
    fn tb_in(store: &Store, project: &str, args: &[&str]) -> String {
        let cli = Cli::try_parse_from(std::iter::once("tb").chain(args.iter().copied())).unwrap();
        let mut out = Vec::new();
        let project = store.dir().join(project);
        run(cli, store, &project, &[], &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn tb(store: &Store, args: &[&str]) -> String {
        tb_in(store, "app", args)
    }

    #[test]
    fn add_move_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();

        let created: Task = serde_json::from_str(&tb(
            &store,
            &[
                "--json",
                "add",
                "Write docs",
                "-s",
                "backlog",
                "--source",
                "https://example.com/1",
            ],
        ))
        .unwrap();
        assert_eq!(created.links[0].kind, LinkKind::Source);

        let second: Task =
            serde_json::from_str(&tb(&store, &["--json", "add", "Ship it", "-s", "backlog"]))
                .unwrap();
        tb(&store, &["move", &second.short_id(), "-p", "1"]);

        let listed = tb(&store, &["list"]);
        assert!(listed.contains("Backlog"));
        assert!(listed.find("Ship it").unwrap() < listed.find("Write docs").unwrap());

        tb(&store, &["close", &created.short_id()]);
        assert!(!tb(&store, &["list"]).contains("Write docs"));
        assert!(tb(&store, &["list", "--all"]).contains("Write docs"));
    }

    #[test]
    fn list_shows_the_current_project_unless_asked_for_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        tb(&store, &["add", "Mine"]);
        let theirs: Task =
            serde_json::from_str(&tb_in(&store, "other", &["--json", "add", "Theirs"])).unwrap();
        // Adding created the project's directory.
        assert!(dir.path().join("other").is_dir());

        let listed = tb(&store, &["list"]);
        assert!(listed.contains("Mine") && !listed.contains("Theirs"));
        let all = tb(&store, &["list", "-A"]);
        assert!(all.contains("[other]  Theirs"));

        tb(
            &store,
            &[
                "edit",
                &theirs.short_id(),
                "--to-project",
                dir.path().join("app").to_str().unwrap(),
            ],
        );
        assert!(tb(&store, &["list"]).contains("Theirs"));
    }

    #[test]
    fn project_move_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        tb(&store, &["add", "Mine"]);
        let app = dir.path().join("app");
        let moved = dir.path().join("moved");
        std::fs::rename(&app, &moved).unwrap();
        assert!(tb(&store, &["project", "ls"]).contains("(folder missing)"));

        let (app, moved) = (app.to_str().unwrap(), moved.to_str().unwrap());
        tb(&store, &["project", "mv", app, moved]);
        assert!(!tb(&store, &["project", "ls"]).contains("missing"));
        assert!(tb(&store, &["project", "rm", moved]).contains("1 task"));
        assert_eq!(tb(&store, &["project", "ls"]), "No projects.\n");
    }

    #[test]
    fn link_add_defaults_kind_to_source_then_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let task: Task = serde_json::from_str(&tb(&store, &["--json", "add", "T"])).unwrap();
        let id = task.short_id();
        tb(&store, &["link", "add", &id, "https://a", "-k", "context"]);
        tb(&store, &["link", "add", &id, "https://b"]);
        tb(&store, &["link", "add", &id, "https://c"]);
        let task: Task = serde_json::from_str(&tb(&store, &["--json", "show", &id])).unwrap();
        let kinds: Vec<_> = task.links.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [LinkKind::Context, LinkKind::Source, LinkKind::Context]
        );
    }
}
