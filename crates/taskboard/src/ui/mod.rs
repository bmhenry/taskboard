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
mod viewer;
mod widgets;
mod zones;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{column, container, mouse_area, opaque, row, stack, text_editor};
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Subscription, Task, event};
use taskboard_core::{FileKind, LinkKind, Status, Ulid};

use crate::app::{Action, App, EditTarget, Owner, View, drop_index, drop_position};
use crate::watch::{self, WatchEvent};
use drag::{Drag, column_at, stacked_mids};
use theme::{Pal, palette};
use widgets::El;

/// Body text size the hardcoded sizes are designed around; the user's font
/// size setting scales the whole UI relative to it.
const BASE_FONT_SIZE: f32 = 14.0;

/// Space between the window's edges and the UI inside it.
const EDGE: f32 = 18.0;

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
    ToggleStatusMenu(Ulid),
    CloseStatusMenu,
    ConfirmDelete(Ulid),
    SetLinkKind(Owner, LinkKind),
    LinkUrl(String),
    AttachPath(String),
    AddLink(Owner, LinkKind),
    Attach(Owner),
    /// Opens the native file dialog to pick a file to attach.
    Browse(Owner),
    /// The file dialog closed; `None` if it was cancelled.
    Picked(Owner, Option<PathBuf>),
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
    Watch(WatchEvent),
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
    pub fn new(app: App) -> Gui {
        Gui {
            app,
            ui: UiState::default(),
        }
    }

    pub fn title(&self) -> String {
        "Taskboard".to_string()
    }

    pub fn theme(&self) -> iced::Theme {
        theme::iced_theme(self.app.settings.theme)
    }

    pub fn scale_factor(&self) -> f32 {
        self.app.settings.font_size as f32 / BASE_FONT_SIZE
    }

    fn pal(&self) -> Pal {
        palette(self.app.settings.theme)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let store = &self.app.store;
        let keys = event::listen_with(|event, status, _| match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::Key(key, modifiers, status))
            }
            _ => None,
        });
        let mut subs = vec![
            watch::subscription(store.dir(), &store.tasks_path()).map(Message::Watch),
            keys,
            iced::time::every(PROJECT_CHECK_INTERVAL).map(|_| Message::CheckProjects),
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
        if app.editing.is_none() {
            ui.project_menu_open = false;
        }
        if app.selected.is_none_or(|id| ui.new_file_for != Some(id)) || app.editing.is_some() {
            ui.new_file_for = None;
        }
        if ui.new_file_for.is_none() {
            ui.new_file_name.clear();
        }
        // The text is read once per file shown, so reloads (like the
        // watcher's after a save) don't disturb the viewer or an edit in
        // progress.
        if app.viewing_file != ui.file_shown {
            ui.file_shown = app.viewing_file;
            ui.file_text = None;
            ui.file_editable = false;
            ui.file_edit = None;
            let att = app.viewing_file.and_then(|(task, file)| {
                let task = app.board.get(task)?;
                task.files.iter().find(|f| f.id == file)
            });
            if let Some(att) = att.filter(|a| a.kind != FileKind::Image) {
                let (text, editable) = viewer::read_text(&app.store.file_path(att));
                ui.file_text = Some(text);
                ui.file_editable = editable;
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
        task
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
            Message::ToggleStatusMenu(id) => {
                ui.status_open = match ui.status_open {
                    Some(_) => None,
                    None => Some(id),
                }
            }
            Message::CloseStatusMenu => ui.status_open = None,
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
                if !ui.attach_path.trim().is_empty() {
                    let path = typed_path(&std::mem::take(&mut ui.attach_path));
                    self.app.apply(Action::Attach { owner, path });
                }
            }
            Message::Browse(owner) => {
                return Task::perform(pick_file(), move |path| Message::Picked(owner, path));
            }
            Message::Picked(owner, Some(path)) => self.app.apply(Action::Attach { owner, path }),
            Message::Picked(_, None) => {}
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
                self.app.apply(Action::NewFile { task, name });
                // Kept open with the typed name to fix if creating failed.
                if self.app.error.is_none() {
                    // The new file is empty; it opens ready to edit.
                    let ui = &mut self.ui;
                    ui.new_file_for = None;
                    ui.file_shown = self.app.viewing_file;
                    ui.file_text = Some(String::new());
                    ui.file_editable = true;
                    ui.file_edit = Some(text_editor::Content::new());
                    return iced::widget::operation::focus(FILE_EDITOR);
                }
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
                    self.app.apply(Action::WriteFile {
                        task,
                        file,
                        text: text.clone(),
                    });
                    if self.app.error.is_none() {
                        self.ui.file_text = Some(text);
                        self.ui.file_edit = None;
                    }
                }
            }
            Message::CancelFileEdit => ui.file_edit = None,
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
                return Task::perform(pick_folder(title), Message::PickedProject);
            }
            Message::PickedProject(Some(path)) => ui.edit_project = self.app.project_text(&path),
            Message::PickedProject(None) => {}
            Message::EditDesc(action) => ui.edit_desc.perform(action),
            Message::RelocatePath(s) => ui.relocate_path = s,
            Message::Relocate(from) => {
                let to = ui.relocate_path.clone();
                self.app.apply(Action::RelocateProject {
                    from: from.clone(),
                    to,
                });
                // Keep the typed folder to fix if relocating failed.
                if self.app.board.project(&from).is_none() {
                    self.ui.relocate_path.clear();
                }
            }
            Message::BrowseRelocate(from) => {
                let title = "Choose the project's new folder";
                return Task::perform(pick_folder(title), move |to| {
                    Message::PickedRelocate(from.clone(), to)
                });
            }
            Message::PickedRelocate(from, Some(to)) => {
                let to = to.display().to_string();
                self.app.apply(Action::RelocateProject { from, to });
            }
            Message::PickedRelocate(_, None) => {}
            Message::ConfirmDeleteProject(path) => ui.confirm_delete_project = Some(path),
            Message::CheckProjects => self.app.check_projects(),
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
            Message::Watch(WatchEvent::Changed) => self.app.reload(),
            Message::Watch(WatchEvent::Failed(e)) => {
                self.app.error = Some(format!(
                    "Live updates are off; couldn't watch {}: {e}",
                    self.app.store.dir().display()
                ));
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
                        if self.app.column_collapsed(status) {
                            continue;
                        }
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
                return self.apply(Action::SetFontSize(app.settings.font_size + 1));
            }
            Key::Character("-" | "_") if ctrl => {
                return self.apply(Action::SetFontSize(app.settings.font_size - 1));
            }
            _ => {}
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        iced::widget::responsive(move |size| self.view_sized(size)).into()
    }

    fn view_sized(&self, size: Size) -> El<'_> {
        let (app, p) = (&self.app, self.pal());
        let mut page = column![toolbar::toolbar(self, size.width)];
        if let Some(err) = &app.error {
            page = page.push(toolbar::error_bar(p, err));
        }
        let main: El = match app.view {
            View::Board => board::board(self, size.width),
            View::List => list::list(self),
            View::InProgress => list::in_progress(self),
        };
        let mut body = row![main].spacing(12).height(Length::Fill);
        // The In Progress view shows the editor and details itself.
        if app.view != View::InProgress {
            let width = side_width(size.width);
            if app.editing.is_some() {
                body = body.push(panels::editor(self, Length::Fixed(width)));
            } else if let Some(task) = app.selected_task() {
                body = body.push(panels::detail(self, task, Length::Fixed(width)));
            }
        }
        page = page
            .push(container(body).padding(iced::Padding::ZERO.right(EDGE).bottom(EDGE).left(EDGE)));
        let page = container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| theme::fill(p.bg, 0.0));

        let mut layers = stack![page];
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

/// Asks for a file with the system's file dialog, without blocking the UI.
/// rfd gives `None` both when cancelled and when there's no portal (or
/// zenity) to show a dialog, so the latter fails silently.
async fn pick_file() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Attach a file")
        .pick_file()
        .await
        .map(|f| f.path().to_path_buf())
}

/// Asks for a directory with the system's folder dialog; see [`pick_file`].
async fn pick_folder(title: &'static str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|f| f.path().to_path_buf())
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

    /// A GUI with one task, `(task, file)`, whose attached text file holds
    /// `text`, not yet open in the viewer.
    fn gui_with_file(text: &str) -> (tempfile::TempDir, Gui, (Ulid, Ulid)) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Store::open(&dir.path().join("data")).unwrap(), None);
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
        (dir, Gui::new(app), (task, file))
    }

    fn esc(gui: &mut Gui) {
        let _ = gui.update(Message::Key(
            Key::Named(Named::Escape),
            Modifiers::default(),
            event::Status::Ignored,
        ));
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
            let _ = gui.update(Message::App(Action::ViewFile(Some(file))));
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
    fn esc_leaves_a_clean_edit_then_closes_the_viewer() {
        let (_dir, mut gui, file) = gui_with_file("one\ntwo\n");
        let _ = gui.update(Message::App(Action::ViewFile(Some(file))));
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
        let _ = gui.update(Message::App(Action::ViewFile(Some(file))));
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
    fn esc_closes_the_new_file_popup_and_cancels_a_drag() {
        let (_dir, mut gui, (task, _)) = gui_with_file("");
        let _ = gui.update(Message::ToggleNewFile(task));
        assert_eq!(gui.ui.new_file_for, Some(task));
        esc(&mut gui);
        assert_eq!(gui.ui.new_file_for, None);
        assert_eq!(gui.app.selected, Some(task));

        let bounds = Rectangle::new(Point::ORIGIN, Size::new(200.0, 60.0));
        let _ = gui.update(Message::DragMoved(task, Point::new(10.0, 10.0), bounds));
        esc(&mut gui);
        assert!(gui.ui.drag.as_ref().is_some_and(|d| d.cancelled));
    }
}
