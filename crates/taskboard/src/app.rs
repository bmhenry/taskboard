//! GUI state and the actions that change it. Kept free of rendering code so
//! it can be tested without a window.

use std::path::PathBuf;

use taskboard_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE, Theme};
use taskboard_core::{
    Board, DateField, Filter, GlobalSettings, Hook, HookEvent, Link, LinkKind, NewTask,
    ProjectSettings, Status, Store, Task, TaskEvent, TaskPatch, Ulid, hooks, parse_date,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Board,
    List,
}

/// The task editor's target: an existing task, or a new one in `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    Existing(Ulid),
    New(Status),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub id: Ulid,
    pub start: (f32, f32),
    /// Becomes true once the pointer has moved far enough to count as a drag
    /// rather than a click.
    pub active: bool,
}

/// Distance in pixels the pointer must travel before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Select(Option<Ulid>),
    SetView(View),
    /// Open the editor; the UI fills its inputs on the next frame.
    Edit(EditTarget),
    /// The UI has copied the edit target into its inputs.
    EditorReady,
    CancelEdit,
    Save {
        title: String,
        description: String,
    },
    Move {
        id: Ulid,
        status: Status,
        position: Option<usize>,
    },
    Delete(Ulid),
    AddLink {
        id: Ulid,
        url: String,
        kind: LinkKind,
    },
    RemoveLink {
        id: Ulid,
        index: usize,
    },
    Attach {
        id: Ulid,
        path: String,
    },
    Detach {
        id: Ulid,
        file: Ulid,
    },
    ViewFile(Option<(Ulid, Ulid)>),
    SetSearch(String),
    ToggleStatusFilter(Status),
    SetDateField(DateField),
    SetDateBounds {
        after: String,
        before: String,
    },
    ClearFilters,
    OpenUrl(String),
    DismissError,
    ShowSettings(bool),
    SetFontSize(u16),
    SetCollapseEmptyColumns(bool),
    SetTheme(Theme),
    ToggleColumn(Status),
    AddHook,
    RemoveHook(usize),
    SetHookEvent(usize, HookEvent),
    /// Changes a hook's command in memory only, so typing doesn't write the
    /// settings file on every key; see [`Action::SaveSettings`].
    SetHookCommand(usize, String),
    SetHookPassTaskId(usize, bool),
    /// Saves settings changed in memory, if any.
    SaveSettings,
}

pub struct App {
    pub store: Store,
    pub board: Board,
    pub view: View,
    pub filter: Filter,
    pub selected: Option<Ulid>,
    pub editing: Option<EditTarget>,
    /// Set when the editor opens so the UI copies the task into its inputs once.
    pub editor_needs_init: bool,
    /// (task, file) shown in the file viewer.
    pub viewing_file: Option<(Ulid, Ulid)>,
    /// The raw date bound inputs, kept so invalid text can be shown and fixed.
    pub date_inputs: (String, String),
    pub error: Option<String>,
    pub settings: GlobalSettings,
    /// Where global settings are saved; `None` keeps changes in memory only.
    pub settings_path: Option<PathBuf>,
    pub settings_open: bool,
    /// Settings were changed without being saved.
    pub settings_dirty: bool,
    /// Settings for this project, saved in `.tb/settings.json`.
    pub project_settings: ProjectSettings,
}

impl App {
    /// Opens the board, loads global settings from `settings_path`, and loads
    /// the project's settings from its `.tb/`.
    pub fn new(store: Store, settings_path: Option<PathBuf>) -> App {
        let mut app = App {
            board: Board::default(),
            store,
            view: View::Board,
            filter: Filter::default(),
            selected: None,
            editing: None,
            editor_needs_init: false,
            viewing_file: None,
            date_inputs: (String::new(), String::new()),
            error: None,
            settings: GlobalSettings::default(),
            settings_path,
            settings_open: false,
            settings_dirty: false,
            project_settings: ProjectSettings::default(),
        };
        if let Some(path) = &app.settings_path {
            match GlobalSettings::load(path) {
                Ok(settings) => app.settings = settings,
                Err(e) => app.error = Some(format!("Couldn't read settings: {e}")),
            }
        }
        match ProjectSettings::load(&app.store.settings_path()) {
            Ok(settings) => app.project_settings = settings,
            Err(e) => app.error = Some(format!("Couldn't read project settings: {e}")),
        }
        app.reload();
        app
    }

    pub fn reload(&mut self) {
        match self.store.load() {
            Ok(board) => {
                self.board = board;
                // Drop references to tasks deleted elsewhere (e.g. by the CLI).
                if self.selected.is_some_and(|id| self.board.get(id).is_none()) {
                    self.selected = None;
                }
                if let Some(EditTarget::Existing(id)) = self.editing
                    && self.board.get(id).is_none()
                {
                    self.editing = None;
                }
                if self
                    .viewing_file
                    .is_some_and(|(t, _)| self.board.get(t).is_none())
                {
                    self.viewing_file = None;
                }
            }
            Err(e) => self.error = Some(format!("Couldn't read tasks: {e}")),
        }
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.selected.and_then(|id| self.board.get(id))
    }

    /// Tasks in a status column that pass the current filter, in board order.
    pub fn visible_column(&self, status: Status) -> Vec<&Task> {
        self.board
            .column(status)
            .into_iter()
            .filter(|t| self.filter.matches(t))
            .collect()
    }

    /// Whether a board column is drawn as a thin strip: as the user last set
    /// it, or by default when it has no tasks and the setting is on.
    pub fn column_collapsed(&self, status: Status) -> bool {
        self.project_settings
            .collapsed_columns
            .get(&status)
            .copied()
            .unwrap_or_else(|| {
                self.settings.collapse_empty_columns && self.board.column(status).is_empty()
            })
    }

    /// Every task passing the filter, ordered by status then board order.
    pub fn visible_list(&self) -> Vec<&Task> {
        Status::ALL
            .into_iter()
            .flat_map(|s| self.visible_column(s))
            .collect()
    }

    pub fn apply(&mut self, action: Action) {
        if let Err(e) = self.try_apply(action) {
            self.error = Some(e.to_string());
        }
    }

    fn try_apply(&mut self, action: Action) -> taskboard_core::Result<()> {
        let writes = matches!(
            action,
            Action::Save { .. }
                | Action::Move { .. }
                | Action::Delete(_)
                | Action::AddLink { .. }
                | Action::RemoveLink { .. }
                | Action::Attach { .. }
                | Action::Detach { .. }
        );
        match action {
            Action::Select(id) => {
                self.selected = id;
                if id.is_none() {
                    self.editing = None;
                }
            }
            Action::SetView(view) => self.view = view,
            Action::Edit(target) => {
                self.editing = Some(target);
                self.editor_needs_init = true;
                if let EditTarget::Existing(id) = target {
                    self.selected = Some(id);
                }
            }
            Action::EditorReady => self.editor_needs_init = false,
            Action::CancelEdit => self.editing = None,
            Action::Save { title, description } => {
                let Some(target) = self.editing else {
                    return Ok(());
                };
                match target {
                    EditTarget::Existing(id) => {
                        let patch = TaskPatch {
                            title: Some(title),
                            description: Some(description),
                            status: None,
                        };
                        self.store.update(|b| b.edit(id, patch))?;
                        self.selected = Some(id);
                    }
                    EditTarget::New(status) => {
                        let new = NewTask {
                            title,
                            description,
                            status: Some(status),
                            links: Vec::new(),
                        };
                        let (_, events) = self.store.update_with_events(|b| b.create(new))?;
                        self.run_hooks(&events);
                        // A new task closes the panel rather than selecting it.
                        self.selected = None;
                    }
                }
                self.editing = None;
            }
            Action::Move {
                id,
                status,
                position,
            } => {
                let ((), events) = self
                    .store
                    .update_with_events(|b| b.move_to(id, status, position))?;
                self.run_hooks(&events);
            }
            Action::Delete(id) => {
                self.store.delete(id)?;
                if self.selected == Some(id) {
                    self.selected = None;
                    self.editing = None;
                }
            }
            Action::AddLink { id, url, kind } => {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    self.store.update(|b| {
                        b.add_link(
                            id,
                            Link {
                                url,
                                label: None,
                                kind,
                            },
                        )
                    })?;
                }
            }
            Action::RemoveLink { id, index } => {
                self.store.update(|b| b.remove_link(id, index))?;
            }
            Action::Attach { id, path } => {
                let path = PathBuf::from(expand_home(path.trim()));
                self.store.attach(id, &path)?;
            }
            Action::Detach { id, file } => {
                self.store.detach(id, file)?;
                if self.viewing_file == Some((id, file)) {
                    self.viewing_file = None;
                }
            }
            Action::ViewFile(f) => self.viewing_file = f,
            Action::SetSearch(text) => self.filter.text = text,
            Action::ToggleStatusFilter(status) => {
                if let Some(i) = self.filter.statuses.iter().position(|s| *s == status) {
                    self.filter.statuses.remove(i);
                } else {
                    self.filter.statuses.push(status);
                }
            }
            Action::SetDateField(field) => self.filter.date_field = field,
            Action::SetDateBounds { after, before } => {
                // Unparseable bounds are ignored (and flagged in the UI) rather
                // than hiding every task while the user is still typing.
                self.filter.after = parse_date(&after).ok();
                self.filter.before = parse_date(&before).ok();
                self.date_inputs = (after, before);
            }
            Action::ClearFilters => {
                self.filter = Filter::default();
                self.date_inputs = (String::new(), String::new());
            }
            Action::OpenUrl(url) => open_url(&url)?,
            Action::DismissError => self.error = None,
            Action::ShowSettings(open) => {
                self.settings_open = open;
                if !open && self.settings_dirty {
                    self.save_settings()?;
                }
            }
            Action::SetFontSize(size) => {
                self.settings.font_size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                self.save_settings()?;
            }
            Action::SetCollapseEmptyColumns(collapse) => {
                self.settings.collapse_empty_columns = collapse;
                self.save_settings()?;
            }
            Action::SetTheme(theme) => {
                self.settings.theme = theme;
                self.save_settings()?;
            }
            Action::ToggleColumn(status) => {
                let collapsed = self.column_collapsed(status);
                self.project_settings
                    .collapsed_columns
                    .insert(status, !collapsed);
                self.project_settings.save(&self.store.settings_path())?;
            }
            Action::AddHook => {
                self.settings.hooks.push(Hook::default());
                self.save_settings()?;
            }
            Action::RemoveHook(i) => {
                if i < self.settings.hooks.len() {
                    self.settings.hooks.remove(i);
                    self.save_settings()?;
                }
            }
            Action::SetHookEvent(i, event) => {
                if let Some(hook) = self.settings.hooks.get_mut(i) {
                    hook.event = event;
                    self.save_settings()?;
                }
            }
            Action::SetHookCommand(i, command) => {
                if let Some(hook) = self.settings.hooks.get_mut(i) {
                    hook.command = command;
                    self.settings_dirty = true;
                }
            }
            Action::SaveSettings => {
                if self.settings_dirty {
                    self.save_settings()?;
                }
            }
            Action::SetHookPassTaskId(i, pass) => {
                if let Some(hook) = self.settings.hooks.get_mut(i) {
                    hook.pass_task_id = pass;
                    self.save_settings()?;
                }
            }
        }
        // Show our own write right away rather than waiting for the watcher.
        if writes {
            self.reload();
        }
        Ok(())
    }

    /// Starts the user's hooks for `events`, showing any that fail to start.
    fn run_hooks(&mut self, events: &[TaskEvent]) {
        let failures = hooks::run(&self.settings.hooks, events, self.store.project_dir());
        if !failures.is_empty() {
            self.error = Some(failures.join("; "));
        }
    }

    fn save_settings(&mut self) -> taskboard_core::Result<()> {
        if let Some(path) = &self.settings_path {
            self.settings.save(path)?;
        }
        self.settings_dirty = false;
        Ok(())
    }
}

/// Converts a drop at visible index `visible_index` (among the column's
/// *visible* tasks, excluding the dragged one) into a position in the full
/// column, so dropping into a filtered column lands next to the right task.
pub fn drop_position(
    board: &Board,
    status: Status,
    visible: &[Ulid],
    visible_index: usize,
    dragged: Ulid,
) -> usize {
    let full: Vec<Ulid> = board
        .column(status)
        .iter()
        .map(|t| t.id)
        .filter(|id| *id != dragged)
        .collect();
    let index_of = |id: &Ulid| full.iter().position(|f| f == id).unwrap_or(full.len());
    match visible.get(visible_index) {
        Some(next) => index_of(next),
        None => visible.last().map_or(full.len(), |last| index_of(last) + 1),
    }
}

/// Index at which a drop at height `y` lands, given the vertical midpoints of
/// the cards it could land between.
pub fn drop_index(card_midpoints: &[f32], y: f32) -> usize {
    card_midpoints.iter().filter(|mid| **mid < y).count()
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(Store::open(dir.path()).unwrap(), None);
        (dir, app)
    }

    fn add(app: &mut App, title: &str) -> Ulid {
        app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
        app.apply(Action::Save {
            title: title.into(),
            description: String::new(),
        });
        app.board.column(Status::Backlog).last().unwrap().id
    }

    #[test]
    fn create_and_drag_within_filtered_column() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        let _b = add(&mut app, "beta");
        let c = add(&mut app, "gamma alpha");
        assert_eq!(app.error, None);

        // With "alpha" searched, only a and c are visible. Dragging c to the
        // top of the visible list must put it above a in the real column.
        app.apply(Action::SetSearch("alpha".into()));
        let visible: Vec<Ulid> = app
            .visible_column(Status::Backlog)
            .iter()
            .map(|t| t.id)
            .filter(|id| *id != c)
            .collect();
        assert_eq!(visible, [a]);
        let pos = drop_position(&app.board, Status::Backlog, &visible, 0, c);
        app.apply(Action::Move {
            id: c,
            status: Status::Backlog,
            position: Some(pos),
        });

        let order: Vec<&str> = app
            .board
            .column(Status::Backlog)
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(order, ["gamma alpha", "alpha", "beta"]);
    }

    #[test]
    fn saving_new_task_closes_panel_but_edit_keeps_selection() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        assert_eq!(app.error, None);
        assert_eq!(app.editing, None);
        assert_eq!(app.selected, None);

        app.apply(Action::Edit(EditTarget::Existing(a)));
        app.apply(Action::Save {
            title: "alpha 2".into(),
            description: String::new(),
        });
        assert_eq!(app.editing, None);
        assert_eq!(app.selected_task().unwrap().title, "alpha 2");
    }

    #[test]
    fn selection_cleared_when_task_deleted_elsewhere() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        app.apply(Action::Select(Some(a)));
        app.store.delete(a).unwrap(); // as if `tb rm` ran
        app.reload();
        assert_eq!(app.selected, None);
    }

    #[test]
    fn font_size_is_clamped_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/tb/settings.json");
        let mut app = App::new(Store::open(dir.path()).unwrap(), Some(path.clone()));
        app.apply(Action::SetFontSize(MAX_FONT_SIZE + 10));
        assert_eq!(app.error, None);
        assert_eq!(app.settings.font_size, MAX_FONT_SIZE);
        assert_eq!(GlobalSettings::load(&path).unwrap(), app.settings);

        let reopened = App::new(Store::open(dir.path()).unwrap(), Some(path));
        assert_eq!(reopened.settings.font_size, MAX_FONT_SIZE);
    }

    #[test]
    fn theme_defaults_to_dark_and_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/tb/settings.json");
        let mut app = App::new(Store::open(dir.path()).unwrap(), Some(path.clone()));
        assert_eq!(app.settings.theme, Theme::Dark);
        app.apply(Action::SetTheme(Theme::Light));
        assert_eq!(app.error, None);

        let reopened = App::new(Store::open(dir.path()).unwrap(), Some(path));
        assert_eq!(reopened.settings.theme, Theme::Light);
    }

    #[test]
    fn empty_columns_collapsed_by_default() {
        let (dir, mut app) = app();
        let a = add(&mut app, "alpha");
        assert!(!app.column_collapsed(Status::Backlog));
        assert!(app.column_collapsed(Status::Closed));

        app.apply(Action::ToggleColumn(Status::Closed));
        assert!(!app.column_collapsed(Status::Closed));
        app.apply(Action::ToggleColumn(Status::Closed));
        assert!(app.column_collapsed(Status::Closed));

        // A column with tasks collapses too, and stays as set once emptied.
        app.apply(Action::ToggleColumn(Status::Backlog));
        assert!(app.column_collapsed(Status::Backlog));
        let reopened = App::new(Store::open(dir.path()).unwrap(), None);
        assert!(reopened.column_collapsed(Status::Backlog));
        app.apply(Action::Delete(a));
        app.apply(Action::ToggleColumn(Status::Backlog));
        assert!(!app.column_collapsed(Status::Backlog));

        app.apply(Action::SetCollapseEmptyColumns(false));
        assert!(!app.column_collapsed(Status::Open));
    }

    #[cfg(unix)]
    #[test]
    fn hooks_run_on_create_and_status_change_but_not_reorder() {
        let (dir, mut app) = app();
        let log = dir.path().join("log");
        let command = format!("sh -c 'echo $0 $1 >> {}'", log.display());
        for (i, event) in [HookEvent::Created, HookEvent::Moved]
            .into_iter()
            .enumerate()
        {
            app.apply(Action::AddHook);
            app.apply(Action::SetHookEvent(i, event));
            app.apply(Action::SetHookCommand(i, format!("{command} {event:?}")));
            app.apply(Action::SetHookPassTaskId(i, true));
        }
        let a = add(&mut app, "alpha");
        let b = add(&mut app, "beta");
        app.apply(Action::Move {
            id: b,
            status: Status::Backlog,
            position: Some(0),
        });
        app.apply(Action::Move {
            id: a,
            status: Status::Closed,
            position: None,
        });
        assert_eq!(app.error, None);

        // Hooks run concurrently, so compare the lines in any order.
        let mut want = [
            format!("Created {a}"),
            format!("Created {b}"),
            format!("Moved {a}"),
        ];
        let mut got = Vec::new();
        for _ in 0..200 {
            got = std::fs::read_to_string(&log)
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

        // Typed commands are saved once editing is done.
        let path = dir.path().join("settings.json");
        app.settings_path = Some(path.clone());
        app.apply(Action::SetHookCommand(1, "true".into()));
        assert!(!path.exists());
        app.apply(Action::SaveSettings);
        assert_eq!(
            GlobalSettings::load(&path).unwrap().hooks[1].command,
            "true"
        );

        app.apply(Action::RemoveHook(0));
        assert_eq!(app.settings.hooks.len(), 1);
        assert_eq!(app.settings.hooks[0].event, HookEvent::Moved);
    }

    #[test]
    fn drop_index_counts_cards_above() {
        assert_eq!(drop_index(&[10.0, 50.0, 90.0], 60.0), 2);
        assert_eq!(drop_index(&[], 60.0), 0);
    }
}
