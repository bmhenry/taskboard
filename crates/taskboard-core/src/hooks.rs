//! User hooks: commands run when a task is created or moved to another
//! status. Both the GUI and `tb` run them, after the change is saved, in the
//! task's project directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::model::{Board, Status};
use crate::{Error, Result};

/// What a hook reacts to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    #[default]
    Created,
    /// The task changed status (moved to another column). Reordering within a
    /// column doesn't count.
    Moved,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hook {
    pub event: HookEvent,
    /// A program and its arguments, split like a shell would (quotes and
    /// backslashes work) but run without one.
    pub command: String,
    /// Append the task's full ID to the command's arguments.
    pub pass_task_id: bool,
}

/// Something that happened to a task, for matching against hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEvent {
    pub event: HookEvent,
    pub task: Ulid,
    /// The task's project directory, where its hooks run.
    pub project: PathBuf,
}

/// Set in every hook's environment. Hooks don't run while it's set, so a
/// hook that changes tasks with `tb` can't set off itself or other hooks.
pub const IN_HOOK_ENV: &str = "TB_IN_HOOK";

/// The events that turned `before` into `after`.
pub fn events(before: &Board, after: &Board) -> Vec<TaskEvent> {
    let old: HashMap<Ulid, Status> = before.tasks.iter().map(|t| (t.id, t.status)).collect();
    after
        .tasks
        .iter()
        .filter_map(|t| {
            let event = match old.get(&t.id) {
                None => HookEvent::Created,
                Some(status) if *status != t.status => HookEvent::Moved,
                Some(_) => return None,
            };
            Some(TaskEvent {
                event,
                task: t.id,
                project: t.project.clone(),
            })
        })
        .collect()
}

/// Starts every hook matching each event, in the event's project directory,
/// without waiting for them to finish. Hooks with a blank command are skipped, and none run from
/// inside a hook (see [`IN_HOOK_ENV`]). Returns a message for each hook that
/// couldn't be started; the rest still run.
pub fn run(hooks: &[Hook], events: &[TaskEvent]) -> Vec<String> {
    let mut failures = Vec::new();
    if std::env::var_os(IN_HOOK_ENV).is_some() {
        return failures;
    }
    for ev in events {
        for hook in hooks.iter().filter(|h| h.event == ev.event) {
            if let Err(e) = spawn(hook, ev.task, &ev.project) {
                failures.push(format!(
                    "Hook `{}` failed to start: {e}",
                    hook.command.trim()
                ));
            }
        }
    }
    failures
}

fn spawn(hook: &Hook, task: Ulid, dir: &Path) -> Result<()> {
    let mut args = split_command(&hook.command)?;
    if args.is_empty() {
        return Ok(());
    }
    if hook.pass_task_id {
        args.push(task.to_string());
    }
    // stdout is dropped so a hook can't mix its output into `tb --json`.
    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .current_dir(dir)
        .env(IN_HOOK_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()?;
    // Reap it when it exits so long-running GUIs don't collect zombies.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Splits a command line into words the way a POSIX shell would, minus
/// expansions: whitespace separates words, single quotes keep everything
/// literal, and in double quotes or bare words a backslash escapes the next
/// character.
pub fn split_command(cmd: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => words.extend(word.take()),
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => w.push(c),
                        None => return Err(unclosed('\'')),
                    }
                }
            }
            '"' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('"') => break,
                        // Like a shell, only these are escapable inside "…".
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => w.push(c),
                            Some(c) => {
                                w.push('\\');
                                w.push(c);
                            }
                            None => return Err(unclosed('"')),
                        },
                        Some(c) => w.push(c),
                        None => return Err(unclosed('"')),
                    }
                }
            }
            '\\' => {
                if let Some(c) = chars.next() {
                    word.get_or_insert_with(String::new).push(c);
                }
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

fn unclosed(quote: char) -> Error {
    Error::Invalid(format!("unclosed {quote} in command"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::NewTask;
    use crate::model::Status;

    #[test]
    fn splits_like_a_shell() {
        let split = |s| split_command(s).unwrap();
        assert_eq!(split("  notify-send  hi "), ["notify-send", "hi"]);
        assert_eq!(
            split(r#"/bin/bash -c "echo \"\$1\" > out" x"#),
            ["/bin/bash", "-c", r#"echo "$1" > out"#, "x"]
        );
        assert_eq!(split(r"'it''s' a\ b"), ["its", "a b"]);
        assert_eq!(split(r#"say '' "\n""#), ["say", "", r"\n"]);
        assert!(split("").is_empty());
        assert!(split_command("echo 'oops").is_err());
        assert!(split_command("echo \"oops").is_err());
    }

    #[test]
    fn events_are_creates_and_status_changes() {
        let mut before = Board::default();
        let new = |title: &str, status| NewTask {
            title: title.into(),
            project: "/p".into(),
            status: Some(status),
            ..Default::default()
        };
        let a = before.create(new("a", Status::Backlog)).unwrap();
        let b = before.create(new("b", Status::Backlog)).unwrap();
        let mut after = before.clone();
        after.move_to(b, Status::Backlog, Some(0)).unwrap(); // reorder only
        after.move_to(a, Status::InProgress, None).unwrap();
        let c = after.create(new("c", Status::Open)).unwrap();

        assert_eq!(
            events(&before, &after),
            [
                TaskEvent {
                    event: HookEvent::Moved,
                    task: a,
                    project: "/p".into(),
                },
                TaskEvent {
                    event: HookEvent::Created,
                    task: c,
                    project: "/p".into(),
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn runs_matching_hooks_with_the_task_id() {
        let dir = tempfile::tempdir().unwrap();
        let task = Ulid::generate();
        let hooks = [
            Hook {
                event: HookEvent::Moved,
                command: r#"sh -c 'echo "$0" > moved'"#.into(),
                pass_task_id: true,
            },
            Hook {
                event: HookEvent::Created,
                command: "touch created".into(),
                pass_task_id: false,
            },
            Hook {
                event: HookEvent::Moved,
                command: "   ".into(),
                pass_task_id: true,
            },
            Hook {
                event: HookEvent::Moved,
                command: "/no/such/program".into(),
                pass_task_id: false,
            },
        ];
        let ev = TaskEvent {
            event: HookEvent::Moved,
            task,
            project: dir.path().to_path_buf(),
        };
        let failures = run(&hooks, &[ev]);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("/no/such/program"));

        let out = dir.path().join("moved");
        for _ in 0..200 {
            if std::fs::read_to_string(&out).is_ok_and(|s| s.ends_with('\n')) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim(),
            task.to_string()
        );
        assert!(!dir.path().join("created").exists());
    }
}
