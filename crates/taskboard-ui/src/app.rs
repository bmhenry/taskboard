//! GUI state and the actions that change it. Kept free of rendering code so
//! it can be tested without a window.
//!
//! Actions that change the store are optimistic: [`App::apply`] makes the
//! change to its own [`Board`] at once, and leaves a [`Command`] in its
//! outbox for the [`crate::Platform`] to run; see [`Effect`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use taskboard_core::ops::{Command, FileSource, Outcome, Output};
use taskboard_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE, NewTaskPosition, Theme};
use taskboard_core::{
    Board, DateField, Error, FileKind, Filter, GlobalSettings, Hook, HookEvent, Link, LinkKind,
    NewTask, Status, Task, TaskPatch, Ulid, parse_date, project_path,
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
    pub source: FileSource,
}

impl Draft {
    /// Adds the file from `source`, checking a path names a file now rather
    /// than when the task is saved.
    fn attach(&mut self, source: FileSource) -> taskboard_core::Result<()> {
        let name = match &source {
            FileSource::Path { path } => {
                // TODO(Platform): asks `Platform::path_kind` instead.
                if !path.is_file() {
                    return Err(Error::Invalid(format!("not a file: {}", path.display())));
                }
                path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| Error::Invalid(format!("bad file name: {}", path.display())))?
                    .to_string()
            }
            FileSource::Bytes { name, .. } => name.clone(),
        };
        self.files.push(DraftFile {
            id: Ulid::generate(),
            kind: FileKind::from_name(&name),
            name,
            source,
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
    /// Makes a task an epic.
    Promote(Ulid),
    /// Makes an epic a normal task.
    Demote(Ulid),
    /// Makes a task a child of the epic `parent`, or of none.
    SetParent {
        id: Ulid,
        parent: Option<Ulid>,
    },
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
        source: FileSource,
    },
    Detach {
        owner: Owner,
        file: Ulid,
    },
    /// Creates an empty file in the store, attaches it to `task` and opens it
    /// in the viewer; see [`taskboard_core::new_file_name`] for the name.
    NewFile {
        task: Ulid,
        name: String,
    },
    /// Replaces the contents of one of `task`'s files.
    WriteFile {
        task: Ulid,
        file: Ulid,
        text: String,
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
    DismissError,
    ShowSettings(bool),
    SetFontSize(u16),
    SetCollapseEmptyColumns(bool),
    SetTheme(Theme),
    SetNewTaskPosition(NewTaskPosition),
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

/// Work [`App::apply`] leaves for the [`crate::Platform`], to be done one
/// at a time, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A change already made to [`App::board`], to make in the store too.
    /// Its result goes to [`App::finished`], and then the board is reloaded.
    Run(Command),
    /// Its result goes to [`App::settings_saved`].
    SaveSettings(GlobalSettings),
}

pub struct App {
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
    pub settings_open: bool,
    /// Settings were changed without being saved.
    pub settings_dirty: bool,
    /// The project new tasks go in unless the user picks another: the one
    /// Taskboard was started in, if any.
    pub default_project: Option<PathBuf>,
    /// Projects whose directory wasn't there when last checked.
    pub missing_projects: Vec<PathBuf>,
    /// Effects waiting to be done, oldest first.
    pub(crate) outbox: VecDeque<Effect>,
}

impl App {
    /// Starts with settings and a board loaded by the platform, showing
    /// why either couldn't be.
    pub fn new(settings: Result<GlobalSettings, String>, board: Result<Board, String>) -> App {
        let mut app = App {
            board: Board::default(),
            view: View::Board,
            filter: Filter::default(),
            selected: None,
            editing: None,
            draft: Draft::default(),
            viewing_file: None,
            date_inputs: (String::new(), String::new()),
            error: None,
            settings: GlobalSettings::default(),
            settings_open: false,
            settings_dirty: false,
            default_project: None,
            missing_projects: Vec::new(),
            outbox: VecDeque::new(),
        };
        match settings {
            Ok(settings) => app.settings = settings,
            Err(e) => app.error = Some(format!("Couldn't read settings: {e}")),
        }
        app.loaded(board);
        app
    }

    /// Shows a board freshly loaded from the store.
    pub fn loaded(&mut self, board: Result<Board, String>) {
        match board {
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
                if self.viewing_file.is_some_and(|(t, f)| {
                    self.board
                        .get(t)
                        .is_none_or(|t| t.files.iter().all(|a| a.id != f))
                }) {
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
            .filter(|p| dir_missing(&p.path))
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

    /// Shows how running `command` went. If it failed, its change to the
    /// board is undone when the board is next loaded.
    pub fn finished(&mut self, command: &Command, result: Result<Outcome, String>) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Some(msg) = outcome.message() {
            self.error = Some(msg);
        }
        // Files only exist once the store has made them, so they're added
        // here rather than when the command was made.
        if let (
            Command::Attach { task, .. } | Command::NewFile { task, .. },
            Output::File { file },
        ) = (command, outcome.output)
        {
            if let Some(t) = self.board.tasks.iter_mut().find(|t| t.id == *task)
                && t.files.iter().all(|f| f.id != file.id)
            {
                t.files.push(file.clone());
            }
            if matches!(command, Command::NewFile { .. }) {
                self.viewing_file = Some((*task, file.id));
            }
        }
    }

    /// Shows whether saving settings worked.
    pub fn settings_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.error = Some(e);
        }
    }

    fn try_apply(&mut self, action: Action) -> taskboard_core::Result<()> {
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
                project: typed,
                description,
            } => {
                let Some(target) = self.editing else {
                    return Ok(());
                };
                if title.trim().is_empty() {
                    return Err(Error::Invalid("title must not be empty".into()));
                }
                // A missing directory is created when the command runs.
                let project = match self.project_input(&typed) {
                    ProjectInput::Empty => {
                        return Err(Error::Invalid("choose a project for the task".into()));
                    }
                    ProjectInput::Known(path)
                    | ProjectInput::New(path)
                    | ProjectInput::Missing(path) => path,
                };
                match target {
                    EditTarget::Existing(id) => {
                        let patch = TaskPatch {
                            title: Some(title.clone()),
                            project: Some(project),
                            description: Some(description.clone()),
                            ..Default::default()
                        };
                        let command = Command::Edit {
                            id,
                            title: Some(title),
                            project: Some(typed),
                            description: Some(description),
                        };
                        self.change(command, |b| b.edit(id, patch))?;
                        self.selected = Some(id);
                    }
                    EditTarget::New(status) => {
                        let position = self.settings.new_task_position;
                        let new = NewTask {
                            title: title.clone(),
                            project,
                            description: description.clone(),
                            status: Some(status),
                            links: self.draft.links.clone(),
                            position,
                            ..Default::default()
                        };
                        let command = Command::Create {
                            title,
                            project: typed,
                            description,
                            status: Some(status),
                            links: self.draft.links.clone(),
                            files: self.draft.files.iter().map(|f| f.source.clone()).collect(),
                            position: Some(position),
                            epic: false,
                            parent: None,
                        };
                        // The draft is kept if creating fails, e.g. on an empty title.
                        self.change(command, |b| b.create(new))?;
                        self.draft = Draft::default();
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
                let command = Command::Move {
                    id,
                    status,
                    position,
                };
                self.change(command, |b| b.move_to(id, status, position))?;
            }
            Action::Delete(id) => {
                self.change(Command::Delete { id }, |b| b.delete(id))?;
                if self.selected == Some(id) {
                    self.selected = None;
                    self.editing = None;
                }
            }
            Action::Promote(id) => self.change(Command::Promote { id }, |b| b.promote(id))?,
            Action::Demote(id) => self.change(Command::Demote { id }, |b| b.demote(id))?,
            Action::SetParent { id, parent } => {
                let command = Command::SetParent { id, parent };
                self.change(command, |b| b.set_parent(id, parent))?;
            }
            Action::AddLink { owner, url, kind } => {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    let link = Link {
                        url: url.clone(),
                        label: None,
                        kind,
                    };
                    match owner {
                        Owner::Task(task) => {
                            let command = Command::AddLink { task, url, kind };
                            self.change(command, |b| b.add_link(task, link))?;
                        }
                        Owner::Draft => self.draft.links.push(link),
                    }
                }
            }
            Action::RemoveLink { owner, index } => match owner {
                Owner::Task(task) => {
                    let command = Command::RemoveLink { task, index };
                    self.change(command, |b| b.remove_link(task, index))?;
                }
                Owner::Draft => {
                    if index < self.draft.links.len() {
                        self.draft.links.remove(index);
                    }
                }
            },
            Action::Attach { owner, source } => {
                match owner {
                    // Shown once attached; see `finished`.
                    Owner::Task(task) => self.run(Command::Attach { task, source }),
                    // A file dialog can close after the new task was saved
                    // or cancelled; don't slip its file into the next unsaved
                    // draft.
                    Owner::Draft if matches!(self.editing, Some(EditTarget::New(_))) => {
                        self.draft.attach(source)?
                    }
                    Owner::Draft => {}
                }
            }
            Action::Detach { owner, file } => match owner {
                Owner::Task(task) => {
                    self.change(Command::Detach { task, file }, |b| {
                        let t = b
                            .tasks
                            .iter_mut()
                            .find(|t| t.id == task)
                            .ok_or_else(|| Error::NotFound(task.to_string()))?;
                        t.files.retain(|f| f.id != file);
                        Ok(())
                    })?;
                    if self.viewing_file == Some((task, file)) {
                        self.viewing_file = None;
                    }
                }
                Owner::Draft => self.draft.files.retain(|f| f.id != file),
            },
            // Opened in the viewer once it's made; see `finished`.
            Action::NewFile { task, name } => self.run(Command::NewFile { task, name }),
            Action::WriteFile { task, file, text } => {
                self.run(Command::WriteFile { task, file, text });
            }
            Action::RelocateProject { from, to } => {
                let typed = to.trim();
                if typed.is_empty() {
                    return Err(Error::Invalid("choose the project's new folder".into()));
                }
                let to = project_path(Path::new(typed));
                // TODO(Platform): asks `Platform::path_kind` instead.
                if !to.is_dir() {
                    return Err(Error::Invalid(format!("no folder at {}", to.display())));
                }
                let command = Command::RelocateProject {
                    from: from.clone(),
                    to: typed.to_string(),
                };
                self.change(command, |b| b.relocate_project(&from, to.clone()))?;
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
                let command = Command::DeleteProject { path: path.clone() };
                self.change(command, |b| b.remove_project(&path))?;
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
            Action::DismissError => self.error = None,
            Action::ShowSettings(open) => {
                self.settings_open = open;
                if !open && self.settings_dirty {
                    self.save_settings();
                }
            }
            Action::SetFontSize(size) => {
                self.settings.font_size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                self.save_settings();
            }
            Action::SetCollapseEmptyColumns(collapse) => {
                self.settings.collapse_empty_columns = collapse;
                self.save_settings();
            }
            Action::SetTheme(theme) => {
                self.settings.theme = theme;
                self.save_settings();
            }
            Action::SetNewTaskPosition(position) => {
                self.settings.new_task_position = position;
                self.save_settings();
            }
            Action::ToggleColumn(status) => {
                let collapsed = self.column_collapsed(status);
                self.settings.collapsed_columns.insert(status, !collapsed);
                self.save_settings();
            }
            Action::AddHook => {
                self.settings.hooks.push(Hook::default());
                self.save_settings();
            }
            Action::RemoveHook(i) => {
                if i < self.settings.hooks.len() {
                    self.settings.hooks.remove(i);
                    self.save_settings();
                }
            }
            Action::SetHookEvent(i, event) => {
                if let Some(hook) = self.settings.hooks.get_mut(i) {
                    hook.event = event;
                    self.save_settings();
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
                    self.save_settings();
                }
            }
            Action::SetHookPassTaskId(i, pass) => {
                if let Some(hook) = self.settings.hooks.get_mut(i) {
                    hook.pass_task_id = pass;
                    self.save_settings();
                }
            }
        }
        Ok(())
    }

    /// Makes a change to the board with `f`, then queues `command` to make
    /// it in the store. If `f` fails, the board is left as it was and nothing
    /// is queued.
    fn change<T>(
        &mut self,
        command: Command,
        f: impl FnOnce(&mut Board) -> taskboard_core::Result<T>,
    ) -> taskboard_core::Result<T> {
        let mut board = self.board.clone();
        let out = f(&mut board)?;
        self.board = board;
        self.run(command);
        Ok(out)
    }

    /// Queues `command` for the platform to run.
    fn run(&mut self, command: Command) {
        // Hooks run with the settings as shown, typed commands included.
        if self.settings_dirty {
            self.save_settings();
        }
        self.outbox.push_back(Effect::Run(command));
    }

    fn save_settings(&mut self) {
        self.outbox
            .push_back(Effect::SaveSettings(self.settings.clone()));
        self.settings_dirty = false;
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

/// Whether a project's directory is gone.
// TODO(Platform): asks the backend instead of the local filesystem.
#[cfg(not(target_arch = "wasm32"))]
fn dir_missing(path: &Path) -> bool {
    !path.is_dir()
}

/// wasm spike: there's no filesystem to check, so none count as missing.
#[cfg(target_arch = "wasm32")]
fn dir_missing(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use taskboard_core::{Store, create_project_dir};

    use super::*;
    use crate::testing::{Harness, TestPlatform};

    /// An app whose store and default project, `app`, are in a temporary
    /// directory. New tasks go at the bottom, so columns read in creation order.
    fn app() -> (tempfile::TempDir, Harness) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("data")).unwrap();
        let mut app = Harness::new(TestPlatform::new(store, None));
        app.default_project = Some(create_project_dir(&dir.path().join("app")).unwrap());
        app.settings.new_task_position = NewTaskPosition::Bottom;
        (dir, app)
    }

    fn save(app: &mut Harness, title: &str) {
        app.apply(Action::Save {
            title: title.into(),
            project: app.new_task_project(),
            description: String::new(),
        });
    }

    fn add(app: &mut Harness, title: &str) -> Ulid {
        app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
        save(app, title);
        app.board.column(Status::Backlog).last().unwrap().id
    }

    /// An app with its store in `store_dir` and settings saved at `path`.
    fn settings_app(store_dir: &Path, path: &Path) -> Harness {
        let store = Store::open(store_dir).unwrap();
        Harness::new(TestPlatform::new(store, Some(path.to_path_buf())))
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
                source: FileSource::Path { path },
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
                .take()
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
    fn epic_actions_apply_and_show_refusals() {
        let (_dir, mut app) = app();
        let epic = add(&mut app, "epic");
        let child = add(&mut app, "child");
        let join = Action::SetParent {
            id: child,
            parent: Some(epic),
        };
        app.apply(join.clone());
        assert!(app.error.take().unwrap().contains("isn't an epic"));

        app.apply(Action::Promote(epic));
        app.apply(join);
        assert_eq!(app.error, None);
        assert_eq!(app.board.children(epic)[0].id, child);

        app.apply(Action::SetParent {
            id: child,
            parent: None,
        });
        app.apply(Action::Demote(epic));
        assert_eq!(app.error, None);
        assert!(!app.board.get(epic).unwrap().epic);
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
            source: FileSource::Path { path: src },
        });
        assert_eq!(app.error, None);
        assert_eq!(app.draft, Draft::default());
    }

    #[test]
    fn new_file_is_created_opened_and_writable() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        app.apply(Action::NewFile {
            task: a,
            name: "a/b".into(),
        });
        assert!(app.error.take().is_some());
        assert_eq!(app.viewing_file, None);

        app.apply(Action::NewFile {
            task: a,
            name: "plan".into(),
        });
        assert_eq!(app.error, None);
        let att = app.board.get(a).unwrap().files[0].clone();
        assert_eq!(att.name, "plan.md");
        assert_eq!(app.viewing_file, Some((a, att.id)));

        let updated = app.board.get(a).unwrap().updated;
        app.apply(Action::WriteFile {
            task: a,
            file: att.id,
            text: "# Plan\n".into(),
        });
        assert_eq!(app.error, None);
        let path = app.platform.store.file_path(&att);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# Plan\n");
        assert!(app.board.get(a).unwrap().updated >= updated);

        // The viewer closes when its file is detached elsewhere.
        app.platform.store.detach(a, att.id).unwrap(); // as if `tb file rm` ran
        app.reload();
        assert_eq!(app.viewing_file, None);
        assert!(app.board.get(a).is_some());
    }

    #[test]
    fn selection_cleared_when_task_deleted_elsewhere() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        app.apply(Action::Select(Some(a)));
        app.platform.store.delete(a).unwrap(); // as if `tb rm` ran
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
        let mut app = settings_app(dir.path(), &path);
        app.apply(Action::SetFontSize(MAX_FONT_SIZE + 10));
        assert_eq!(app.error, None);
        assert_eq!(app.settings.font_size, MAX_FONT_SIZE);
        assert_eq!(GlobalSettings::load(&path).unwrap(), app.settings);

        let reopened = settings_app(dir.path(), &path);
        assert_eq!(reopened.settings.font_size, MAX_FONT_SIZE);
    }

    #[test]
    fn new_tasks_go_to_the_top_unless_set_to_bottom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/tb/settings.json");
        let mut app = settings_app(&dir.path().join("data"), &path);
        app.default_project = Some(create_project_dir(&dir.path().join("app")).unwrap());
        assert_eq!(app.settings.new_task_position, NewTaskPosition::Top);
        for title in ["a", "b"] {
            app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
            save(&mut app, title);
        }
        app.apply(Action::SetNewTaskPosition(NewTaskPosition::Bottom));
        app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
        save(&mut app, "c");
        assert_eq!(app.error, None);
        let order: Vec<&str> = app
            .board
            .column(Status::Backlog)
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(order, ["b", "a", "c"]);

        let reopened = settings_app(&dir.path().join("data"), &path);
        assert_eq!(reopened.settings.new_task_position, NewTaskPosition::Bottom);
    }

    #[test]
    fn theme_defaults_to_dark_and_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/tb/settings.json");
        let mut app = settings_app(dir.path(), &path);
        assert_eq!(app.settings.theme, Theme::Dark);
        app.apply(Action::SetTheme(Theme::Light));
        assert_eq!(app.error, None);

        let reopened = settings_app(dir.path(), &path);
        assert_eq!(reopened.settings.theme, Theme::Light);
    }

    #[test]
    fn empty_columns_collapsed_by_default() {
        let (dir, mut app) = app();
        let settings = dir.path().join("settings.json");
        app.platform.settings_path = Some(settings.clone());
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
        let reopened = settings_app(app.platform.store.dir(), &settings);
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
        app.platform.settings_path = Some(path.clone());
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
    fn changes_show_before_their_command_runs_and_are_undone_if_it_fails() {
        let (_dir, mut app) = app();
        let epic = add(&mut app, "epic");
        let child = add(&mut app, "child");
        app.apply(Action::Promote(epic));

        // Applied without running its command yet.
        app.app.apply(Action::SetParent {
            id: child,
            parent: Some(epic),
        });
        assert_eq!(app.board.get(child).unwrap().parent, Some(epic));
        let stored = app.platform.store.load().unwrap();
        assert_eq!(stored.get(child).unwrap().parent, None);

        // Changed elsewhere meanwhile, so the command fails.
        app.platform.store.update(|b| b.demote(epic)).unwrap(); // as if `tb demote` ran
        app.settle();
        assert!(app.error.take().unwrap().contains("isn't an epic"));
        assert_eq!(app.board.get(child).unwrap().parent, None);
        assert!(!app.board.get(epic).unwrap().epic);
    }

    #[test]
    fn drop_index_counts_cards_above() {
        assert_eq!(drop_index(&[10.0, 50.0, 90.0], 60.0), 2);
        assert_eq!(drop_index(&[], 60.0), 0);
    }
}
