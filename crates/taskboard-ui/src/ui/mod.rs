//! The Iced front end: turns [`App`] state into widgets, and the user's input
//! into [`Action`]s. All changes to the board go through those actions.

mod board;
mod drag;
mod list;
mod panels;
mod popup;
mod settings;
pub mod theme;
mod toolbar;
mod touch;
mod viewer;
mod widgets;
mod zones;

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Duration;

use iced::time::Instant;

use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{column, container, mouse_area, opaque, row, stack, text_editor};
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Subscription, Task, event};
use taskboard_core::ops::{Command, FileSource, Outcome};
use taskboard_core::{Board, LinkKind, Status, Ulid};

use crate::app::{
    Action, App, EditTarget, Effect, Owner, PathCheck, ProjectInput, View, drop_index,
    drop_position,
};
use crate::platform::{PathKind, Platform, WatchEvent};
use drag::{Drag, column_at, stacked_mids};
use theme::{Pal, palette};
use widgets::El;

/// Body text size the hardcoded sizes are designed around; the user's font
/// size setting scales the whole UI relative to it.
const BASE_FONT_SIZE: f32 = 14.0;

/// Space between the window's edges and the UI inside it.
const EDGE: f32 = 18.0;

/// [`EDGE`] in compact mode, where every pixel of width counts.
const COMPACT_EDGE: f32 = 8.0;

/// Windows narrower than this, in logical pixels, get the compact layout.
const COMPACT_WIDTH: f32 = 720.0;

/// How often to check whether project folders still exist.
const PROJECT_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// How long after the last keystroke in a hook command it's saved.
const HOOK_SAVE_DELAY: Duration = Duration::from_secs(1);

const EDIT_TITLE: &str = "edit_title";
const SEARCH: &str = "search";
const NEW_FILE_NAME: &str = "new_file_name";
const FILE_EDITOR: &str = "file_editor";

/// Shown in the settings modal's keybindings page.
const KEYBINDINGS: &[(&str, &str)] = &[
    ("Ctrl+F", "Focus search"),
    ("Ctrl+N", "New task"),
    ("Ctrl+S / Ctrl+Enter", "Save the task or file being edited"),
    ("Ctrl+=", "Increase font size"),
    ("Ctrl+-", "Decrease font size"),
    ("Esc", "Close a dialog or menu, cancel editing, or deselect"),
];

pub struct Gui {
    app: App,
    ui: UiState,
    platform: Box<dyn Platform>,
    /// The effect the platform is doing; the rest wait in the app's outbox.
    running: Option<Effect>,
    /// [`PathCheck::Refresh`] checks the platform is doing. They're done
    /// alongside `running` rather than after it, so looking at paths never
    /// holds up a command, or a command a path's hint.
    refreshing: Vec<PathCheck>,
    /// The typed project path being looked for. Another isn't asked about
    /// until it's answered, so typing a path asks a few times rather than
    /// once a keystroke.
    typed_check: Option<PathBuf>,
    /// How many commands have been started, so a board loaded before the
    /// latest one is dropped.
    commands: u64,
    /// How many loads of the board have been started, and the latest one
    /// shown, so loads that finish out of order don't go back in time.
    loads: u64,
    shown_load: u64,
    /// The board is to be reloaded once no effects are left.
    stale: bool,
}

/// UI-only state that doesn't belong in [`App`].
#[derive(Default)]
struct UiState {
    confirm_delete: Option<Ulid>,
    /// Link kind the user toggled to for a task or the draft; otherwise the
    /// add-link row uses [`taskboard_core::default_link_kind`].
    link_kind: Option<(Owner, LinkKind)>,
    /// The (task, file) whose text is in `file_text`.
    file_shown: Option<(Ulid, Ulid)>,
    /// The open file viewer's text, read when it opened.
    file_text: Option<String>,
    /// The viewed file is UTF-8 text, so it can be edited without loss.
    file_editable: bool,
    /// The viewed image, once read.
    file_image: Option<iced::widget::image::Handle>,
    /// The viewed file's text being edited, while the viewer is in edit mode.
    file_edit: Option<text_editor::Content>,
    /// The task whose new-file popup is open.
    new_file_for: Option<Ulid>,
    /// The name typed in the new-file popup.
    new_file_name: String,
    /// The settings modal shows keybindings instead of settings.
    show_keybindings: bool,
    /// The filter dropdown under the search bar is open.
    filters_open: bool,
    /// The task whose status dropdown in the detail panel is open.
    status_open: Option<Ulid>,
    /// The task whose list of epics to join in the detail panel is open.
    epic_open: Option<Ulid>,
    /// The new folder typed for the selected task's missing project.
    relocate_path: String,
    /// The missing project whose Delete button was pressed once.
    confirm_delete_project: Option<PathBuf>,
    /// The editor's list of known projects is open.
    project_menu_open: bool,
    edit_title: String,
    /// The editor's project input: a project's name or directory.
    edit_project: String,
    edit_desc: text_editor::Content,
    /// The add-link and attach-file inputs, shared by the detail panel and
    /// the editor.
    link_url: String,
    attach_path: String,
    /// When a hook command was last typed in, while that edit is unsaved.
    hook_edited_at: Option<Instant>,
    hovered_card: Option<Ulid>,
    drag: Option<Drag>,
    /// The column picked from the compact board's tabs.
    compact_column: Option<Status>,
    /// The window was last laid out in compact mode. Set by the view, the
    /// only place that knows the window's size.
    compact: Cell<bool>,
}

#[derive(Debug, Clone)]
pub enum Message {
    App(Action),
    Search(String),
    DateAfter(String),
    DateBefore(String),
    ClearFilters,
    ToggleFilters,
    CloseFilters,
    /// A tab of the compact board was picked.
    ShowColumn(Status),
    /// A view was picked in compact mode, which starts on its list of tasks.
    OpenCompactView(View),
    ToggleStatusMenu(Ulid),
    CloseStatusMenu,
    /// A status was picked from the task's list.
    ChooseStatus(Ulid, Status),
    ToggleEpicMenu(Ulid),
    CloseEpicMenu,
    /// An epic was picked from the task's list: (task, epic).
    ChooseEpic(Ulid, Ulid),
    ConfirmDelete(Ulid),
    SetLinkKind(Owner, LinkKind),
    LinkUrl(String),
    AttachPath(String),
    AddLink(Owner, LinkKind),
    Attach(Owner),
    /// Opens the platform's file dialog to pick a file to attach.
    Browse(Owner),
    /// The file dialog closed; `None` if it was cancelled.
    Picked(Owner, Result<Option<FileSource>, String>),
    /// Opens or closes the popup asking for a new file's name.
    ToggleNewFile(Ulid),
    CloseNewFile,
    NewFileName(String),
    /// Creates the file named in the popup and opens it for editing.
    CreateFile(Ulid),
    /// Switches the file viewer to editing the file's text.
    EditFile,
    EditFileText(text_editor::Action),
    SaveFile,
    /// Leaves edit mode, dropping unsaved changes.
    CancelFileEdit,
    /// Closes the file viewer, unless it has unsaved changes.
    CloseViewer,
    /// The viewed (task, file) was read.
    FileRead((Ulid, Ulid), viewer::Contents),
    EditTitle(String),
    EditProject(String),
    ToggleProjectMenu,
    CloseProjectMenu,
    /// A project was picked from the editor's list, by directory.
    ChooseProject(PathBuf),
    /// Opens the native folder dialog to pick the task's project.
    BrowseProject,
    /// The folder dialog closed; `None` if it was cancelled.
    PickedProject(Option<PathBuf>),
    EditDesc(text_editor::Action),
    RelocatePath(String),
    /// Points the missing project at the typed folder.
    Relocate(PathBuf),
    /// Opens the native folder dialog to pick a missing project's new folder.
    BrowseRelocate(PathBuf),
    /// The folder dialog closed: (missing project, new folder if picked).
    PickedRelocate(PathBuf, Option<PathBuf>),
    ConfirmDeleteProject(PathBuf),
    CheckProjects,
    Save,
    HookCommand(usize, String),
    ToggleKeybindings,
    Key(Key, Modifiers, event::Status),
    OpenUrl(String),
    Watch(WatchEvent),
    /// The running command finished.
    Ran(Result<Outcome, String>),
    /// The running settings save finished.
    SettingsSaved(Result<(), String>),
    /// A path check finished: what's at each of its paths.
    Checked(PathCheck, Vec<PathKind>),
    /// A board that was loaded.
    Loaded(LoadStamp, Result<Board, String>),
    Tick,
    HoverCard(Ulid, bool),
    /// A card was dragged to a point; it's as big as the rectangle.
    DragMoved(Ulid, Point, Rectangle),
    DragZones(Vec<(iced::widget::Id, Rectangle)>),
    /// A column or the board scrolled, maybe mid-drag.
    DragScrolled,
    /// A frame of the gap under a dragged card opening or closing.
    Frame(Instant),
    Dropped(Ulid, Point),
    DropColumns(Ulid, Point, Vec<(iced::widget::Id, Rectangle)>),
    DropCards(Ulid, Status, f32, Vec<(iced::widget::Id, Rectangle)>),
}

impl Gui {
    /// Shows `app`, doing what it can't do itself (like changing the store)
    /// through `platform`.
    pub fn new(app: App, platform: impl Platform + 'static) -> Gui {
        Gui {
            app,
            ui: UiState::default(),
            platform: Box::new(platform),
            running: None,
            refreshing: Vec::new(),
            typed_check: None,
            commands: 0,
            loads: 0,
            shown_load: 0,
            stale: false,
        }
    }

    /// Starts what the app left for the platform as it loaded, like
    /// checking projects' folders; for the task `iced::application` starts.
    pub fn start(&mut self) -> Task<Message> {
        self.next_effect()
    }

    pub fn title(&self) -> String {
        "Taskboard".to_string()
    }

    pub fn theme(&self) -> iced::Theme {
        theme::iced_theme(self.app.device.theme)
    }

    pub fn scale_factor(&self) -> f32 {
        self.app.device.font_size as f32 / BASE_FONT_SIZE
    }

    fn pal(&self) -> Pal {
        palette(self.app.device.theme, self.ui.compact.get())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let keys = event::listen_with(|event, status, _| match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::Key(key, modifiers, status))
            }
            _ => None,
        });
        let mut subs = vec![
            keys,
            iced::time::every(PROJECT_CHECK_INTERVAL).map(|_| Message::CheckProjects),
            self.platform.board_changes().map(Message::Watch),
        ];
        if self.ui.hook_edited_at.is_some() {
            subs.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick));
        }
        if self.ui.drag.as_ref().is_some_and(Drag::animating) {
            subs.push(iced::window::frames().map(Message::Frame));
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let was_editing = self.app.editing.is_some();
        let was_selected = self.app.selected;
        let mut task = self.handle(message);
        // The viewer can't be closed with unsaved changes, so they're only
        // lost when the file or its task is removed elsewhere.
        if self.app.viewing_file != self.ui.file_shown
            && self.file_edit_dirty()
            && let Some(content) = &self.ui.file_edit
        {
            self.app.error = Some(
                "The file being edited was removed elsewhere; your unsaved text was copied to the clipboard."
                    .into(),
            );
            task = Task::batch([task, iced::clipboard::write(content.text())]);
        }
        let (app, ui) = (&self.app, &mut self.ui);
        if !app.settings_open {
            ui.show_keybindings = false;
            ui.hook_edited_at = None;
        }
        if app.editing.is_some() || app.selected != ui.status_open {
            ui.status_open = None;
        }
        // A task that's become an epic or a child elsewhere has no list of
        // epics, which mustn't pop open if it later stops being one.
        let can_join_epic = app
            .selected_task()
            .is_some_and(|t| !t.epic && app.board.parent_of(t).is_none());
        if app.editing.is_some() || app.selected != ui.epic_open || !can_join_epic {
            ui.epic_open = None;
        }
        if app.editing.is_none() {
            ui.project_menu_open = false;
        }
        if app.selected.is_none_or(|id| ui.new_file_for != Some(id)) || app.editing.is_some() {
            ui.new_file_for = None;
        }
        if ui.new_file_for.is_none() {
            ui.new_file_name.clear();
        }
        // The file is read once per file shown, so reloads (like the
        // watcher's after a save) don't disturb the viewer or an edit in
        // progress.
        if app.viewing_file != ui.file_shown {
            ui.file_shown = app.viewing_file;
            ui.file_text = None;
            ui.file_editable = false;
            ui.file_image = None;
            ui.file_edit = None;
            if let Some((task_id, file)) = app.viewing_file
                && let Some(att) = app
                    .board
                    .get(task_id)
                    .and_then(|t| t.files.iter().find(|f| f.id == file))
            {
                let kind = att.kind;
                let read = Task::perform(self.platform.read_file(task_id, att), move |bytes| {
                    Message::FileRead((task_id, file), viewer::contents(kind, bytes))
                });
                task = Task::batch([task, read]);
            }
        }
        // A missing project's fix belongs to the task it was started on.
        if app.selected != was_selected {
            ui.relocate_path.clear();
            ui.confirm_delete_project = None;
        }
        // Don't leave a new task's unadded link or file in the detail panel.
        if was_editing && app.editing.is_none() {
            ui.link_url.clear();
            ui.attach_path.clear();
        }
        // A typed project's folder is looked for once; see
        // `Message::CheckProjects` for looking again.
        if app.editing.is_some()
            && self.typed_check.is_none()
            && let ProjectInput::Unchecked(path) = app.project_input(&ui.edit_project)
            && self.app.check_paths(vec![path.clone()])
        {
            self.typed_check = Some(path);
        }
        Task::batch([task, self.next_effect()])
    }

    /// Starts the path refreshes in the app's outbox, and the next of its
    /// other effects unless one is running, or, with none left, reloads the
    /// board if it's stale.
    fn next_effect(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        let outbox = std::mem::take(&mut self.app.outbox);
        for effect in outbox {
            match effect {
                Effect::CheckPaths(check @ PathCheck::Refresh { .. }) => {
                    self.refreshing.push(check.clone());
                    let kinds = self.platform.path_kinds(check.paths());
                    tasks.push(Task::perform(kinds, move |kinds| {
                        Message::Checked(check.clone(), kinds)
                    }));
                }
                effect => self.app.outbox.push_back(effect),
            }
        }
        tasks.push(self.next_ordered_effect());
        Task::batch(tasks)
    }

    fn next_ordered_effect(&mut self) -> Task<Message> {
        if self.running.is_some() {
            return Task::none();
        }
        if let Some(effect) = self.app.outbox.pop_front() {
            self.running = Some(effect.clone());
            return match effect {
                Effect::Run(command) => {
                    self.commands += 1;
                    Task::perform(self.platform.run(command), Message::Ran)
                }
                Effect::SaveSettings(settings) => Task::perform(
                    self.platform.save_settings(settings),
                    Message::SettingsSaved,
                ),
                Effect::SaveDeviceSettings(settings) => Task::perform(
                    self.platform.save_device_settings(settings),
                    Message::SettingsSaved,
                ),
                Effect::CheckPaths(check) => {
                    let kinds = self.platform.path_kinds(check.paths());
                    Task::perform(kinds, move |kinds| Message::Checked(check.clone(), kinds))
                }
            };
        }
        if std::mem::take(&mut self.stale) {
            let stamp = self.load_stamp();
            return Task::perform(self.platform.load_board(), move |board| {
                Message::Loaded(stamp, board)
            });
        }
        Task::none()
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        let ui = &mut self.ui;
        match message {
            Message::App(action) => return self.apply(action),
            Message::Search(s) => self.app.apply(Action::SetSearch(s)),
            Message::DateAfter(after) => {
                let before = self.app.date_inputs.1.clone();
                self.app.apply(Action::SetDateBounds { after, before });
            }
            Message::DateBefore(before) => {
                let after = self.app.date_inputs.0.clone();
                self.app.apply(Action::SetDateBounds { after, before });
            }
            Message::ClearFilters => self.app.apply(Action::ClearFilters),
            Message::ToggleFilters => ui.filters_open = !ui.filters_open,
            Message::CloseFilters => ui.filters_open = false,
            Message::ShowColumn(status) => ui.compact_column = Some(status),
            Message::OpenCompactView(view) => {
                // The In Progress view opens on a task's details, which here
                // would cover the list it's chosen from.
                self.app.apply(Action::SetView(view));
                self.app.apply(Action::Select(None));
            }
            Message::ToggleStatusMenu(id) => {
                ui.status_open = match ui.status_open {
                    Some(_) => None,
                    None => Some(id),
                }
            }
            Message::CloseStatusMenu => ui.status_open = None,
            Message::ChooseStatus(id, status) => {
                ui.status_open = None;
                return self.apply(Action::Move {
                    id,
                    status,
                    position: None,
                });
            }
            Message::ToggleEpicMenu(id) => {
                ui.epic_open = match ui.epic_open {
                    Some(_) => None,
                    None => Some(id),
                }
            }
            Message::CloseEpicMenu => ui.epic_open = None,
            Message::ChooseEpic(id, epic) => {
                ui.epic_open = None;
                self.app.apply(Action::SetParent {
                    id,
                    parent: Some(epic),
                });
            }
            Message::ConfirmDelete(id) => ui.confirm_delete = Some(id),
            Message::SetLinkKind(owner, kind) => ui.link_kind = Some((owner, kind)),
            Message::LinkUrl(s) => ui.link_url = s,
            Message::AttachPath(s) => ui.attach_path = s,
            Message::AddLink(owner, kind) => {
                let url = std::mem::take(&mut ui.link_url);
                ui.link_kind = None;
                self.app.apply(Action::AddLink { owner, url, kind });
            }
            Message::Attach(owner) => {
                // The input is hidden then, but Enter could still submit it.
                if self.platform.attaches_paths() && !ui.attach_path.trim().is_empty() {
                    let path = typed_path(&std::mem::take(&mut ui.attach_path));
                    let source = FileSource::Path { path };
                    self.app.apply(Action::Attach { owner, source });
                }
            }
            Message::Browse(owner) => {
                return Task::perform(self.platform.pick_file(), move |source| {
                    Message::Picked(owner, source)
                });
            }
            Message::Picked(owner, Ok(Some(source))) => {
                self.app.apply(Action::Attach { owner, source });
            }
            Message::Picked(_, Ok(None)) => {}
            Message::Picked(_, Err(e)) => self.app.error = Some(e),
            Message::ToggleNewFile(task) => {
                ui.new_file_for = match ui.new_file_for {
                    Some(_) => None,
                    None => Some(task),
                };
                if ui.new_file_for.is_some() {
                    return iced::widget::operation::focus(NEW_FILE_NAME);
                }
            }
            Message::CloseNewFile => ui.new_file_for = None,
            Message::NewFileName(s) => ui.new_file_name = s,
            Message::CreateFile(task) => {
                let name = ui.new_file_name.clone();
                // Any earlier error is cleared so a failed create shows.
                self.app.error = None;
                // Opened once made; see `Gui::ran`.
                self.app.apply(Action::NewFile { task, name });
            }
            Message::EditFile => {
                if ui.file_editable
                    && let Some(text) = &ui.file_text
                {
                    ui.file_edit = Some(text_editor::Content::with_text(text));
                    return iced::widget::operation::focus(FILE_EDITOR);
                }
            }
            Message::EditFileText(action) => {
                if let Some(content) = &mut ui.file_edit {
                    content.perform(action);
                }
            }
            Message::SaveFile => {
                if let (Some(content), Some((task, file))) = (&ui.file_edit, self.app.viewing_file)
                {
                    let text = content.text();
                    // Any earlier error is cleared so a failed save shows.
                    self.app.error = None;
                    // Leaves edit mode once saved; see `Gui::ran`.
                    self.app.apply(Action::WriteFile { task, file, text });
                }
            }
            Message::CancelFileEdit => ui.file_edit = None,
            Message::FileRead(shown, contents) => {
                if ui.file_shown == Some(shown) {
                    match contents {
                        viewer::Contents::Text { text, editable } => {
                            ui.file_text = Some(text);
                            ui.file_editable = editable;
                        }
                        viewer::Contents::Image(handle) => ui.file_image = Some(handle),
                    }
                }
            }
            Message::CloseViewer => {
                if !self.file_edit_dirty() {
                    return self.apply(Action::ViewFile(None));
                }
            }
            Message::EditTitle(s) => ui.edit_title = s,
            Message::EditProject(s) => ui.edit_project = s,
            Message::ToggleProjectMenu => ui.project_menu_open = !ui.project_menu_open,
            Message::CloseProjectMenu => ui.project_menu_open = false,
            Message::ChooseProject(path) => {
                ui.edit_project = self.app.project_text(&path);
                ui.project_menu_open = false;
            }
            Message::BrowseProject => {
                ui.project_menu_open = false;
                let title = "Choose the task's project";
                return Task::perform(self.platform.pick_folder(title), Message::PickedProject);
            }
            Message::PickedProject(Some(path)) => ui.edit_project = self.app.project_text(&path),
            Message::PickedProject(None) => {}
            Message::EditDesc(action) => ui.edit_desc.perform(action),
            Message::RelocatePath(s) => ui.relocate_path = s,
            Message::Relocate(from) => {
                let to = ui.relocate_path.clone();
                self.app.apply(Action::RelocateProject { from, to });
            }
            Message::BrowseRelocate(from) => {
                let title = "Choose the project's new folder";
                return Task::perform(self.platform.pick_folder(title), move |to| {
                    Message::PickedRelocate(from.clone(), to)
                });
            }
            Message::PickedRelocate(from, Some(to)) => {
                let to = to.display().to_string();
                self.app.apply(Action::RelocateProject { from, to });
            }
            Message::PickedRelocate(_, None) => {}
            Message::ConfirmDeleteProject(path) => ui.confirm_delete_project = Some(path),
            Message::CheckProjects => {
                self.app.check_projects();
                // The typed project's folder may have been made or removed.
                if self.app.editing.is_some()
                    && let ProjectInput::New(path) | ProjectInput::Missing(path) =
                        self.app.project_input(&ui.edit_project)
                {
                    self.app.check_paths(vec![path]);
                }
            }
            Message::Save => {
                // The file editor sits above the task editor, so it saves first.
                if ui.file_edit.is_some() {
                    return self.handle(Message::SaveFile);
                } else if self.app.editing.is_some() {
                    self.app.apply(Action::Save {
                        title: ui.edit_title.clone(),
                        project: ui.edit_project.clone(),
                        description: ui.edit_desc.text(),
                    });
                }
            }
            Message::HookCommand(i, command) => {
                ui.hook_edited_at = Some(Instant::now());
                self.app.apply(Action::SetHookCommand(i, command));
            }
            Message::ToggleKeybindings => ui.show_keybindings = !ui.show_keybindings,
            Message::Key(key, modifiers, status) => return self.key(key, modifiers, status),
            Message::OpenUrl(url) => {
                if let Err(e) = self.platform.open_url(&url) {
                    self.app.error = Some(e);
                }
            }
            Message::Watch(WatchEvent::Changed) => self.stale = true,
            Message::Watch(WatchEvent::Failed(e)) => {
                self.app.error = Some(format!("Live updates are off; {e}"));
            }
            Message::Ran(result) => {
                let Some(Effect::Run(command)) = self.running.take() else {
                    return Task::none();
                };
                // Reloaded even if it failed, to undo its change to the board.
                self.stale = true;
                let ok = result.is_ok();
                let viewing = self.app.viewing_file;
                self.app.finished(&command, result);
                let opened = self.app.viewing_file != viewing;
                return self.ran(command, ok, opened);
            }
            Message::SettingsSaved(result) => {
                self.running = None;
                self.app.settings_saved(result);
            }
            Message::Checked(check, kinds) => {
                if let PathCheck::Refresh { paths, .. } = &check {
                    let Some(i) = self.refreshing.iter().position(|c| *c == check) else {
                        return Task::none();
                    };
                    self.refreshing.remove(i);
                    if self.typed_check.as_ref().is_some_and(|p| paths.contains(p)) {
                        self.typed_check = None;
                    }
                } else if self.running.as_ref() == Some(&Effect::CheckPaths(check.clone())) {
                    self.running = None;
                } else {
                    return Task::none();
                }
                let relocating = match &check {
                    PathCheck::Relocate { from, .. } => Some(from.clone()),
                    _ => None,
                };
                self.app.checked(check, kinds);
                // Keep the typed folder to fix if relocating failed.
                if relocating.is_some_and(|from| self.app.board.project(&from).is_none()) {
                    ui.relocate_path.clear();
                }
            }
            Message::Loaded(stamp, board) => {
                // A board loaded before the latest command started, or
                // without the changes of those waiting, would undo them until
                // the reload that follows them. One loaded before the board
                // shown is older than it.
                let waiting = self.app.outbox.iter().any(|e| matches!(e, Effect::Run(_)));
                if stamp.commands == self.commands && !waiting && stamp.load > self.shown_load {
                    self.shown_load = stamp.load;
                    self.app.loaded(board);
                }
            }
            Message::Tick => {
                if ui
                    .hook_edited_at
                    .is_some_and(|at| at.elapsed() >= HOOK_SAVE_DELAY)
                {
                    ui.hook_edited_at = None;
                    self.app.apply(Action::SaveSettings);
                }
            }
            Message::HoverCard(id, entered) => {
                if entered {
                    ui.hovered_card = Some(id);
                } else if ui.hovered_card == Some(id) {
                    ui.hovered_card = None;
                }
            }
            Message::DragMoved(id, point, bounds) => {
                if ui.drag.as_ref().is_none_or(|d| d.id != id) {
                    let Some(status) = self.app.board.get(id).map(|t| t.status) else {
                        return Task::none();
                    };
                    let visible: Vec<Ulid> = self
                        .app
                        .visible_column(status)
                        .iter()
                        .map(|t| t.id)
                        .collect();
                    let next = visible.iter().skip_while(|t| **t != id).nth(1).copied();
                    let mut ids = board::column_ids();
                    for status in Status::ALL {
                        ids.push(board::cards_id(status));
                        ids.extend(
                            self.app
                                .visible_column(status)
                                .iter()
                                .map(|t| board::card_id(t.id)),
                        );
                    }
                    let mut drag = Drag::new(id, bounds.height, (status, next), ids.clone());
                    drag.pointer = point;
                    ui.drag = Some(drag);
                    // Measure the board as it was before the card left it.
                    return zones::find(ids).map(Message::DragZones);
                }
                if let Some(drag) = &mut ui.drag {
                    drag.hover(point);
                }
            }
            Message::DragZones(zones) => {
                if let Some(drag) = &mut ui.drag {
                    drag.columns = board::columns_from(&zones);
                    let bounds = |id: iced::widget::Id| {
                        zones.iter().find(|(z, _)| *z == id).map(|(_, b)| *b)
                    };
                    for status in Status::ALL {
                        // Only columns showing their cards have a list.
                        let Some(list) = bounds(board::cards_id(status)) else {
                            continue;
                        };
                        let cards: Vec<(Ulid, f32)> = self
                            .app
                            .visible_column(status)
                            .iter()
                            .filter(|t| t.id != drag.id)
                            .filter_map(|t| Some((t.id, bounds(board::card_id(t.id))?.height)))
                            .collect();
                        let heights: Vec<f32> = cards.iter().map(|(_, h)| *h).collect();
                        let mids = stacked_mids(list.y, &heights);
                        let cards = cards.iter().map(|(id, _)| *id).zip(mids).collect();
                        drag.cards.insert(status, cards);
                    }
                    // Place the gap now rather than on the next move.
                    drag.hover(drag.pointer);
                }
            }
            Message::DragScrolled => {
                if let Some(drag) = &ui.drag {
                    return zones::find(drag.zones.clone()).map(Message::DragZones);
                }
            }
            Message::Frame(now) => {
                if let Some(drag) = &mut ui.drag {
                    drag.frame(now);
                }
            }
            Message::Dropped(id, point) => {
                let Some(drag) = ui.drag.take() else {
                    return Task::none();
                };
                if drag.cancelled {
                    return Task::none();
                }
                // Land where the gap showed, unless the board wasn't measured
                // in time to show one.
                if drag.columns.is_empty() {
                    return zones::find(board::column_ids())
                        .map(move |zones| Message::DropColumns(id, point, zones));
                }
                if let Some((status, before)) = drag.target {
                    let visible = self.visible_except(status, id);
                    let index = before
                        .and_then(|b| visible.iter().position(|t| *t == b))
                        .unwrap_or(visible.len());
                    self.drop_at(id, status, &visible, index);
                }
            }
            Message::DropColumns(id, point, zones) => {
                let Some(status) = column_at(&board::columns_from(&zones), point) else {
                    return Task::none();
                };
                let visible = self.visible_except(status, id);
                // A collapsed column shows no cards, so drops land at its bottom.
                if self.app.column_collapsed(status) {
                    self.drop_at(id, status, &visible, visible.len());
                } else {
                    let ids = visible.iter().map(|t| board::card_id(*t)).collect();
                    return zones::find(ids)
                        .map(move |cards| Message::DropCards(id, status, point.y, cards));
                }
            }
            Message::DropCards(id, status, y, cards) => {
                let visible = self.visible_except(status, id);
                let mids: Vec<f32> = visible
                    .iter()
                    .filter_map(|t| cards.iter().find(|(c, _)| *c == board::card_id(*t)))
                    .map(|(_, b)| b.y + b.height / 2.0)
                    .collect();
                self.drop_at(id, status, &visible, drop_index(&mids, y));
            }
        }
        Task::none()
    }

    /// Starts a load of the board.
    fn load_stamp(&mut self) -> LoadStamp {
        self.loads += 1;
        LoadStamp {
            load: self.loads,
            commands: self.commands,
        }
    }

    /// Updates the file viewer for a command that finished: whether it
    /// worked, and whether the app opened a file it made in the viewer.
    fn ran(&mut self, command: Command, ok: bool, opened: bool) -> Task<Message> {
        let ui = &mut self.ui;
        match command {
            Command::NewFile { task, .. } if ok => {
                // The popup stays open with the typed name to fix if
                // creating failed.
                if ui.new_file_for == Some(task) {
                    ui.new_file_for = None;
                }
                // The new file is empty; it opens ready to edit.
                if opened {
                    ui.file_shown = self.app.viewing_file;
                    ui.file_text = Some(String::new());
                    ui.file_editable = true;
                    ui.file_image = None;
                    ui.file_edit = Some(text_editor::Content::new());
                    return iced::widget::operation::focus(FILE_EDITOR);
                }
            }
            // Edit mode is left once the text is saved, unless more was
            // typed meanwhile.
            Command::WriteFile { task, file, text }
                if ok && ui.file_shown == Some((task, file)) =>
            {
                if ui.file_edit.as_ref().is_some_and(|c| c.text() == text) {
                    ui.file_edit = None;
                }
                ui.file_text = Some(text);
            }
            _ => {}
        }
        Task::none()
    }

    /// The file being edited has changes that aren't saved.
    fn file_edit_dirty(&self) -> bool {
        self.ui
            .file_edit
            .as_ref()
            .is_some_and(|c| Some(c.text()) != self.ui.file_text)
    }

    /// The ids of a column's visible tasks, leaving out the dragged one.
    fn visible_except(&self, status: Status, dragged: Ulid) -> Vec<Ulid> {
        self.app
            .visible_column(status)
            .iter()
            .map(|t| t.id)
            .filter(|id| *id != dragged)
            .collect()
    }

    fn drop_at(&mut self, id: Ulid, status: Status, visible: &[Ulid], index: usize) {
        let position = drop_position(&self.app.board, status, visible, index, id);
        self.app.apply(Action::Move {
            id,
            status,
            position: Some(position),
        });
    }

    /// Applies `action`, filling the editor's inputs when it opens.
    fn apply(&mut self, action: Action) -> Task<Message> {
        let opens_editor = match action {
            Action::Edit(target) => Some(target),
            _ => None,
        };
        self.app.apply(action);
        let Some(target) = opens_editor else {
            return Task::none();
        };
        let (title, project, desc) = match target {
            EditTarget::Existing(id) => self
                .app
                .board
                .get(id)
                .map(|t| {
                    let project = self.app.project_text(&t.project);
                    (t.title.clone(), project, t.description.clone())
                })
                .unwrap_or_default(),
            EditTarget::New(_) => (String::new(), self.app.new_task_project(), String::new()),
        };
        let ui = &mut self.ui;
        ui.edit_title = title;
        ui.edit_project = project;
        ui.project_menu_open = false;
        ui.edit_desc = text_editor::Content::with_text(&desc);
        ui.link_url.clear();
        ui.attach_path.clear();
        ui.link_kind = None;
        iced::widget::operation::focus(EDIT_TITLE)
    }

    fn key(&mut self, key: Key, modifiers: Modifiers, status: event::Status) -> Task<Message> {
        let ctrl = modifiers.control() || modifiers.logo();
        let ui = &mut self.ui;
        let app = &self.app;
        match key.as_ref() {
            Key::Named(Named::Escape) => {
                if let Some(drag) = &mut ui.drag {
                    drag.cancel();
                }
                if app.settings_open {
                    return self.apply(Action::ShowSettings(false));
                } else if ui.filters_open {
                    ui.filters_open = false;
                } else if ui.project_menu_open {
                    ui.project_menu_open = false;
                } else if ui.status_open.is_some() {
                    ui.status_open = None;
                } else if ui.epic_open.is_some() {
                    ui.epic_open = None;
                } else if ui.new_file_for.is_some() {
                    ui.new_file_for = None;
                } else if ui.file_edit.is_some() {
                    // Unsaved changes are only dropped by pressing Cancel.
                    if !self.file_edit_dirty() {
                        self.ui.file_edit = None;
                    }
                } else if app.viewing_file.is_some() {
                    return self.apply(Action::ViewFile(None));
                } else if app.editing.is_some() {
                    return self.apply(Action::CancelEdit);
                } else if status == event::Status::Captured {
                    // A focused input took the Esc to drop its focus.
                } else if app.selected.is_some() {
                    return self.apply(Action::Select(None));
                }
            }
            Key::Character("f") if ctrl => return iced::widget::operation::focus(SEARCH),
            Key::Character("s") | Key::Named(Named::Enter) if ctrl => {
                return self.handle(Message::Save);
            }
            Key::Character("n") if ctrl && app.editing.is_none() => {
                return self.apply(Action::Edit(EditTarget::New(Status::Open)));
            }
            Key::Character("=" | "+") if ctrl => {
                return self.apply(Action::SetFontSize(app.device.font_size + 1));
            }
            Key::Character("-" | "_") if ctrl => {
                return self.apply(Action::SetFontSize(app.device.font_size - 1));
            }
            _ => {}
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Touch scrolls and taps everywhere, rather than being taken by
        // whichever button or card it lands on.
        touch::touch_input(iced::widget::responsive(move |size| self.view_sized(size))).into()
    }

    fn view_sized(&self, size: Size) -> El<'_> {
        let compact = is_compact(size.width);
        self.ui.compact.set(compact);
        let (app, p) = (&self.app, self.pal());
        let mut page = column![toolbar::toolbar(self, size.width)];
        if let Some(err) = &app.error {
            page = page.push(toolbar::error_bar(p, err));
        }
        let main: El = match app.view {
            View::Board if compact => board::compact_board(self, size.width),
            View::Board => board::board(self, size.width),
            View::List => list::list(self, compact),
            View::InProgress => list::in_progress(self, compact),
        };
        let mut body = row![main].spacing(12).height(Length::Fill);
        // The In Progress view shows the editor and details itself, and
        // compact mode shows them in a sheet.
        if !compact && app.view != View::InProgress {
            let width = side_width(size.width);
            if app.editing.is_some() {
                body = body.push(panels::editor(self, Length::Fixed(width)));
            } else if let Some(task) = app.selected_task() {
                body = body.push(panels::detail(self, task, Length::Fixed(width)));
            }
        }
        let edge = edge(size.width);
        page = page
            .push(container(body).padding(iced::Padding::ZERO.right(edge).bottom(edge).left(edge)));
        let page = container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| theme::fill(p.bg, 0.0));

        let mut layers = stack![page];
        if compact {
            layers = layers.push(self.sheet());
        }
        if let Some((task_id, file_id)) = app.viewing_file
            && let Some(att) = app
                .board
                .get(task_id)
                .and_then(|t| t.files.iter().find(|f| f.id == file_id))
        {
            let dialog = viewer::file_viewer(self, att, size);
            layers = layers.push(modal(dialog, Message::CloseViewer));
        }
        if app.settings_open {
            let dialog = settings::settings(self, size);
            layers = layers.push(modal(dialog, Message::App(Action::ShowSettings(false))));
        }
        layers.into()
    }

    /// The editor or the selected task's details, covering the window. It's
    /// laid over the page, which stays put, so closing it goes back to the
    /// same place in the board or list.
    fn sheet(&self) -> Option<El<'_>> {
        let (app, p) = (&self.app, self.pal());
        let panel = if app.editing.is_some() {
            panels::editor(self, Length::Fill)
        } else {
            panels::detail(self, app.selected_task()?, Length::Fill)
        };
        let mut sheet = column![];
        if let Some(err) = &app.error {
            sheet = sheet.push(toolbar::error_bar(p, err));
        }
        Some(opaque(
            container(sheet.push(panel))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| theme::fill(p.panel, 0.0)),
        ))
    }

    /// The column the compact board shows: the one last picked, or else the
    /// first with tasks.
    fn compact_column(&self) -> Status {
        let app = &self.app;
        self.ui.compact_column.unwrap_or_else(|| {
            Status::ALL
                .into_iter()
                .find(|s| !app.visible_column(*s).is_empty())
                .unwrap_or(Status::Open)
        })
    }
}

/// Whether a window of this width gets the compact layout.
fn is_compact(window_width: f32) -> bool {
    window_width < COMPACT_WIDTH
}

/// The space kept between a window of this width and the UI inside it.
fn edge(window_width: f32) -> f32 {
    if is_compact(window_width) {
        COMPACT_EDGE
    } else {
        EDGE
    }
}

/// How wide a dialog is that would otherwise be `width`: all of a compact
/// window.
fn dialog_width(window_width: f32, width: f32) -> f32 {
    if is_compact(window_width) {
        window_width
    } else {
        width
    }
}

/// A path typed into the attach input, trimmed and with a leading `~/`
/// expanded. Picked paths are used as they are.
fn typed_path(text: &str) -> PathBuf {
    let text = text.trim();
    match (text.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(text),
    }
}

/// When a load of the board was started: the `load`th, after `commands`
/// commands; see [`Message::Loaded`].
#[derive(Debug, Clone, Copy)]
pub struct LoadStamp {
    load: u64,
    commands: u64,
}

/// `dialog` centered over a scrim that covers the window. A press on the
/// scrim sends `on_close`.
fn modal<'a>(dialog: El<'a>, on_close: Message) -> El<'a> {
    opaque(
        mouse_area(
            container(opaque(dialog))
                .center(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                    ..container::Style::default()
                }),
        )
        .on_press(on_close),
    )
}

/// The width of the panel beside the board or list.
fn side_width(window_width: f32) -> f32 {
    (window_width * 0.42)
        .clamp(340.0, 480.0)
        .min(window_width * 0.6)
}

#[cfg(test)]
mod tests {
    use taskboard_core::{Store, create_project_dir};
    use text_editor::Edit;

    use super::*;
    use crate::testing::{Harness, TestPlatform, now};

    /// A GUI with one task, `(task, file)`, whose attached text file holds
    /// `text`, not yet open in the viewer.
    fn gui_with_file(text: &str) -> (tempfile::TempDir, Gui, (Ulid, Ulid)) {
        gui_with_file_on(text, |_| {})
    }

    /// [`gui_with_file`], on a platform `setup` has changed.
    fn gui_with_file_on(
        text: &str,
        setup: impl FnOnce(&mut TestPlatform),
    ) -> (tempfile::TempDir, Gui, (Ulid, Ulid)) {
        let dir = tempfile::tempdir().unwrap();
        let mut platform = TestPlatform::new(store(&dir), None);
        setup(&mut platform);
        let mut app = Harness::new(platform);
        app.default_project = Some(create_project_dir(&dir.path().join("app")).unwrap());
        app.apply(Action::Edit(EditTarget::New(Status::Open)));
        app.apply(Action::Save {
            title: "alpha".into(),
            project: app.new_task_project(),
            description: String::new(),
        });
        let task = app.board.column(Status::Open)[0].id;
        app.apply(Action::NewFile {
            task,
            name: "notes.txt".into(),
        });
        let file = app.board.get(task).unwrap().files[0].id;
        app.apply(Action::WriteFile {
            task,
            file,
            text: text.into(),
        });
        app.apply(Action::ViewFile(None));
        app.apply(Action::Select(Some(task)));
        assert_eq!(app.error, None);
        let Harness { app, platform } = app;
        (dir, Gui::new(app, platform), (task, file))
    }

    /// The store [`gui_with_file`] made in `dir`, to change as `tb` would.
    fn store(dir: &tempfile::TempDir) -> Store {
        Store::open(&dir.path().join("data")).unwrap()
    }

    /// Does what `gui` left for the platform, as iced would by running the
    /// tasks its updates returned: its effects, a reload of the board, and
    /// reading the file in the viewer.
    fn settle(gui: &mut Gui) {
        run_effects(gui);
        let stamp = gui.load_stamp();
        let board = now(gui.platform.load_board());
        let _ = gui.update(Message::Loaded(stamp, board));
        run_effects(gui);
        if let Some((task, file)) = gui.ui.file_shown
            && gui.ui.file_text.is_none()
            && gui.ui.file_image.is_none()
            && let Some(att) = gui
                .app
                .board
                .get(task)
                .and_then(|t| t.files.iter().find(|f| f.id == file))
        {
            let contents = viewer::contents(att.kind, now(gui.platform.read_file(task, att)));
            let _ = gui.update(Message::FileRead((task, file), contents));
        }
    }

    /// Does the effects `gui` started, and those they lead to, oldest first.
    fn run_effects(gui: &mut Gui) {
        while let Some(effect) = gui
            .running
            .clone()
            .or_else(|| gui.refreshing.first().cloned().map(Effect::CheckPaths))
        {
            let message = match effect {
                Effect::Run(command) => Message::Ran(now(gui.platform.run(command))),
                Effect::SaveSettings(settings) => {
                    Message::SettingsSaved(now(gui.platform.save_settings(settings)))
                }
                Effect::SaveDeviceSettings(settings) => {
                    Message::SettingsSaved(now(gui.platform.save_device_settings(settings)))
                }
                Effect::CheckPaths(check) => {
                    let kinds = now(gui.platform.path_kinds(check.paths()));
                    Message::Checked(check, kinds)
                }
            };
            let _ = gui.update(message);
        }
    }

    /// The path refresh `gui` started for `path` alone, if any.
    fn refresh_of(gui: &Gui, path: &std::path::Path) -> Option<PathCheck> {
        gui.refreshing.iter().find(|c| c.paths() == [path]).cloned()
    }

    fn view_file(gui: &mut Gui, file: (Ulid, Ulid)) {
        let _ = gui.update(Message::App(Action::ViewFile(Some(file))));
        settle(gui);
    }

    fn type_x(gui: &mut Gui) {
        let _ = gui.update(Message::EditFileText(text_editor::Action::Edit(
            Edit::Insert('x'),
        )));
    }

    fn esc(gui: &mut Gui) {
        let _ = gui.update(Message::Key(
            Key::Named(Named::Escape),
            Modifiers::default(),
            event::Status::Ignored,
        ));
    }

    #[test]
    fn compact_board_shows_the_first_column_with_tasks_until_one_is_picked() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::App(Action::Move {
            id: task,
            status: Status::Backlog,
            position: None,
        }));
        settle(&mut gui);
        assert_eq!(gui.compact_column(), Status::Backlog);

        let _ = gui.update(Message::ShowColumn(Status::Closed));
        assert_eq!(gui.compact_column(), Status::Closed);
    }

    #[test]
    fn choosing_a_status_moves_the_task_and_closes_the_menu() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::ToggleStatusMenu(task));
        let _ = gui.update(Message::ChooseStatus(task, Status::Closed));
        settle(&mut gui);
        assert_eq!(gui.ui.status_open, None);
        assert_eq!(gui.app.selected_task().unwrap().status, Status::Closed);
    }

    #[test]
    fn controls_are_touch_sized_only_in_a_compact_window() {
        let (_dir, gui, _) = gui_with_file("");
        let _ = gui.view_sized(Size::new(390.0, 844.0));
        assert_eq!(gui.pal().control(28.0), 44.0);
        assert_eq!(gui.pal().pad_y(18.0, 6.0), 13.0);
        let _ = gui.view_sized(Size::new(1000.0, 844.0));
        assert_eq!(gui.pal().control(28.0), 28.0);
        assert_eq!(gui.pal().pad_y(18.0, 6.0), 6.0);
    }

    #[test]
    fn compact_in_progress_view_starts_on_its_list() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::App(Action::Move {
            id: task,
            status: Status::InProgress,
            position: None,
        }));
        settle(&mut gui);
        let _ = gui.update(Message::App(Action::Select(None)));
        let _ = gui.update(Message::OpenCompactView(View::InProgress));
        assert_eq!(gui.app.view, View::InProgress);
        assert_eq!(gui.app.selected, None);
    }

    #[test]
    fn unedited_file_is_clean_whatever_its_line_endings() {
        for text in [
            "",
            "a",
            "a\n",
            "a\nb",
            "a\nb\n",
            "\n\n",
            "a\r\nb\r\n",
            "a\r\nb",
        ] {
            let (_dir, mut gui, file) = gui_with_file(text);
            view_file(&mut gui, file);
            let _ = gui.update(Message::EditFile);
            assert!(gui.ui.file_edit.is_some(), "{text:?}");
            assert!(!gui.file_edit_dirty(), "{text:?}");

            let _ = gui.update(Message::EditFileText(text_editor::Action::Edit(
                Edit::Insert('x'),
            )));
            assert!(gui.file_edit_dirty(), "{text:?}");
        }
    }

    #[test]
    fn typed_paths_are_not_attached_where_the_platform_refuses_them() {
        let (dir, mut gui, (task, _)) = gui_with_file_on("", |p| p.attaches_paths = false);
        let path = dir.path().join("secret.txt");
        std::fs::write(&path, "x").unwrap();
        let _ = gui.update(Message::AttachPath(path.display().to_string()));
        let _ = gui.update(Message::Attach(Owner::Task(task)));
        settle(&mut gui);
        assert_eq!(gui.app.board.get(task).unwrap().files.len(), 1);
        assert_eq!(gui.app.error, None);
    }

    #[test]
    fn esc_leaves_a_clean_edit_then_closes_the_viewer() {
        let (_dir, mut gui, file) = gui_with_file("one\ntwo\n");
        view_file(&mut gui, file);
        let _ = gui.update(Message::EditFile);

        esc(&mut gui);
        assert!(gui.ui.file_edit.is_none());
        assert_eq!(gui.app.viewing_file, Some(file));

        esc(&mut gui);
        assert_eq!(gui.app.viewing_file, None);
        assert_eq!(gui.app.selected, Some(file.0));
    }

    #[test]
    fn only_cancel_drops_unsaved_changes() {
        let (_dir, mut gui, file) = gui_with_file("one\n");
        view_file(&mut gui, file);
        let _ = gui.update(Message::EditFile);
        let _ = gui.update(Message::EditFileText(text_editor::Action::Edit(
            Edit::Insert('x'),
        )));

        esc(&mut gui);
        let _ = gui.update(Message::CloseViewer);
        assert!(gui.file_edit_dirty());
        assert_eq!(gui.app.viewing_file, Some(file));

        let _ = gui.update(Message::CancelFileEdit);
        assert!(gui.ui.file_edit.is_none());
        assert_eq!(gui.app.viewing_file, Some(file));
    }

    #[test]
    fn esc_closes_the_new_file_popup() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::ToggleNewFile(task));
        assert_eq!(gui.ui.new_file_for, Some(task));
        esc(&mut gui);
        assert_eq!(gui.ui.new_file_for, None);
        assert_eq!(gui.app.selected, Some(task));
    }

    #[test]
    fn epic_menu_closes_on_esc_and_on_choosing() {
        let (dir, mut gui, (task, _)) = gui_with_file("");
        let epic = store(&dir).update(|b| {
            let epic = b.create(taskboard_core::NewTask {
                title: "epic".into(),
                project: b.get(task).unwrap().project.clone(),
                epic: true,
                ..Default::default()
            })?;
            Ok(epic)
        });
        settle(&mut gui);
        let _ = gui.update(Message::ToggleEpicMenu(task));
        esc(&mut gui);
        assert_eq!(gui.ui.epic_open, None);
        assert_eq!(gui.app.selected, Some(task));

        let _ = gui.update(Message::ToggleEpicMenu(task));
        let _ = gui.update(Message::ChooseEpic(task, epic.unwrap()));
        assert_eq!(gui.ui.epic_open, None);
        assert_eq!(gui.app.error, None);
        assert!(gui.app.board.get(task).unwrap().parent.is_some());
    }

    #[test]
    fn epic_menu_closes_when_its_task_is_promoted_elsewhere() {
        let (dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::ToggleEpicMenu(task));
        store(&dir).update(|b| b.promote(task)).unwrap(); // as if `tb promote` ran
        let _ = gui.update(Message::Watch(WatchEvent::Changed));
        settle(&mut gui);
        assert!(gui.app.board.get(task).unwrap().epic);
        assert_eq!(gui.ui.epic_open, None);

        // Demoted again, it doesn't reopen by itself.
        store(&dir).update(|b| b.demote(task)).unwrap();
        let _ = gui.update(Message::Watch(WatchEvent::Changed));
        settle(&mut gui);
        assert_eq!(gui.ui.epic_open, None);
    }

    #[test]
    fn esc_cancels_a_drag() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(200.0, 60.0));
        let _ = gui.update(Message::DragMoved(task, Point::new(10.0, 10.0), bounds));
        esc(&mut gui);
        assert!(gui.ui.drag.as_ref().is_some_and(|d| d.cancelled));
    }

    #[test]
    fn new_file_opens_once_made_and_leaves_edit_mode_once_saved() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::ToggleNewFile(task));
        let _ = gui.update(Message::NewFileName("plan".into()));
        let _ = gui.update(Message::CreateFile(task));
        assert_eq!(gui.ui.new_file_for, Some(task));
        assert_eq!(gui.app.viewing_file, None);
        settle(&mut gui);
        assert_eq!(gui.ui.new_file_for, None);
        let (_, file) = gui.app.viewing_file.unwrap();
        assert!(gui.ui.file_edit.is_some());

        type_x(&mut gui);
        let _ = gui.update(Message::SaveFile);
        assert!(gui.ui.file_edit.is_some());
        settle(&mut gui);
        assert!(gui.ui.file_edit.is_none());
        assert_eq!(gui.ui.file_text.as_deref(), Some("x"));
        let task = gui.app.board.get(task).unwrap();
        let att = task.files.iter().find(|f| f.id == file).unwrap();
        assert_eq!(now(gui.platform.read_file(task.id, att)).unwrap(), b"x");
    }

    #[test]
    fn a_new_file_doesnt_replace_a_file_opened_meanwhile() {
        let (_dir, mut gui, file) = gui_with_file("one");
        let _ = gui.update(Message::ToggleNewFile(file.0));
        let _ = gui.update(Message::NewFileName("plan".into()));
        let _ = gui.update(Message::CreateFile(file.0));
        let _ = gui.update(Message::App(Action::ViewFile(Some(file))));
        settle(&mut gui);
        assert_eq!(gui.app.error, None);
        assert_eq!(gui.app.viewing_file, Some(file));
        assert_eq!(gui.ui.file_text.as_deref(), Some("one"));
        assert!(gui.ui.file_edit.is_none());
        assert_eq!(gui.app.board.get(file.0).unwrap().files.len(), 2);
    }

    #[test]
    fn a_typed_project_is_classified_once_the_platform_answers() {
        let (dir, mut gui, _) = gui_with_file("");
        let _ = gui.update(Message::App(Action::Edit(EditTarget::New(Status::Open))));
        let folder = dir.path().canonicalize().unwrap();
        let _ = gui.update(Message::EditProject(folder.display().to_string()));
        assert!(refresh_of(&gui, &folder).is_some());
        settle(&mut gui);
        let input = gui.app.project_input(&gui.ui.edit_project);
        assert_eq!(input, ProjectInput::New(folder.clone()));

        // Typed on while it's looked for, only the latest path is asked
        // about next, once the first is answered.
        let typed = folder.join("new");
        for text in [folder.join("n"), folder.join("ne"), typed.clone()] {
            let _ = gui.update(Message::EditProject(text.display().to_string()));
        }
        let first = refresh_of(&gui, &folder.join("n")).unwrap();
        assert!(refresh_of(&gui, &folder.join("ne")).is_none());
        assert!(refresh_of(&gui, &typed).is_none());
        let kinds = now(gui.platform.path_kinds(first.paths()));
        let _ = gui.update(Message::Checked(first, kinds));
        assert!(refresh_of(&gui, &typed).is_some());
        run_effects(&mut gui);
        let input = gui.app.project_input(&gui.ui.edit_project);
        assert_eq!(input, ProjectInput::Missing(typed.clone()));

        // A save doesn't wait for path checks.
        let _ = gui.update(Message::EditProject(
            folder.join("other").display().to_string(),
        ));
        let _ = gui.update(Message::EditProject(typed.display().to_string()));
        let _ = gui.update(Message::EditTitle("alpha".into()));
        let _ = gui.update(Message::Save);
        assert!(!gui.refreshing.is_empty());
        assert!(matches!(
            gui.running,
            Some(Effect::Run(Command::Create { .. }))
        ));
        settle(&mut gui);
        assert_eq!(gui.app.error, None);
        assert!(typed.is_dir());
        assert!(gui.app.missing_projects.is_empty());
        assert!(gui.refreshing.is_empty());
        assert_eq!(gui.typed_check, None);
    }

    #[test]
    fn a_typed_folder_is_kept_until_the_project_is_relocated() {
        let (dir, mut gui, (task, _)) = gui_with_file("");
        let old = gui.app.board.get(task).unwrap().project.clone();
        let moved = dir.path().join("moved");
        std::fs::rename(&old, &moved).unwrap();
        let _ = gui.update(Message::CheckProjects);
        settle(&mut gui);
        assert!(gui.app.project_missing(&old));

        let nowhere = dir.path().join("nowhere").display().to_string();
        let _ = gui.update(Message::RelocatePath(nowhere.clone()));
        let _ = gui.update(Message::Relocate(old.clone()));
        settle(&mut gui);
        assert!(gui.app.error.take().unwrap().starts_with("no folder at "));
        assert_eq!(gui.ui.relocate_path, nowhere);

        let _ = gui.update(Message::RelocatePath(moved.display().to_string()));
        let _ = gui.update(Message::Relocate(old));
        settle(&mut gui);
        assert_eq!(gui.app.error, None);
        assert!(gui.ui.relocate_path.is_empty());
        assert!(gui.app.missing_projects.is_empty());
    }

    #[test]
    fn loads_that_finish_out_of_order_are_dropped() {
        let (dir, mut gui, (task, _)) = gui_with_file("");
        let (first, old) = (gui.load_stamp(), now(gui.platform.load_board()));
        store(&dir).update(|b| b.promote(task)).unwrap(); // as if `tb promote` ran
        let (second, new) = (gui.load_stamp(), now(gui.platform.load_board()));
        let _ = gui.update(Message::Loaded(second, new));
        let _ = gui.update(Message::Loaded(first, old));
        assert!(gui.app.board.get(task).unwrap().epic);
    }

    #[test]
    fn a_board_loaded_before_a_command_started_is_dropped() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let (stamp, old) = (gui.load_stamp(), now(gui.platform.load_board()));
        let _ = gui.update(Message::App(Action::Promote(task)));
        let _ = gui.update(Message::Loaded(stamp, old));
        assert!(gui.app.board.get(task).unwrap().epic);
        settle(&mut gui);
        assert!(gui.app.board.get(task).unwrap().epic);
        assert_eq!(gui.app.error, None);
    }
}
