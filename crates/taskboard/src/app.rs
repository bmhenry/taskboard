//! GUI state and the actions that change it. Kept free of rendering code so
//! it can be tested without a window.

use std::path::{Path, PathBuf};

use taskboard_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE, Theme};
use taskboard_core::{
    Board, DateField, Error, FileKind, Filter, GlobalSettings, Hook, HookEvent, Link, LinkKind,
    NewTask, Status, Store, Task, TaskEvent, TaskPatch, Ulid, create_project_dir, hooks,
    parse_date, project_path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Board,
    List,
    /// In-progress tasks in a sidebar, with the selected task's details.
    InProgress,
}

/// The task editor's target: an existing task, or a new one in `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    Existing(Ulid),
    New(Status),
}

/// Whose links and files an action changes: a saved task, or the [`Draft`]
/// of the new task being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Task(Ulid),
    Draft,
}

/// Links and files added to a new task before it's saved; they're saved with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub links: Vec<Link>,
    pub files: Vec<DraftFile>,
}

/// A file to attach once the new task is saved.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftFile {
    /// Identifies the file in the draft, like [`taskboard_core::Attachment::id`].
    pub id: Ulid,
    pub name: String,
    pub kind: FileKind,
    pub source: PathBuf,
}

impl Draft {
    /// Adds the file at `source`, checking it exists now rather than when the
    /// task is saved.
    fn attach(&mut self, source: &Path) -> taskboard_core::Result<()> {
        if !source.is_file() {
            return Err(Error::Invalid(format!("not a file: {}", source.display())));
        }
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Invalid(format!("bad file name: {}", source.display())))?
            .to_string();
        self.files.push(DraftFile {
            id: Ulid::generate(),
            kind: FileKind::from_name(&name),
            name,
            source: source.to_path_buf(),
        });
        Ok(())
    }
}

/// What the editor's project input names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectInput {
    Empty,
    /// A project already on the board, by name or directory.
    Known(PathBuf),
    /// A directory that exists but has no tasks yet.
    New(PathBuf),
    /// A directory that will be created when the task is saved.
    Missing(PathBuf),
}

/// Distance in pixels the pointer must travel before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Select(Option<Ulid>),
    SetView(View),
    /// Open the editor; the UI fills its inputs from the target.
    Edit(EditTarget),
    CancelEdit,
    Save {
        title: String,
        /// A project's name or directory; see [`App::project_input`].
        project: String,
        description: String,
    },
    Move {
        id: Ulid,
        status: Status,
        position: Option<usize>,
    },
    Delete(Ulid),
    AddLink {
        owner: Owner,
        url: String,
        kind: LinkKind,
    },
    RemoveLink {
        owner: Owner,
        index: usize,
    },
    Attach {
        owner: Owner,
        path: PathBuf,
    },
    Detach {
        owner: Owner,
        file: Ulid,
    },
    ViewFile(Option<(Ulid, Ulid)>),
    SetSearch(String),
    ToggleStatusFilter(Status),
    /// Points a project, and its tasks, at the directory typed or picked in
    /// `to`, as when the project was moved.
    RelocateProject {
        from: PathBuf,
        to: String,
    },
    /// Deletes a project and all its tasks.
    DeleteProject(PathBuf),
    /// Show only the project at this directory, or every project.
    SetProjectFilter(Option<PathBuf>),
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
    /// Links and files for the new task being written.
    pub draft: Draft,
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
    /// The project new tasks go in unless the user picks another: the one
    /// Taskboard was started in, if any.
    pub default_project: Option<PathBuf>,
    /// Projects whose directory wasn't there when last checked.
    pub missing_projects: Vec<PathBuf>,
}

impl App {
    /// Opens the board and loads settings from `settings_path`.
    pub fn new(store: Store, settings_path: Option<PathBuf>) -> App {
        let mut app = App {
            board: Board::default(),
            store,
            view: View::Board,
            filter: Filter::default(),
            selected: None,
            editing: None,
            draft: Draft::default(),
            viewing_file: None,
            date_inputs: (String::new(), String::new()),
            error: None,
            settings: GlobalSettings::default(),
            settings_path,
            settings_open: false,
            settings_dirty: false,
            default_project: None,
            missing_projects: Vec::new(),
        };
        if let Some(path) = &app.settings_path {
            match GlobalSettings::load(path) {
                Ok(settings) => app.settings = settings,
                Err(e) => app.error = Some(format!("Couldn't read settings: {e}")),
            }
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
                self.check_projects();
            }
            Err(e) => self.error = Some(format!("Couldn't read tasks: {e}")),
        }
    }

    /// Notes which projects' directories are gone, such as after being
    /// moved or deleted outside Taskboard.
    pub fn check_projects(&mut self) {
        self.missing_projects = self
            .board
            .projects
            .iter()
            .filter(|p| !p.path.is_dir())
            .map(|p| p.path.clone())
            .collect();
    }

    pub fn project_missing(&self, path: &Path) -> bool {
        self.missing_projects.iter().any(|p| p == path)
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
        self.settings
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

    /// In-progress tasks passing the filter, grouped under their project's
    /// name. Groups are sorted by name; tasks keep their board order.
    pub fn in_progress_groups(&self) -> Vec<(String, Vec<&Task>)> {
        let mut groups: Vec<(String, &Path, Vec<&Task>)> = Vec::new();
        for task in self.visible_column(Status::InProgress) {
            match groups.iter_mut().find(|g| g.1 == task.project) {
                Some(group) => group.2.push(task),
                None => groups.push((
                    self.board.project_name(&task.project),
                    &task.project,
                    vec![task],
                )),
            }
        }
        groups.sort_by(|a, b| (a.0.to_lowercase(), a.1).cmp(&(b.0.to_lowercase(), b.1)));
        groups
            .into_iter()
            .map(|(name, _, tasks)| (name, tasks))
            .collect()
    }

    /// Reads the editor's project input: a known project's name or
    /// directory, or the directory of a new one.
    pub fn project_input(&self, text: &str) -> ProjectInput {
        let text = text.trim();
        if text.is_empty() {
            return ProjectInput::Empty;
        }
        if let Some((p, _)) = self
            .board
            .project_names()
            .into_iter()
            .find(|(_, n)| n == text)
        {
            return ProjectInput::Known(p.path.clone());
        }
        let path = project_path(Path::new(text));
        if self.board.project(&path).is_some() {
            ProjectInput::Known(path)
        } else if path.is_dir() {
            ProjectInput::New(path)
        } else {
            ProjectInput::Missing(path)
        }
    }

    /// What the editor's project input starts as for a new task: the
    /// filtered project, or else the default one.
    pub fn new_task_project(&self) -> String {
        self.filter
            .project
            .as_ref()
            .or(self.default_project.as_ref())
            .map(|p| self.project_text(p))
            .unwrap_or_default()
    }

    /// How the project input names the project at `path`: by its name if
    /// it's on the board, else by path.
    pub fn project_text(&self, path: &Path) -> String {
        if self.board.project(path).is_some() {
            self.board.project_name(path)
        } else {
            path.display().to_string()
        }
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
                | Action::RelocateProject { .. }
                | Action::DeleteProject(_)
                | Action::AddLink {
                    owner: Owner::Task(_),
                    ..
                }
                | Action::RemoveLink {
                    owner: Owner::Task(_),
                    ..
                }
                | Action::Attach {
                    owner: Owner::Task(_),
                    ..
                }
                | Action::Detach {
                    owner: Owner::Task(_),
                    ..
                }
        );
        match action {
            Action::Select(id) => {
                self.selected = id;
                if id.is_none() {
                    self.editing = None;
                }
            }
            Action::SetView(view) => {
                self.view = view;
                // Open on a listed task's details rather than an empty pane,
                // unless that would swap out the task being edited.
                if view == View::InProgress && self.editing.is_none() {
                    let listed: Vec<Ulid> = self
                        .in_progress_groups()
                        .iter()
                        .flat_map(|(_, tasks)| tasks.iter().map(|t| t.id))
                        .collect();
                    if !self.selected.is_some_and(|id| listed.contains(&id)) {
                        self.selected = listed.first().copied();
                    }
                }
            }
            Action::Edit(target) => {
                self.editing = Some(target);
                match target {
                    EditTarget::Existing(id) => self.selected = Some(id),
                    EditTarget::New(_) => self.draft = Draft::default(),
                }
            }
            Action::CancelEdit => {
                self.editing = None;
                self.draft = Draft::default();
            }
            Action::Save {
                title,
                project,
                description,
            } => {
                let Some(target) = self.editing else {
                    return Ok(());
                };
                // Checked before the project, so a bad title doesn't leave
                // behind a newly created directory.
                if title.trim().is_empty() {
                    return Err(Error::Invalid("title must not be empty".into()));
                }
                let project = match self.project_input(&project) {
                    ProjectInput::Empty => {
                        return Err(Error::Invalid("choose a project for the task".into()));
                    }
                    ProjectInput::Known(path) | ProjectInput::New(path) => path,
                    ProjectInput::Missing(path) => create_project_dir(&path)?,
                };
                match target {
                    EditTarget::Existing(id) => {
                        let patch = TaskPatch {
                            title: Some(title),
                            project: Some(project),
                            description: Some(description),
                            status: None,
                        };
                        self.store.update(|b| b.edit(id, patch))?;
                        self.selected = Some(id);
                    }
                    EditTarget::New(status) => {
                        let new = NewTask {
                            title,
                            project,
                            description,
                            status: Some(status),
                            links: self.draft.links.clone(),
                        };
                        // The draft is kept if creating fails, e.g. on an empty title.
                        let (id, events) = self.store.update_with_events(|b| b.create(new))?;
                        let draft = std::mem::take(&mut self.draft);
                        // Hooks run once the files are attached, so they see the whole task.
                        let failed: Vec<String> = draft
                            .files
                            .iter()
                            .filter_map(|f| {
                                let e = self.store.attach(id, &f.source).err()?;
                                Some(format!("{} ({e})", f.name))
                            })
                            .collect();
                        self.run_hooks(&events);
                        if !failed.is_empty() {
                            let msg = format!("Couldn't attach: {}", failed.join(", "));
                            self.error = Some(match self.error.take() {
                                Some(hooks) => format!("{hooks}; {msg}"),
                                None => msg,
                            });
                        }
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
            Action::AddLink { owner, url, kind } => {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    let link = Link {
                        url,
                        label: None,
                        kind,
                    };
                    match owner {
                        Owner::Task(id) => self.store.update(|b| b.add_link(id, link))?,
                        Owner::Draft => self.draft.links.push(link),
                    }
                }
            }
            Action::RemoveLink { owner, index } => match owner {
                Owner::Task(id) => {
                    self.store.update(|b| b.remove_link(id, index))?;
                }
                Owner::Draft => {
                    if index < self.draft.links.len() {
                        self.draft.links.remove(index);
                    }
                }
            },
            Action::Attach { owner, path } => {
                match owner {
                    Owner::Task(id) => {
                        self.store.attach(id, &path)?;
                    }
                    // A file dialog can close after the new task was saved
                    // or cancelled; don't slip its file into the next unsaved
                    // draft.
                    Owner::Draft if matches!(self.editing, Some(EditTarget::New(_))) => {
                        self.draft.attach(&path)?
                    }
                    Owner::Draft => {}
                }
            }
            Action::Detach { owner, file } => match owner {
                Owner::Task(id) => {
                    self.store.detach(id, file)?;
                    if self.viewing_file == Some((id, file)) {
                        self.viewing_file = None;
                    }
                }
                Owner::Draft => self.draft.files.retain(|f| f.id != file),
            },
            Action::RelocateProject { from, to } => {
                let to = to.trim();
                if to.is_empty() {
                    return Err(Error::Invalid("choose the project's new folder".into()));
                }
                let to = project_path(Path::new(to));
                if !to.is_dir() {
                    return Err(Error::Invalid(format!("no folder at {}", to.display())));
                }
                self.store
                    .update(|b| b.relocate_project(&from, to.clone()))?;
                for path in [&mut self.filter.project, &mut self.default_project]
                    .into_iter()
                    .flatten()
                {
                    if *path == from {
                        *path = to.clone();
                    }
                }
            }
            Action::DeleteProject(path) => {
                self.store.delete_project(&path)?;
                if self.filter.project.as_ref() == Some(&path) {
                    self.filter.project = None;
                }
                if self.default_project.as_ref() == Some(&path) {
                    self.default_project = None;
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
            Action::SetProjectFilter(project) => self.filter.project = project,
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
                self.settings.collapsed_columns.insert(status, !collapsed);
                self.save_settings()?;
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
        let failures = hooks::run(&self.settings.hooks, events);
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

    /// An app whose store and default project, `app`, are in a temporary directory.
    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Store::open(&dir.path().join("data")).unwrap(), None);
        app.default_project = Some(create_project_dir(&dir.path().join("app")).unwrap());
        (dir, app)
    }

    fn save(app: &mut App, title: &str) {
        app.apply(Action::Save {
            title: title.into(),
            project: app.new_task_project(),
            description: String::new(),
        });
    }

    fn add(app: &mut App, title: &str) -> Ulid {
        app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
        save(app, title);
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
        save(&mut app, "alpha 2");
        assert_eq!(app.editing, None);
        assert_eq!(app.selected_task().unwrap().title, "alpha 2");
    }

    #[test]
    fn new_task_saves_draft_links_and_files() {
        let (dir, mut app) = app();
        let src = dir.path().join("notes.md");
        let gone = dir.path().join("gone.txt");
        std::fs::write(&src, "# hi").unwrap();
        std::fs::write(&gone, "").unwrap();
        app.apply(Action::Edit(EditTarget::New(Status::Open)));
        app.apply(Action::AddLink {
            owner: Owner::Draft,
            url: "https://example.com/1".into(),
            kind: LinkKind::Source,
        });
        for path in [gone.clone(), src, "missing.md".into()] {
            app.apply(Action::Attach {
                owner: Owner::Draft,
                path,
            });
        }
        assert!(app.error.take().unwrap().contains("missing.md"));
        assert_eq!(app.draft.files.len(), 2);
        // A file removed before saving doesn't stop the others attaching.
        std::fs::remove_file(&gone).unwrap();

        // A failed save keeps the draft to try again.
        save(&mut app, " ");
        assert!(app.error.take().is_some());
        save(&mut app, "alpha");
        assert!(
            app.error
                .unwrap()
                .starts_with("Couldn't attach: gone.txt (")
        );
        assert_eq!(app.draft, Draft::default());
        let task = app.board.column(Status::Open)[0];
        assert_eq!(task.links[0].url, "https://example.com/1");
        assert_eq!(task.files.len(), 1);
        assert_eq!(task.files[0].name, "notes.md");
    }

    #[test]
    fn add_link_trims_url_and_ignores_blank() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        for url in ["  https://example.com ", "   "] {
            app.apply(Action::AddLink {
                owner: Owner::Task(a),
                url: url.into(),
                kind: LinkKind::Context,
            });
        }
        let task = app.board.get(a).unwrap();
        let urls: Vec<&str> = task.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://example.com"]);
    }

    #[test]
    fn file_picked_after_new_task_closed_is_dropped() {
        let (dir, mut app) = app();
        let src = dir.path().join("notes.md");
        std::fs::write(&src, "").unwrap();
        app.apply(Action::Edit(EditTarget::New(Status::Open)));
        app.apply(Action::CancelEdit);
        app.apply(Action::Attach {
            owner: Owner::Draft,
            path: src,
        });
        assert_eq!(app.error, None);
        assert_eq!(app.draft, Draft::default());
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
    fn project_is_picked_by_name_or_directory_and_created_if_missing() {
        let (dir, mut app) = app();
        let app_dir = app.default_project.clone().unwrap();
        // Until it has tasks, the default project is named by its path.
        assert_eq!(app.new_task_project(), app_dir.display().to_string());
        let a = add(&mut app, "alpha");
        assert_eq!(app.new_task_project(), "app");
        assert_eq!(
            app.project_input(" app "),
            ProjectInput::Known(app_dir.clone())
        );
        assert_eq!(
            app.project_input(app_dir.to_str().unwrap()),
            ProjectInput::Known(app_dir.clone())
        );
        assert_eq!(app.project_input(""), ProjectInput::Empty);

        // A project typed as a directory that doesn't exist yet is created on save.
        let fresh = dir.path().join("new/thing");
        let typed = fresh.to_str().unwrap();
        assert_eq!(
            app.project_input(typed),
            ProjectInput::Missing(fresh.clone())
        );
        app.apply(Action::Edit(EditTarget::Existing(a)));
        app.apply(Action::Save {
            title: "alpha".into(),
            project: typed.into(),
            description: String::new(),
        });
        assert_eq!(app.error, None);
        assert!(fresh.is_dir());
        let fresh = fresh.canonicalize().unwrap();
        assert_eq!(app.board.get(a).unwrap().project, fresh);
        assert_eq!(app.project_input("thing"), ProjectInput::Known(fresh));

        // Neither a blank project nor a blank title saves, and a blank title
        // doesn't create the directory.
        let other = dir.path().join("other");
        for (title, project) in [("beta", ""), (" ", other.to_str().unwrap())] {
            app.apply(Action::Edit(EditTarget::New(Status::Open)));
            app.apply(Action::Save {
                title: title.into(),
                project: project.into(),
                description: String::new(),
            });
            assert!(app.error.take().is_some());
        }
        assert!(!other.exists());
        assert!(app.board.column(Status::Open).is_empty());
    }

    #[test]
    fn missing_project_can_be_relocated_or_deleted() {
        let (dir, mut app) = app();
        let a = add(&mut app, "alpha");
        let old = app.default_project.clone().unwrap();
        app.apply(Action::SetProjectFilter(Some(old.clone())));
        std::fs::rename(&old, dir.path().join("moved")).unwrap();
        app.reload();
        assert!(app.project_missing(&old));

        // The new folder must exist.
        let to = dir.path().join("moved");
        for typed in ["", dir.path().join("nowhere").to_str().unwrap()] {
            app.apply(Action::RelocateProject {
                from: old.clone(),
                to: typed.into(),
            });
            assert!(app.error.take().is_some());
        }
        app.apply(Action::RelocateProject {
            from: old.clone(),
            to: to.to_str().unwrap().into(),
        });
        assert_eq!(app.error, None);
        let to = to.canonicalize().unwrap();
        assert_eq!(app.board.get(a).unwrap().project, to);
        assert!(app.missing_projects.is_empty());
        assert_eq!(app.filter.project.as_ref(), Some(&to));
        assert_eq!(app.default_project.as_ref(), Some(&to));

        app.apply(Action::Select(Some(a)));
        app.apply(Action::DeleteProject(to));
        assert_eq!(app.error, None);
        assert!(app.board.tasks.is_empty() && app.board.projects.is_empty());
        assert_eq!(app.selected, None);
        assert_eq!(app.filter.project, None);
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
        let settings = dir.path().join("settings.json");
        app.settings_path = Some(settings.clone());
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
        let reopened = App::new(app.store.clone(), Some(settings));
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
    fn in_progress_view_groups_by_project_and_selects_first() {
        let (dir, mut app) = app();
        assert!(app.in_progress_groups().is_empty());
        let a = add(&mut app, "alpha");
        // Zeta's project sorts last but comes first on the board.
        app.default_project = Some(create_project_dir(&dir.path().join("Zed")).unwrap());
        let z = add(&mut app, "zeta");
        app.default_project = Some(dir.path().join("app").canonicalize().unwrap());
        let b = add(&mut app, "beta");
        let c = add(&mut app, "gamma");
        for id in [z, b, c] {
            app.apply(Action::Move {
                id,
                status: Status::InProgress,
                position: None,
            });
        }
        assert_eq!(app.error, None);

        let groups: Vec<(String, Vec<Ulid>)> = app
            .in_progress_groups()
            .into_iter()
            .map(|(name, tasks)| (name, tasks.iter().map(|t| t.id).collect()))
            .collect();
        assert_eq!(
            groups,
            [("app".into(), vec![b, c]), ("Zed".into(), vec![z])]
        );

        // A selection outside the view gives way to its first task...
        app.apply(Action::Select(Some(a)));
        app.apply(Action::SetView(View::InProgress));
        assert_eq!(app.selected, Some(b));
        // ...but a listed one is kept...
        app.apply(Action::Select(Some(c)));
        app.apply(Action::SetView(View::Board));
        app.apply(Action::SetView(View::InProgress));
        assert_eq!(app.selected, Some(c));
        // ...as is the task being edited.
        app.apply(Action::SetView(View::Board));
        app.apply(Action::Edit(EditTarget::Existing(a)));
        app.apply(Action::SetView(View::InProgress));
        assert_eq!(app.selected, Some(a));
        app.apply(Action::CancelEdit);

        // The search filter applies here too, so a hidden task gives way.
        app.apply(Action::SetSearch("beta".into()));
        let groups = app.in_progress_groups();
        assert_eq!(groups.len(), 1);
        let ids: Vec<Ulid> = groups[0].1.iter().map(|t| t.id).collect();
        assert_eq!(ids, [b]);
        app.apply(Action::Select(Some(c)));
        app.apply(Action::SetView(View::InProgress));
        assert_eq!(app.selected, Some(b));
    }

    #[test]
    fn drop_index_counts_cards_above() {
        assert_eq!(drop_index(&[10.0, 50.0, 90.0], 60.0), 2);
        assert_eq!(drop_index(&[], 60.0), 0);
    }
}
