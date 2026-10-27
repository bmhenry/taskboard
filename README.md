# Taskboard

Taskboard is a simple app for tracking tasks for a project.

Taskboard tasks are powerful:
- Each task contains plenty of data for developer & AI use
    - A ULID
    - a title
    - a description
    - "created", "last updated", and "closed" datetimes
    - a status
        - open: exists but not ready to work
        - backlog: ready to work
        - in progress
        - needs review
        - in review
        - closed
    - order (relative to the current status)
    - arbitrary URLs: clickable links which can be tagged as a "source" or as "context"
    - arbitrary files, which may be images, markdown, or plain text which is useful for the task

Tasks are stored in a `.tb/` folder local to each project for machine readability and storage, with the task itself being stored in JSON. Tasks should all be stored in a single JSON file, but can reference other files which are written or copied into the `.tb/` folder with ULIDs. The task JSON stores their original names, so they can be displayed as such in the UI instead of the ULID. The task JSON includes a schema so it can be migrated later if necessary. Per-project UI settings (such as which board columns are collapsed) are kept in `.tb/settings.json`; global settings (such as font size) are kept in `$XDG_CONFIG_HOME/tb/settings.json`. A project is designated as the root of the current git repo if Taskboard was started inside one, otherwise as the directory taskboard was started from.

Taskboard is actually two distinct programs:
- `taskboard`: The main application, which runs a local GUI
    - Provides a Kanban board as the main view, or you can switch to a list view mode of all tasks
    - Provides search to filter by name, description, or link text, as well as separate filtering for status and datetime
    - Provides prioritization of tasks by reordering them on the board
    - Enables Markdown formatting in the description of a task
    - Allows users to view other files (as markdown or plain text)
    - Watches files for change to update the tasks live (for example, when the CLI updates a task)
- `tb`: The CLI application, which enables adding tasks or retrieving task information
    - Defaults to human-readable output
    - Includes a `--json` flag for JSON output instead
    - Provides a CRUD interface for all task operations

The two programs share a common core which handles modifying the tasks on disk with atomic writes, so that each program can focus
just on its specific use case for UX. All changes are written to disk immediately on save.


Taskboard is built with Rust in a Cargo workspace, using [Ply](https://plyx.iz.rs/docs/getting-started/) for the GUI framework.


## Building

The workspace has three crates:
- `crates/taskboard-core`: task model, board operations, search filters, and the `.tb/` store
- `crates/tb`: the CLI
- `crates/taskboard`: the GUI

```sh
cargo build --release   # binaries land in target/release/{tb,taskboard}
cargo test --workspace
```

On Linux the GUI needs the usual X11/OpenGL development libraries (e.g. `libx11-dev libxi-dev libgl1-mesa-dev` on Debian/Ubuntu).


## Install

```sh
cargo install --path crates/tb
cargo install --path crates/taskboard
```


## Usage

Both programs find the project from the current directory (or a path you give them) and create `.tb/` on first use.

### GUI

```sh
taskboard            # current project
taskboard ~/src/app  # another project
```

- Click a card to open it; drag cards between and within columns to change status and priority
- `+` on a column (or `Ctrl+N`) creates a task; `Ctrl+S` saves, `Esc` cancels or closes
- `Ctrl+F` focuses search; status chips and the date range filter what's shown
- Add links and attach files (by path) from the task panel; click a file to view it

### CLI

Tasks are referred to by any unique suffix of their ID, such as the 8-character short ID shown in listings.

```sh
tb add "Fix login redirect" -s backlog --source https://tracker/issues/42 --file notes.md
echo "Longer *markdown* description" | tb add "Write docs" -d -
tb list                      # everything except closed tasks, grouped by status
tb list --all -q login       # search, including closed tasks
tb list --date updated --after 2026-01-01
tb show 7F3TPKKW
tb edit 7F3TPKKW -t "New title" -d -
tb move 7F3TPKKW in-progress -p 1   # 1 = top of the column
tb close 7F3TPKKW
tb rm 7F3TPKKW
tb link add 7F3TPKKW https://docs.rs -l "API docs" -k context
tb file add 7F3TPKKW screenshot.png
tb file cat 7F3TPKKW notes.md
```

Add `--json` to any command for machine-readable output (errors become `{"error": "..."}`).

Taskboard tasks are _not_ intended to be committed to `git` - they're for local work that the current developer is planning to work on themselves.


