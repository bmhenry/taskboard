---
name: taskboard
description: Manage the project's Taskboard tasks with the `tb` CLI — list, show, add, edit, move, prioritize, close, or delete tasks, and attach links or files to them. Use when the user mentions tasks, the taskboard, the backlog, "what's next", or asks to track, file, pick up, update, or close a task in this project.
---

# Taskboard (`tb`)

Taskboard keeps every project's tasks in one store, `$XDG_DATA_HOME/tb/`
(`~/.local/share/tb/`). Each task belongs to a project: a directory, normally the git
repo root (or the current directory outside a repo). `tb` is its CLI; the `taskboard`
GUI watches the same files, so changes made with `tb` show up live on the board.

Tasks are local planning notes for the current developer, kept outside the repo.

## Ground rules

- **Use `--json` whenever you need to read the output.** It works on every command,
  and errors come back as `{"error": "..."}` with a non-zero exit code. Use the
  human-readable output only when showing results straight to the user.
- **Refer to tasks by short ID** — any unique suffix of the ULID works; the
  8-character short ID shown in listings is the usual choice.
- Run `tb` from inside the project, or pass `--project DIR`. `add` files tasks under
  that project and `list` shows only its tasks (`list -A` shows every project's);
  commands that take a task ID find it in any project.
- If `tb` isn't on `PATH`, use `cargo run -q -p tb --` from this repo, or install it
  with `cargo install --path crates/tb`.
- Ask before `tb rm` — it deletes the task and its attached files permanently.
  Prefer `tb close` for finished work.

## Statuses

| Key            | Meaning                    |
|----------------|----------------------------|
| `open`         | exists but not ready to work (default for new tasks) |
| `backlog`      | ready to work              |
| `in-progress`  | being worked on            |
| `needs-review` | done, waiting for review   |
| `in-review`    | being reviewed             |
| `closed`       | finished                   |

Status arguments are case-insensitive and accept `-`, `_`, or spaces between words
(`in-progress`, `in_progress`, `"In Progress"`). JSON output uses the snake_case key.

Each status is a column with its own order: position 1 (`order: 0` in JSON) is the
top of the column, i.e. highest priority.

## Commands

```sh
# Read
tb --json list                       # the project's non-closed tasks, in board order
tb --json list -A                    # ...from every project
tb --json list -s backlog            # one status (-s is repeatable)
tb --json list --all -q login        # search title/description/links, incl. closed
tb --json list --date updated --after 2026-01-01 --before 2026-02-01
tb --json show 7F3TPKKW              # one task in full, including description

# Create
tb --json add "Fix login redirect" -s backlog \
  --source https://tracker/issues/42 --context https://docs.rs --file notes.md
tb --json add "Write docs" -d - <<'MD'
Longer *markdown* description.
MD

# Update
tb --json edit 7F3TPKKW -t "New title"          # -t title, -d description, -s status
tb --json edit 7F3TPKKW -d - < description.md   # "-" reads the description from stdin
tb --json edit 7F3TPKKW --to-project ~/src/app  # move to another project
tb --json move 7F3TPKKW in-progress             # change status (goes to the bottom)
tb --json move 7F3TPKKW in-progress -p 1        # ...and put it at the top
tb --json move 7F3TPKKW -p 1                    # reprioritize within its column
tb --json close 7F3TPKKW
tb --json rm 7F3TPKKW                           # permanent; ask first

# Projects
tb --json project ls                            # path, name, task count, "missing" if its folder is gone
tb --json project mv ~/src/old ~/src/new        # point a moved project's tasks at its new folder
tb --json project rm ~/src/old                  # deletes the project and ALL its tasks; ask first

# Links (kind is "source" = where the task came from, or "context" = supporting material)
# Without -k, a link is "source" until the task has a source link, then "context".
tb --json link add 7F3TPKKW https://example.com -l "Label" -k source
tb --json link rm 7F3TPKKW 2                    # by number, as listed by `tb show`

# Files (copied into the store; referenced by original name or file-id suffix)
tb --json file add 7F3TPKKW screenshot.png
tb file cat 7F3TPKKW notes.md                   # prints raw contents (no JSON wrapper)
tb --json file rm 7F3TPKKW notes.md
```

`edit -d` **replaces** the whole description. To append, read the current one with
`tb --json show`, then write back the combined text via `-d -`.

## Task JSON

```json
{
  "id": "01M5X724HRT6FPK984034NF3M2",
  "title": "Hide columns for empty statuses by default",
  "project": "/home/me/src/taskboard",
  "description": "Markdown text",
  "status": "backlog",
  "order": 0,
  "created": "2026-10-26T22:59:53Z",
  "updated": "2026-10-26T22:59:53Z",
  "closed": null,
  "links": [{ "url": "https://…", "label": null, "kind": "source" }],
  "files": [{ "id": "01M…", "name": "notes.md", "path": "files/01M….md", "kind": "markdown" }]
}
```

The short ID is the last 8 characters of `id`; `project` is the project's absolute
directory. `list --json` returns an array in
board order: grouped by status (open → closed), then by `order` within each status.

## Common workflows

- **What's next?** `tb --json list -s backlog` and take the first entry (fall back
  to `-s open` if the backlog is empty). Then `tb --json show <id>` and read any
  attached files with `tb file cat` for full context.
- **Start a task:** `tb move <id> in-progress -p 1`.
- **Finish a task:** move it to `needs-review` if the user will review it, or
  `tb close <id>` if it's done. Mention the short ID and title when reporting.
- **File a new task from a conversation:** `tb add "<title>" -d -` with a Markdown
  description; add `--source <url>` for the issue/thread it came from. Default status
  is `open`; use `-s backlog` only if it's ready to be worked on.
- **Attach generated docs** (plans, requirements, notes): write the file somewhere
  temporary, then `tb file add <id> <path>`. Attaching the same name again adds a
  second file — `tb file rm` the old one first to replace it.
