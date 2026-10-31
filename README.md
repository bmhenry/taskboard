# Taskboard

Taskboard is a simple app for tracking tasks across your projects.

Taskboard tasks are powerful:
- Each task contains plenty of data for developer & AI use
    - A ULID
    - a title
    - a project: the directory the task belongs to (usually a git repo)
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

Tasks for every project are stored together in `$XDG_DATA_HOME/tb/` (`~/.local/share/tb/` by default) in a single JSON file, `tasks.json`, which also lists the known projects. Each task names its project by absolute directory; a project's name is its directory's name, with parent directories added in front (`work/app`, `src/app`) when it's needed to tell same-named projects apart. Tasks can reference other files, which are copied into `$XDG_DATA_HOME/tb/files/` under ULIDs; the task JSON stores their original names, so they can be displayed as such in the UI instead of the ULID. The task JSON includes a schema version so it can be migrated later if necessary. Settings (font size, theme, hooks, which board columns are collapsed) are kept in `$XDG_CONFIG_HOME/tb/settings.json` (`~/.config/tb/` by default).

The current project is the root of the git repo Taskboard was started inside, otherwise the directory it was started from. Older versions kept tasks in a `.tb/` folder inside each project: when `tb` or `taskboard` starts in such a project, its tasks and files are moved into the shared store and the folder is renamed to `.tb.imported` (delete it once you're happy with the result).

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

## Hooks

Settings → Hooks lists commands to run when a task is created or moved to another column, whether by the GUI or by `tb`. Reordering within a column and editing a task's text don't trigger them. Each hook is a program and its arguments, split like a shell would but run without one (use `bash -c '…'` for pipes and redirects), started in the task's project directory without waiting for it to finish; its stdout is discarded. Tick "Pass the task ID" to append the task's full ULID as the last argument, e.g. `notify-send Moved 01M6197HP62B5BKZRPNK3SCY8K`. Hooks are global settings, so they run in every project.


Taskboard is built with Rust in a Cargo workspace, using [Iced](https://iced.rs) for the GUI framework.


## Building

The workspace has three crates:
- `crates/taskboard-core`: task model, board operations, search filters, and the shared task store
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

Both programs find the current project from the current directory (or a path you give them). The store is created on first use.

### GUI

The GUI shows every project's tasks; the current project is where new tasks go by default.

```sh
taskboard            # new tasks default to the current project
taskboard ~/src/app  # ...or to another one
```

- Click a card to open it; drag cards between and within columns to change status and priority
- Each task's project is picked in the editor: choose a known project from the ▾ list, click Browse… to pick a folder, or type a folder's path (it's created on save if it doesn't exist)
- The In Progress view groups tasks under their project's name
- The Filter menu narrows the board to one project
- If a project's folder disappears (moved or deleted), its tasks get a yellow ⚠; hover it for details. Open one of the tasks to point Taskboard at the project's new folder, or to delete the project and its tasks
- `+` on a column (or `Ctrl+N`) creates a task; `Ctrl+S` saves, `Esc` cancels or closes
- `Ctrl+F` focuses search; status and project chips and the date range filter what's shown
- Add links and attach files (by path) from the task panel; click a file to view it

### CLI

Tasks are referred to by any unique suffix of their ID, such as the 8-character short ID shown in listings; IDs work from any project. `add` and `list` use the current project, or the one given with `--project DIR`.

```sh
tb add "Fix login redirect" -s backlog --source https://tracker/issues/42 --file notes.md
echo "Longer *markdown* description" | tb add "Write docs" -d -
tb --project ~/src/app add "Bump deps"   # file a task under another project
tb list                      # the project's tasks except closed ones, grouped by status
tb list -A                   # every project's tasks
tb list --all -q login       # search, including closed tasks
tb list --date updated --after 2026-01-01
tb show 7F3TPKKW
tb edit 7F3TPKKW -t "New title" -d -
tb edit 7F3TPKKW --to-project ~/src/app
tb project ls                # projects, flagging any whose folder is missing
tb project mv ~/src/old ~/src/new   # after moving a project's folder
tb project rm ~/src/old      # delete a project and all its tasks
tb move 7F3TPKKW in-progress -p 1   # 1 = top of the column
tb close 7F3TPKKW
tb rm 7F3TPKKW
tb link add 7F3TPKKW https://docs.rs -l "API docs" -k context   # -k defaults to source until the task has one
tb file add 7F3TPKKW screenshot.png
tb file cat 7F3TPKKW notes.md
```

Add `--json` to any command for machine-readable output (errors become `{"error": "..."}`).

Taskboard tasks are _not_ intended to be committed to `git` - they're for local work that the current developer is planning to work on themselves.


