//! The Iced front end: turns [`App`] state into widgets, and the user's input
//! into [`Action`]s. All changes to the board go through those actions.

mod board;
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
use theme::{Pal, palette};
use widgets::El;

/// Body text size the hardcoded sizes are designed around; the user's font
/// size setting scales the whole UI relative to it.
const BASE_FONT_SIZE: f32 = 14.0;

/// Space between the window's edges and the UI inside it.
const EDGE: f32 = 18.0;

/// How long after the last keystroke in a hook command it's saved.
const HOOK_SAVE_DELAY: Duration = Duration::from_secs(1);

const EDIT_TITLE: &str = "edit_title";
const SEARCH: &str = "search";

/// Shown in the settings modal's keybindings page.
const KEYBINDINGS: &[(&str, &str)] = &[
    ("Ctrl+F", "Focus search"),
    ("Ctrl+N", "New task"),
    ("Ctrl+S / Ctrl+Enter", "Save the task being edited"),
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
    /// The open file viewer's text, read when it opened.
    file_text: Option<String>,
    /// The settings modal shows keybindings instead of settings.
    show_keybindings: bool,
    /// The filter dropdown under the search bar is open.
    filters_open: bool,
    /// The task whose status dropdown in the detail panel is open.
    status_open: Option<Ulid>,
    edit_title: String,
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

/// A card being dragged.
struct Drag {
    id: Ulid,
    /// Where the columns were when the drag started, to highlight the one
    /// under the pointer.
    columns: Vec<(Status, Rectangle)>,
    over: Option<Status>,
    /// Esc was pressed; the drop does nothing.
    cancelled: bool,
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
    EditTitle(String),
    EditDesc(text_editor::Action),
    Save,
    HookCommand(usize, String),
    ToggleKeybindings,
    Key(Key, Modifiers, event::Status),
    Watch(WatchEvent),
    Tick,
    HoverCard(Ulid, bool),
    DragMoved(Ulid, Point),
    DragColumns(Vec<(iced::widget::Id, Rectangle)>),
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
        ];
        if self.ui.hook_edited_at.is_some() {
            subs.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let was_editing = self.app.editing.is_some();
        let task = self.handle(message);
        let (app, ui) = (&self.app, &mut self.ui);
        if !app.settings_open {
            ui.show_keybindings = false;
            ui.hook_edited_at = None;
        }
        if app.editing.is_some() || app.selected != ui.status_open {
            ui.status_open = None;
        }
        match app.viewing_file {
            None => ui.file_text = None,
            Some((task, file)) if ui.file_text.is_none() => {
                let att = app
                    .board
                    .get(task)
                    .and_then(|t| t.files.iter().find(|f| f.id == file));
                if let Some(att) = att.filter(|a| a.kind != FileKind::Image) {
                    ui.file_text = Some(viewer::read_text(&app.store.file_path(att)));
                }
            }
            Some(_) => {}
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
            Message::EditTitle(s) => ui.edit_title = s,
            Message::EditDesc(action) => ui.edit_desc.perform(action),
            Message::Save => {
                if self.app.editing.is_some() {
                    self.app.apply(Action::Save {
                        title: ui.edit_title.clone(),
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
                self.app.error = Some(format!("Live updates are off; couldn't watch .tb/: {e}"));
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
            Message::DragMoved(id, point) => {
                if ui.drag.as_ref().is_none_or(|d| d.id != id) {
                    ui.drag = Some(Drag {
                        id,
                        columns: Vec::new(),
                        over: None,
                        cancelled: false,
                    });
                    return zones::find(board::column_ids()).map(Message::DragColumns);
                }
                if let Some(drag) = &mut ui.drag
                    && !drag.cancelled
                {
                    drag.over = column_at(&drag.columns, point);
                }
            }
            Message::DragColumns(zones) => {
                if let Some(drag) = &mut ui.drag {
                    drag.columns = board::columns_from(&zones);
                }
            }
            Message::Dropped(id, point) => {
                let cancelled = ui.drag.take().is_none_or(|d| d.cancelled);
                if !cancelled {
                    return zones::find(board::column_ids())
                        .map(move |zones| Message::DropColumns(id, point, zones));
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
        let (title, desc) = match target {
            EditTarget::Existing(id) => self
                .app
                .board
                .get(id)
                .map(|t| (t.title.clone(), t.description.clone()))
                .unwrap_or_default(),
            EditTarget::New(_) => Default::default(),
        };
        let ui = &mut self.ui;
        ui.edit_title = title;
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
                    drag.cancelled = true;
                    drag.over = None;
                }
                if app.settings_open {
                    return self.apply(Action::ShowSettings(false));
                } else if ui.filters_open {
                    ui.filters_open = false;
                } else if ui.status_open.is_some() {
                    ui.status_open = None;
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
            layers = layers.push(modal(dialog, Message::App(Action::ViewFile(None))));
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

fn column_at(columns: &[(Status, Rectangle)], point: Point) -> Option<Status> {
    columns
        .iter()
        .find(|(_, bounds)| bounds.contains(point))
        .map(|(status, _)| *status)
}
