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
    - whether it's an epic, and the epic it's a child of, if any
    - optionally, the person or agent it's assigned to

Epics are promoted tasks for larger, longer running work. An epic has everything a task has, and stays on the board like one, with a status you set yourself and an order in its column; it can also have child tasks, which stay on the board as normal cards. Epics are one level deep: an epic can't have a parent, and only an epic can be one. A child is always in the same project as its epic, so moving an epic to another project takes its children along, and a child has to leave its epic to move on its own. Only an epic without children can be demoted. Closing an epic doesn't touch its children, and deleting one keeps them, without a parent.

A task or epic can be assigned to anyone by name. A child follows its epic's assignee, including later changes to it, until it's given its own, which can be nobody. Names are remembered once used, even after no task has them, in the `assignees` list at the top of `tasks.json`; delete one there to forget it, unless a task still has it.

Tasks for every project are stored together in `$XDG_DATA_HOME/tb/` (`~/.local/share/tb/` by default) in a single JSON file, `tasks.json`, which also lists the known projects. Each task names its project by absolute directory; a project's name is its directory's name, with parent directories added in front (`work/app`, `src/app`) when it's needed to tell same-named projects apart. Tasks can reference other files, which are copied into `$XDG_DATA_HOME/tb/files/` under ULIDs; the task JSON stores their original names, so they can be displayed as such in the UI instead of the ULID. The task JSON includes a schema version so it can be migrated later if necessary. Settings shared by the GUI and `tb` (where new tasks go, hooks, which board columns are collapsed) are kept in `$XDG_CONFIG_HOME/tb/settings.json` (`~/.config/tb/` by default); the GUI's font size and theme are per device, in `device.json` beside it.

The current project is the root of the git repo Taskboard was started inside, otherwise the directory it was started from.

Taskboard is actually two distinct programs:
- `taskboard`: The main application, which runs a local GUI
    - Provides a Kanban board as the main view, or you can switch to a list view mode of all tasks
    - Provides search to filter by name, description, or link text, as well as separate filtering for status and datetime
    - Provides prioritization of tasks by reordering them on the board
    - Enables Markdown formatting in the description of a task
    - Draws charts from ` ```chart ` blocks in Markdown (see [docs/charts.md](docs/charts.md))
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

The workspace's crates:
- `crates/taskboard-core`: task model, board operations, search filters, and the shared task store
- `crates/tb`: the CLI
- `crates/taskboard-ui`: the GUI's iced UI, as a library shared with the web client. It reaches the store, file dialogs and settings through a `Platform` its host provides.
- `crates/taskboard`: the desktop GUI app, with the desktop `Platform` (local filesystem, native dialogs, a `notify` watcher)
- `crates/taskboard-web`: the daemon for the web client: an HTTP API over the store (its routes and JSON types are in `taskboard_core::api`) with live updates over server-sent events, and the web client's files (see [Web UI](#web-ui)). `taskboard-web [--bind 127.0.0.1:7878] [--allow-host NAME[:PORT]]... [--static DIR] [DIR]`. Every request needs a token, kept in `$XDG_CONFIG_HOME/tb/web-token` (made on first run). Open the `http://…/?token=…` link the daemon prints on starting: it sets a cookie and then drops the token from the URL. Scripts can send `Authorization: Bearer <token>` instead. Requests must also name the daemon in their `Host` header (any of localhost's names when it's bound to loopback), which blocks DNS rebinding. Add `--allow-host` for a Tailscale name or an SSH tunnel on another port; bound to `0.0.0.0`, every name clients use needs one. Requests that change things are refused if their `Origin` is another site. Browsers send cookies to every port on a host, so other servers on the same host (say, on another localhost port) also receive the token cookie.
- `crates/taskboard-wasm`: the web client: the UI built for the browser with trunk, with the web `Platform` over `taskboard-web`'s API (see its `README.md`)

```sh
cargo build --release   # binaries land in target/release/{tb,taskboard}
cargo test --workspace
```

On Linux the GUI needs the usual X11/OpenGL development libraries (e.g. `libx11-dev libxi-dev libgl1-mesa-dev` on Debian/Ubuntu).


## Development

To run the web UI from a debug build (it needs `trunk` and the `wasm32-unknown-unknown` target):

```sh
(cd crates/taskboard-wasm && trunk build)
cargo run -p taskboard-web -- --static crates/taskboard-wasm/dist
```

Then open the `http://127.0.0.1:7878/?token=…` link the daemon prints.

To have trunk rebuild the client while working on it, run the daemon and `trunk serve` side by side:

```sh
cargo run -p taskboard-web -- --allow-host 127.0.0.1:8080
cd crates/taskboard-wasm && trunk serve
```

Open the daemon's link once for the sign-in cookie, then use `http://127.0.0.1:8080`. Trunk proxies `/api/` to the daemon on port 7878 (see `Trunk.toml`).

A dev daemon uses your real task store, the same one as `tb` and the desktop app. If the systemd service is running, it already holds port 7878, so stop it first.


## Install

```sh
cargo install --path crates/tb
cargo install --path crates/taskboard
```

### Web UI

`scripts/build-web.sh` builds the web client with trunk (it needs `trunk` and the `wasm32-unknown-unknown` target) and then `taskboard-web` with the client built in (the `embed` feature), and says where the binary is. A `taskboard-web` built without it, as by `cargo build`, serves a page saying to run the script, or a `dist/` given with `--static`.

To run it as a systemd user service:

```sh
scripts/build-web.sh
install -Dm755 target/release/taskboard-web ~/.cargo/bin/taskboard-web
cp contrib/taskboard-web.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now taskboard-web
journalctl --user -u taskboard-web   # the http://127.0.0.1:7878/?token=… sign-in link
```

Edit `ExecStart` in the copied unit for another binary path, address, default project or `--allow-host`. It listens on localhost only. From another machine, either tunnel it (`ssh -L 7878:127.0.0.1:7878 host`, then open the link there; a different local port needs `--allow-host localhost:PORT`), or with Tailscale bind it to the machine's Tailscale address (`--bind 100.x.y.z:7878 --allow-host host.tailnet.ts.net`) and open the link with that address. Hooks run with the service's environment; see the unit's comments for `PATH`.


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
- `+` on a column (or `Ctrl+N`) creates a task, at the top of the column (Settings → New tasks can make it the bottom, for `tb add` too); `Ctrl+S` saves, `Esc` cancels or closes
- `Ctrl+F` focuses search; status and project chips and the date range filter what's shown
- Epics have a colored edge and a badge with how many of their children are closed, and child cards name their epic
- The dropdown next to a task's status assigns it: pick a name, Unassigned, or the epic's assignee, or type a new name; the new-task editor has the same dropdown, and cards and list rows show the name
- In the task panel, Promote to epic makes a task an epic and Choose epic ▾ makes it a child of one in its project; an epic lists its children, which open on a click, and × takes a child out of its epic
- Add links and attach files (by path) from the task panel; each file gets a tab beside the task's Details tab, in every view (in the editor, clicking a file opens it in the viewer)
- New… in the task panel's Files creates an empty file in the store (a name without an extension gets `.md`) and opens it for editing
- Edit in the file viewer edits any attached text file as plain text; `Ctrl+S` saves, Cancel discards. With unsaved changes, `Esc` and clicking outside don't close the viewer

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
tb add "Rework auth" --epic  # create an epic, or...
tb promote 7F3TPKKW          # ...promote a task (tb demote undoes it, if it has no children)
tb add "Login page" --parent 7F3TPKKW   # a child, in its epic's project unless --project says otherwise
tb edit 8M2QKRTW --parent 7F3TPKKW      # make a task a child of an epic
tb edit 8M2QKRTW --no-parent            # take it out again
tb list --parent 7F3TPKKW    # an epic's children, whichever project you're in; closed ones need -a
tb add "Login page" --assignee Alice    # assign to a person or agent; children otherwise follow their epic
tb edit 8M2QKRTW --assignee Bob         # assign or reassign; --no-assignee for nobody, --inherit-assignee to follow the epic again
tb project ls                # projects, flagging any whose folder is missing
tb project mv ~/src/old ~/src/new   # after moving a project's folder
tb project rm ~/src/old      # delete a project and all its tasks
tb move 7F3TPKKW in-progress -p 1   # 1 = top of the column
tb close 7F3TPKKW
tb rm 7F3TPKKW
tb link add 7F3TPKKW https://docs.rs -l "API docs" -k context   # -k defaults to source until the task has one
tb file add 7F3TPKKW screenshot.png
tb file new 7F3TPKKW plan    # empty file owned by the store: plan.md
tb file cat 7F3TPKKW notes.md
```

Add `--json` to any command for machine-readable output (errors become `{"error": "..."}`).

Taskboard tasks are _not_ intended to be committed to `git` - they're for local work that the current developer is planning to work on themselves.


