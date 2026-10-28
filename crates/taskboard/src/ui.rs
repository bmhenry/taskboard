//! Builds the Ply element tree each frame from [`App`] state and returns the
//! [`Action`]s the user triggered. All state changes go through those actions.

use std::collections::HashMap;

use ply_engine::ElementBuilder;
use ply_engine::layout::Sizing;
use ply_engine::prelude::*;
use taskboard_core::settings::{DEFAULT_FONT_SIZE, Theme};
use taskboard_core::{
    Attachment, DateField, FileKind, HookEvent, LinkKind, Status, Task, Ulid, hooks,
};

use crate::app::{Action, App, DRAG_THRESHOLD, Drag, EditTarget, View, drop_index, drop_position};
use crate::markdown::{self, BlockKind};

pub static FONT: FontAsset = FontAsset::Bytes {
    file_name: "DejaVuSans.ttf",
    data: include_bytes!("../assets/fonts/DejaVuSans.ttf"),
};
/// Fonts other than the default are only loaded when drawn, and layout
/// measures text with whatever is loaded, so load these up front and keep
/// them from being evicted while unused.
pub async fn ensure_fonts() {
    for font in [&BOLD, &MONO] {
        ply_engine::renderer::FontManager::ensure(font).await;
    }
}

static BOLD: FontAsset = FontAsset::Bytes {
    file_name: "DejaVuSans-Bold.ttf",
    data: include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf"),
};
static MONO: FontAsset = FontAsset::Bytes {
    file_name: "DejaVuSansMono.ttf",
    data: include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
};

/// The colors the UI draws with, one set per [`Theme`].
pub struct Palette {
    /// Window background, behind the panels.
    pub bg: u32,
    panel: u32,
    card: u32,
    card_hover: u32,
    /// Outline of cards and card-colored buttons, so they hold their shape
    /// on a background close to their own color.
    card_edge: u32,
    input: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    danger: u32,
    /// Indexed like [`Status::ALL`].
    status: [u32; 6],
    primary: u32,
    primary_hover: u32,
    danger_button: u32,
    danger_button_hover: u32,
    /// Text on the danger button and the error bar.
    danger_text: u32,
    danger_text_hover: u32,
    error_bar: u32,
}

static DARK: Palette = Palette {
    bg: 0x15171C,
    panel: 0x1C1F26,
    card: 0x252932,
    card_hover: 0x2D323D,
    card_edge: 0x252932,
    input: 0x111318,
    border: 0x343A46,
    text: 0xE6E8EE,
    muted: 0x8F96A3,
    accent: 0x6EA8FE,
    danger: 0xE5534B,
    status: [0x8F96A3, 0x6EA8FE, 0xE3B341, 0xD2A8FF, 0xF778BA, 0x56D364],
    primary: 0x2F5FAE,
    primary_hover: 0x3A71CC,
    danger_button: 0x5A2422,
    danger_button_hover: 0x7A2E2A,
    danger_text: 0xFFD7D4,
    danger_text_hover: 0xFFFFFF,
    error_bar: 0x3A1D1C,
};

/// Neutral grays in the style of Apple's site: a white window, columns only a
/// hair off white, and white cards held apart by thin edges rather than heavy
/// fills. Status colors are deeper than the dark theme's so they read on white.
static LIGHT: Palette = Palette {
    bg: 0xFFFFFF,
    panel: 0xF5F5F7,
    card: 0xFFFFFF,
    card_hover: 0xEDEDF0,
    card_edge: 0xDCDCE1,
    input: 0xFFFFFF,
    border: 0xD2D2D7,
    text: 0x1D1D1F,
    muted: 0x6E6E73,
    accent: 0x0071E3,
    danger: 0xD70015,
    status: [0x6E6E73, 0x0071E3, 0xB25F00, 0x8944AB, 0xC4307A, 0x248A3D],
    primary: 0x0071E3,
    primary_hover: 0x0062C4,
    danger_button: 0xFFE5E5,
    danger_button_hover: 0xFFD0D0,
    danger_text: 0xB0000F,
    danger_text_hover: 0x8A000C,
    error_bar: 0xFFEBEB,
};

pub fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// Unscaled minimum width of the status dropdown's list.
const STATUS_MENU_MIN_WIDTH: f32 = 160.0;

/// Unscaled space between the window's edges and the UI inside it.
const EDGE: u16 = 18;

/// Body text size the hardcoded sizes below are designed around; the user's
/// font size setting scales every size relative to it.
const BASE_FONT_SIZE: f32 = 14.0;

thread_local! {
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static PALETTE: std::cell::Cell<&'static Palette> = const { std::cell::Cell::new(&DARK) };
}

/// This frame's colors, from the theme setting.
fn pal() -> &'static Palette {
    PALETTE.get()
}

/// Scales a font size (or other text metric) designed for the base size.
fn fs(size: u16) -> u16 {
    (size as f32 * SCALE.get()).round().max(1.0) as u16
}

/// Scales a pixel dimension that has to fit text, like a button's height.
fn px(n: f32) -> f32 {
    (n * SCALE.get()).round()
}

fn status_color(status: Status) -> u32 {
    pal().status[status_index(status) as usize]
}

fn status_index(status: Status) -> u32 {
    Status::ALL.iter().position(|s| *s == status).unwrap_or(0) as u32
}

fn card_id(id: Ulid) -> Id {
    ("card", id.0 as u32).into()
}

fn column_id(status: Status) -> Id {
    ("column", status_index(status)).into()
}

fn hook_input_id(index: usize) -> Id {
    ("hook_command", index as u32).into()
}

/// UI-only state that doesn't belong in [`App`].
#[derive(Default)]
pub struct UiState {
    drag: Option<Drag>,
    confirm_delete: Option<Ulid>,
    link_kind: LinkKind,
    /// Decoded images and file contents for the file viewer, by attachment id.
    textures: HashMap<Ulid, Texture2D>,
    file_text: HashMap<Ulid, String>,
    pointer_cursor: bool,
    /// The settings modal shows keybindings instead of settings.
    show_keybindings: bool,
    /// The filter dropdown under the search bar is open.
    filters_open: bool,
    /// The task whose status dropdown in the detail panel is open.
    status_open: Option<Ulid>,
    /// How many hooks' commands were copied into the settings inputs, or
    /// `None` while the inputs aren't shown.
    hook_inputs: Option<usize>,
    /// When a hook command was last typed in, while that edit is unsaved.
    hook_edited_at: Option<f64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Normal,
    Primary,
    Danger,
    Quiet,
}

/// Builds one frame. Returns the actions to apply after rendering.
pub fn frame(ui: &mut Ui, app: &App, state: &mut UiState) -> Vec<Action> {
    let mut actions = Vec::new();
    state.pointer_cursor = false;
    SCALE.set(app.settings.font_size as f32 / BASE_FONT_SIZE);
    PALETTE.set(palette(app.settings.theme));
    if !app.settings_open {
        state.show_keybindings = false;
    }
    if app.editing.is_some() || app.selected != state.status_open {
        state.status_open = None;
    }

    if app.editor_needs_init {
        let (title, desc) = match app.editing {
            Some(EditTarget::Existing(id)) => app
                .board
                .get(id)
                .map(|t| (t.title.clone(), t.description.clone()))
                .unwrap_or_default(),
            _ => Default::default(),
        };
        ui.set_text_value("edit_title", &title);
        ui.set_text_value("edit_desc", &desc);
        ui.set_focus("edit_title");
        actions.push(Action::EditorReady);
    }

    read_filter_inputs(ui, app, &mut actions);
    sync_hook_inputs(ui, app, state, &mut actions);
    close_menus_on_outside_click(ui, state);
    handle_keys(ui, app, state, &mut actions);
    handle_drag(ui, app, state, &mut actions);

    let (w, h) = (screen_width(), screen_height());
    ui.element()
        .width(fixed!(w))
        .height(fixed!(h))
        .background_color(pal().bg)
        .layout(|l| l.direction(TopToBottom))
        .children(|ui| {
            toolbar(ui, app, state, &mut actions);
            if let Some(err) = &app.error {
                error_bar(ui, err, &mut actions);
            }
            ui.element()
                .width(grow!())
                .height(grow!())
                .layout(|l| {
                    l.direction(LeftToRight)
                        .gap(12)
                        .padding((0, fs(EDGE), fs(EDGE), fs(EDGE)))
                })
                .children(|ui| {
                    match app.view {
                        View::Board => board_view(ui, app, state, &mut actions),
                        View::List => list_view(ui, app, state, &mut actions),
                    }
                    if app.editing.is_some() {
                        editor_panel(ui, app, state, &mut actions);
                    } else if let Some(task) = app.selected_task() {
                        detail_panel(ui, task, state, &mut actions);
                    }
                });
            if let Some((task_id, file_id)) = app.viewing_file
                && let Some(att) = app
                    .board
                    .get(task_id)
                    .and_then(|t| t.files.iter().find(|f| f.id == file_id))
            {
                file_viewer(ui, app, att, state, &mut actions, (w, h));
            }
            if app.settings_open {
                settings_modal(ui, app, state, &mut actions, (w, h));
            }
            if let Some(drag) = state.drag.filter(|d| d.active)
                && let Some(task) = app.board.get(drag.id)
            {
                drag_ghost(ui, task);
            }
        });

    set_mouse_cursor(
        if state.pointer_cursor || state.drag.is_some_and(|d| d.active) {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        },
    );
    actions
}

fn read_filter_inputs(ui: &mut Ui, app: &App, actions: &mut Vec<Action>) {
    let search = ui.get_text_value("search");
    if search != app.filter.text {
        actions.push(Action::SetSearch(search.to_string()));
    }
    let after = ui.get_text_value("date_after").to_string();
    let before = ui.get_text_value("date_before").to_string();
    if (&after, &before) != (&app.date_inputs.0, &app.date_inputs.1) {
        actions.push(Action::SetDateBounds { after, before });
    }
}

/// Fills the hook command inputs when the settings page appears or a hook is
/// added or removed (which shifts the inputs), and passes on edits made in
/// them, saving once the user leaves the input or stops typing for a second.
/// Closing settings saves too.
fn sync_hook_inputs(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    if !app.settings_open || state.show_keybindings {
        state.hook_inputs = None;
        return;
    }
    let hooks = &app.settings.hooks;
    if state.hook_inputs != Some(hooks.len()) {
        for (i, hook) in hooks.iter().enumerate() {
            ui.set_text_value(hook_input_id(i), &hook.command);
        }
        state.hook_inputs = Some(hooks.len());
        return;
    }
    for (i, hook) in hooks.iter().enumerate() {
        let command = ui.get_text_value(hook_input_id(i));
        if command != hook.command {
            actions.push(Action::SetHookCommand(i, command.to_string()));
            state.hook_edited_at = Some(get_time());
        }
    }
    let focused = ui.focused_element().map(|f| f.id);
    let typing = (0..hooks.len()).any(|i| Some(hook_input_id(i).id) == focused);
    if let Some(at) = state.hook_edited_at
        && (!typing || get_time() - at > 1.0)
    {
        state.hook_edited_at = None;
        actions.push(Action::SaveSettings);
    }
}

/// Closes the filter and status dropdowns when a press lands outside the
/// dropdown and its button, using last frame's layout.
fn close_menus_on_outside_click(ui: &mut Ui, state: &mut UiState) {
    if !is_mouse_button_pressed(MouseButton::Left) {
        return;
    }
    if state.filters_open && !ui.pointer_over("filter_menu") && !ui.pointer_over("filter_button") {
        close_filters(ui, state);
    }
    if state.status_open.is_some()
        && !ui.pointer_over("status_menu")
        && !ui.pointer_over("status_button")
    {
        state.status_open = None;
    }
}

/// Hides the filter dropdown, dropping focus from its inputs so typing
/// doesn't go to a hidden field.
fn close_filters(ui: &mut Ui, state: &mut UiState) {
    state.filters_open = false;
    let focused = ui.focused_element().map(|f| f.id);
    if [Id::from("date_after"), Id::from("date_before")]
        .iter()
        .any(|i| Some(i.id) == focused)
    {
        ui.clear_focus();
    }
}

fn handle_keys(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let ctrl = is_key_down(KeyCode::LeftControl)
        || is_key_down(KeyCode::RightControl)
        || is_key_down(KeyCode::LeftSuper)
        || is_key_down(KeyCode::RightSuper);

    if is_key_pressed(KeyCode::Escape) {
        state.drag = None;
        if app.settings_open {
            actions.push(Action::ShowSettings(false));
        } else if state.filters_open {
            close_filters(ui, state);
        } else if state.status_open.is_some() {
            state.status_open = None;
        } else if app.viewing_file.is_some() {
            actions.push(Action::ViewFile(None));
        } else if app.editing.is_some() {
            actions.push(Action::CancelEdit);
        } else if ui.focused_element().is_some() {
            ui.clear_focus();
        } else if app.selected.is_some() {
            actions.push(Action::Select(None));
        }
    }
    if ctrl && is_key_pressed(KeyCode::F) {
        ui.set_focus("search");
    }
    if ctrl
        && (is_key_pressed(KeyCode::S) || is_key_pressed(KeyCode::Enter))
        && app.editing.is_some()
    {
        actions.push(save_action(ui));
    }
    if ctrl && is_key_pressed(KeyCode::N) && app.editing.is_none() {
        actions.push(Action::Edit(EditTarget::New(Status::Open)));
    }
    let size = app.settings.font_size;
    if ctrl && (is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd)) {
        actions.push(Action::SetFontSize(size + 1));
    }
    if ctrl && (is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract)) {
        actions.push(Action::SetFontSize(size - 1));
    }
}

/// Shown in the settings modal's keybindings page.
const KEYBINDINGS: &[(&str, &str)] = &[
    ("Ctrl+F", "Focus search"),
    ("Ctrl+N", "New task"),
    ("Ctrl+S / Ctrl+Enter", "Save the task being edited"),
    ("Ctrl+=", "Increase font size"),
    ("Ctrl+-", "Decrease font size"),
    ("Esc", "Close a dialog or menu, cancel editing, or deselect"),
];

fn save_action(ui: &Ui) -> Action {
    Action::Save {
        title: ui.get_text_value("edit_title").to_string(),
        description: ui.get_text_value("edit_desc").to_string(),
    }
}

/// Turns presses on cards into either a click (select) or a drag (move),
/// using last frame's layout to find where a drag was dropped.
fn handle_drag(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let Some(mut drag) = state.drag else { return };
    let (mx, my) = mouse_position();
    if !drag.active
        && ((mx - drag.start.0).powi(2) + (my - drag.start.1).powi(2)).sqrt() > DRAG_THRESHOLD
    {
        drag.active = true;
    }
    state.drag = Some(drag);
    if is_mouse_button_down(MouseButton::Left) {
        return;
    }
    state.drag = None;
    if !drag.active {
        actions.push(Action::Select(Some(drag.id)));
        return;
    }
    let Some(status) = Status::ALL
        .into_iter()
        .find(|s| ui.pointer_over(column_id(*s)))
    else {
        return;
    };
    let visible: Vec<Ulid> = app
        .visible_column(status)
        .iter()
        .map(|t| t.id)
        .filter(|id| *id != drag.id)
        .collect();
    let mids: Vec<f32> = visible
        .iter()
        .filter_map(|id| ui.bounding_box(card_id(*id)))
        .map(|b| b.y + b.height / 2.0)
        .collect();
    // A collapsed column shows no cards, so drops land at its bottom.
    let index = if app.column_collapsed(status) {
        visible.len()
    } else {
        drop_index(&mids, my)
    };
    let position = drop_position(&app.board, status, &visible, index, drag.id);
    actions.push(Action::Move {
        id: drag.id,
        status,
        position: Some(position),
    });
}

fn button(ui: &mut Ui, state: &mut UiState, label: &str, style: Style) -> bool {
    let mut clicked = false;
    ui.element()
        .width(fit!())
        .height(fixed!(px(28.0)))
        .children(|ui| {
            clicked = ui.just_released();
            let hovered = ui.hovered();
            if hovered {
                state.pointer_cursor = true;
            }
            let (bg, fg) = match (style, hovered) {
                (Style::Primary, false) => (pal().primary, 0xFFFFFF),
                (Style::Primary, true) => (pal().primary_hover, 0xFFFFFF),
                (Style::Danger, false) => (pal().danger_button, pal().danger_text),
                (Style::Danger, true) => (pal().danger_button_hover, pal().danger_text_hover),
                (Style::Quiet, false) => (pal().panel, pal().muted),
                (Style::Quiet, true) => (pal().card_hover, pal().text),
                (Style::Normal, false) => (pal().card, pal().text),
                (Style::Normal, true) => (pal().card_hover, pal().text),
            };
            ui.element()
                .width(fit!())
                .height(grow!())
                .background_color(bg)
                .corner_radius(6.0)
                .border(|b| {
                    b.all(1).color(if style == Style::Normal {
                        pal().card_edge
                    } else {
                        bg
                    })
                })
                .layout(|l| l.padding((0, 10, 0, 10)).align(CenterX, CenterY))
                .children(|ui| {
                    ui.text(label, |t| {
                        t.font_size(fs(13)).color(fg).wrap_mode(WrapMode::None)
                    })
                });
        });
    clicked
}

/// A toggleable pill; `on` draws it filled with `color`.
fn chip(ui: &mut Ui, state: &mut UiState, label: &str, color: u32, on: bool) -> bool {
    let mut clicked = false;
    ui.element()
        .width(fit!())
        .height(fixed!(px(26.0)))
        .corner_radius(px(13.0))
        .background_color(if on { color } else { pal().panel })
        .border(|b| b.all(1).color(color))
        .layout(|l| l.padding((0, fs(12), 0, fs(12))).align(CenterX, CenterY))
        .children(|ui| {
            clicked = ui.just_released();
            if ui.hovered() {
                state.pointer_cursor = true;
            }
            ui.text(label, |t| {
                t.font_size(fs(12))
                    .color(if on { pal().bg } else { color })
                    .wrap_mode(WrapMode::None)
            });
        });
    clicked
}

/// Text inputs draw edge to edge and ignore padding, so the frame and padding
/// live on a wrapper element.
fn input_frame(
    ui: &mut Ui,
    width: Sizing,
    height: Sizing,
    invalid: bool,
    input: impl FnOnce(&mut Ui),
) {
    ui.element()
        .width(width)
        .height(height)
        .background_color(pal().input)
        .corner_radius(6.0)
        .border(|b| {
            b.all(1)
                .color(if invalid { pal().danger } else { pal().border })
        })
        .layout(|l| l.padding((7, 8, 7, 8)))
        .children(input);
}

fn text_input(ui: &mut Ui, id: impl Into<Id>, placeholder: &str, width: Sizing, invalid: bool) {
    input_frame(ui, width, fixed!(px(30.0)), invalid, |ui| {
        ui.element()
            .id(id)
            .width(grow!())
            .height(grow!())
            .text_input(|t| {
                t.placeholder(placeholder)
                    .font_size(fs(14))
                    .text_color(pal().text)
                    .placeholder_color(pal().muted)
                    .cursor_color(pal().accent)
            })
            .empty();
    });
}

/// A box that's ticked when `on`, followed by `text`. Returns true when
/// either is clicked.
fn checkbox(ui: &mut Ui, state: &mut UiState, text: &str, on: bool) -> bool {
    let mut clicked = false;
    ui.element()
        .width(fit!())
        .height(fit!())
        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
        .children(|ui| {
            clicked = ui.just_released();
            let hovered = ui.hovered();
            if hovered {
                state.pointer_cursor = true;
            }
            let edge = if on || hovered {
                pal().primary
            } else {
                pal().border
            };
            ui.element()
                .width(fixed!(px(16.0)))
                .height(fixed!(px(16.0)))
                .corner_radius(4.0)
                .background_color(if on { pal().primary } else { pal().input })
                .border(|b| b.all(1).color(edge))
                .layout(|l| l.align(CenterX, CenterY))
                .children(|ui| {
                    if on {
                        ui.text("✓", |t| t.font_size(fs(12)).color(0xFFFFFF));
                    }
                });
            ui.text(text, |t| t.font_size(fs(14)).color(pal().text));
        });
    clicked
}

/// A small "?" that shows `paragraphs` in a bubble below it while hovered.
/// Paragraphs marked `true` are code and drawn in the monospace font.
fn help_bubble(ui: &mut Ui, paragraphs: &[(&str, bool)]) {
    let size = px(18.0);
    ui.element()
        .width(fixed!(size))
        .height(fixed!(size))
        .corner_radius(size / 2.0)
        .border(|b| b.all(1).color(pal().muted))
        .layout(|l| l.align(CenterX, CenterY))
        .children(|ui| {
            let hovered = ui.hovered();
            ui.text("?", |t| t.font_size(fs(11)).font(&BOLD).color(pal().muted));
            if hovered {
                ui.element()
                    .width(fixed!(px(360.0)))
                    .height(fit!())
                    .floating(|f| {
                        f.attach_parent()
                            .z_index(70)
                            .anchor((CenterX, Top), (CenterX, Bottom))
                            .offset((0.0, 6.0))
                    })
                    .background_color(pal().card)
                    .corner_radius(8.0)
                    .border(|b| b.all(1).color(pal().border))
                    .layout(|l| l.direction(TopToBottom).padding(fs(10)).gap(6))
                    .children(|ui| {
                        for &(para, code) in paragraphs {
                            if code {
                                ui.text(para, |t| {
                                    t.font_size(fs(12)).font(&MONO).color(pal().accent)
                                });
                            } else {
                                ui.text(para, |t| t.font_size(fs(13)).color(pal().text));
                            }
                        }
                    });
            }
        });
}

fn label(ui: &mut Ui, text: &str) {
    ui.text(text, |t| t.font_size(fs(12)).color(pal().muted));
}

/// A muted `prefix` followed by `path`, wrapping after any `/` so a long path
/// stays within `width` (Ply only wraps text at whitespace).
fn path_label(ui: &mut Ui, width: f32, prefix: &str, path: &str) {
    // Ply wraps a growing row against its unshrunk width; give it its final width.
    ui.element()
        .width(fixed!(width))
        .height(fit!())
        .layout(|l| l.direction(LeftToRight).wrap())
        .children(|ui| {
            for word in prefix.split_inclusive(' ') {
                label_segment(ui, word);
            }
            label_segment(ui, " ");
            for part in path.split_inclusive('/') {
                label_segment(ui, part);
            }
        });
}

fn label_segment(ui: &mut Ui, text: &str) {
    ui.text(text, |t| {
        t.font_size(fs(12))
            .color(pal().muted)
            .wrap_mode(WrapMode::None)
    });
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| l.padding((8, 0, 2, 0)))
        .children(|ui| {
            ui.text(text, |t| t.font_size(fs(13)).font(&BOLD).color(pal().muted));
        });
}

fn toolbar(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| {
            l.direction(LeftToRight)
                .gap(10)
                .padding((fs(EDGE), fs(EDGE), 6, fs(EDGE)))
                .align(Left, CenterY)
        })
        .children(|ui| {
            ui.text("Taskboard", |t| {
                t.font_size(fs(18)).font(&BOLD).color(pal().text)
            });
            // Last frame's width of the path's slot, which grows into whatever
            // the buttons leave, so it doesn't depend on the path drawn in it.
            let room = ui
                .bounding_box("project_path")
                .map(|b| (b.width / (fs(12) as f32 * PATH_CHAR_WIDTH)) as usize)
                .unwrap_or(usize::MAX);
            ui.element()
                .id("project_path")
                .width(grow!())
                .height(fit!())
                .overflow(|o| o.clip_x())
                .children(|ui| {
                    let root = app
                        .store
                        .dir()
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let root = shorten_path(&root, room.min(MAX_PATH_CHARS));
                    ui.text(&root, |t| {
                        t.font_size(fs(12))
                            .color(pal().muted)
                            .wrap_mode(WrapMode::None)
                    });
                });
            for (view, name) in [(View::Board, "Board"), (View::List, "List")] {
                let style = if app.view == view {
                    Style::Primary
                } else {
                    Style::Normal
                };
                if button(ui, state, name, style) {
                    actions.push(Action::SetView(view));
                }
            }
            if button(ui, state, "+ New task", Style::Normal) {
                actions.push(Action::Edit(EditTarget::New(Status::Open)));
            }
            if button(ui, state, "⚙ Settings", Style::Quiet) {
                actions.push(Action::ShowSettings(true));
            }
        });
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| {
            l.direction(LeftToRight)
                .gap(8)
                .wrap()
                .wrap_gap(8)
                .padding((6, fs(EDGE), 12, fs(EDGE)))
                .align(Left, CenterY)
        })
        .children(|ui| {
            text_input(
                ui,
                "search",
                "Search title, description, links  (Ctrl+F)",
                grow!(px(110.0), px(360.0)),
                false,
            );

            filter_button(ui, app, state, actions);

            if !app.filter.is_empty() && button(ui, state, "Clear", Style::Quiet) {
                ui.set_text_value("search", "");
                ui.set_text_value("date_after", "");
                ui.set_text_value("date_before", "");
                actions.push(Action::ClearFilters);
            }
        });
}

/// The longest project path the toolbar shows before collapsing it.
const MAX_PATH_CHARS: usize = 30;
/// A generous average glyph width for paths, as a fraction of the font size.
const PATH_CHAR_WIDTH: f32 = 0.65;

/// Collapses `path` to `…` plus as many trailing components (with their
/// leading separator) as fit in `max_chars`. The last component is always
/// kept, even if it alone is longer.
fn shorten_path(path: &str, max_chars: usize) -> String {
    const SEPARATORS: [char; 2] = ['/', '\\'];
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    let path = path.trim_end_matches(SEPARATORS);
    let mut seps = path.rmatch_indices(SEPARATORS).map(|(i, _)| i);
    let Some(mut start) = seps.next() else {
        return path.to_string();
    };
    for sep in seps {
        if path[sep..].chars().count() + 1 > max_chars {
            break;
        }
        start = sep;
    }
    format!("…{}", &path[start..])
}

/// The toolbar's filter button, with the status and date filters in a
/// dropdown below it.
fn filter_button(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let f = &app.filter;
    let active = f.statuses.len() + f.after.is_some() as usize + f.before.is_some() as usize;
    let label = if active > 0 {
        format!("Filter ({active}) ▾")
    } else {
        "Filter ▾".to_string()
    };
    let style = if active > 0 {
        Style::Primary
    } else {
        Style::Normal
    };
    ui.element()
        .id("filter_button")
        .width(fit!())
        .height(fit!())
        .children(|ui| {
            if button(ui, state, &label, style) {
                if state.filters_open {
                    close_filters(ui, state);
                } else {
                    state.filters_open = true;
                }
            }
            if state.filters_open {
                filter_menu(ui, app, state, actions);
            }
        });
}

/// A panel floating just below its parent (or above it, if `up`), moved
/// `shift` px sideways.
fn dropdown<'a>(ui: &'a mut Ui, id: &'static str, shift: f32, up: bool) -> ElementBuilder<'a> {
    ui.element()
        .id(id)
        .floating(|f| {
            let f = f.attach_parent().z_index(40);
            if up {
                f.anchor((Left, Bottom), (Left, Top)).offset((shift, -4.0))
            } else {
                f.anchor((Left, Top), (Left, Bottom)).offset((shift, 4.0))
            }
        })
        .background_color(pal().panel)
        .corner_radius(8.0)
        .border(|b| b.all(1).color(pal().border))
}

fn filter_menu(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let (width, pad) = (px(330.0), fs(14));
    // Shift left if hanging off the button's left edge would leave the window.
    let shift = ui
        .bounding_box("filter_button")
        .map(|b| (screen_width() - fs(EDGE) as f32 - (b.x + width)).min(0.0))
        .unwrap_or(0.0);
    dropdown(ui, "filter_menu", shift, false)
        .width(fixed!(width))
        .height(fit!())
        .layout(|l| l.direction(TopToBottom).padding(pad).gap(8))
        .children(|ui| {
            label(ui, "Status");
            // Ply wraps a growing row against its unshrunk width, so the
            // chips would overflow the menu; give the row its final width.
            ui.element()
                .width(fixed!(width - 2.0 * pad as f32))
                .height(fit!())
                .layout(|l| l.direction(LeftToRight).gap(8).wrap().wrap_gap(8))
                .children(|ui| {
                    for status in Status::ALL {
                        let on = app.filter.statuses.contains(&status);
                        if chip(ui, state, status.label(), status_color(status), on) {
                            actions.push(Action::ToggleStatusFilter(status));
                        }
                    }
                });

            ui.element()
                .width(grow!())
                .height(fit!())
                .layout(|l| {
                    l.direction(LeftToRight)
                        .gap(8)
                        .padding((6, 0, 0, 0))
                        .align(Left, CenterY)
                })
                .children(|ui| {
                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                        label(ui, "Date");
                    });
                    let (field_label, next) = match app.filter.date_field {
                        DateField::Created => ("Created ▾", DateField::Updated),
                        DateField::Updated => ("Updated ▾", DateField::Closed),
                        DateField::Closed => ("Closed ▾", DateField::Created),
                    };
                    if button(ui, state, field_label, Style::Quiet) {
                        actions.push(Action::SetDateField(next));
                    }
                });
            let bad = |s: &str| !s.trim().is_empty() && taskboard_core::parse_date(s).is_err();
            ui.element()
                .width(grow!())
                .height(fit!())
                .layout(|l| l.direction(LeftToRight).gap(8))
                .children(|ui| {
                    text_input(
                        ui,
                        "date_after",
                        "from YYYY-MM-DD",
                        grow!(),
                        bad(&app.date_inputs.0),
                    );
                    text_input(
                        ui,
                        "date_before",
                        "to YYYY-MM-DD",
                        grow!(),
                        bad(&app.date_inputs.1),
                    );
                });
        });
}

fn error_bar(ui: &mut Ui, err: &str, actions: &mut Vec<Action>) {
    ui.element()
        .width(grow!())
        .height(fit!())
        .background_color(pal().error_bar)
        .layout(|l| {
            l.direction(LeftToRight)
                .padding((8, 12, 8, 12))
                .gap(12)
                .align(Left, CenterY)
        })
        .children(|ui| {
            ui.element().width(grow!()).height(fit!()).children(|ui| {
                ui.text(err, |t| t.font_size(fs(13)).color(pal().danger_text));
            });
            ui.element().width(fit!()).height(fit!()).children(|ui| {
                if ui.just_released() {
                    actions.push(Action::DismissError);
                }
                ui.text("Dismiss", |t| t.font_size(fs(13)).color(pal().text));
            });
        });
}

/// Ply sends the wheel only to the innermost scroll container under the
/// pointer, so over a column's cards a sideways scroll (shift+wheel or a
/// horizontal wheel) would be lost on the vertical-only card list. Pass it
/// on to the board, using the same speed and shift mapping as Ply.
fn forward_board_scroll(ui: &mut Ui, app: &App) {
    // Modals cover the board, so the wheel belongs to them.
    if app.settings_open || app.viewing_file.is_some() {
        return;
    }
    let over_cards = Status::ALL
        .into_iter()
        .any(|s| ui.pointer_over(("cards", status_index(s))));
    if !over_cards {
        return;
    }
    // Scrolling a board that fits would only flash its scrollbar.
    let Some(board) = ui
        .scroll_container_data("board")
        .filter(|b| b.content_dimensions.width > b.scroll_container_dimensions.width)
    else {
        return;
    };
    let (wheel_x, wheel_y) = mouse_wheel();
    let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
    let dx = if shift { wheel_x + wheel_y } else { wheel_x } * 20.0;
    if dx != 0.0 {
        let pos = board.scroll_position;
        ui.set_scroll_position("board", (-(pos.x + dx), -pos.y));
    }
}

fn board_view(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    forward_board_scroll(ui, app);
    ui.element()
        .id("board")
        .width(grow!())
        .height(grow!())
        .overflow(|o| {
            o.scroll_x()
                .no_drag_scroll()
                .scrollbar(|s| s.width(6.0).thumb_color(pal().border).hide_after_frames(90))
        })
        .layout(|l| l.direction(LeftToRight).gap(10))
        .children(|ui| {
            for status in Status::ALL {
                if app.column_collapsed(status) {
                    collapsed_column(ui, app, state, actions, status);
                } else {
                    column(ui, app, state, actions, status);
                }
            }
        });
}

/// A collapsed column drawn as a thin strip with its title along it like a
/// book spine. Clicking expands it; it still accepts dropped cards.
fn collapsed_column(
    ui: &mut Ui,
    app: &App,
    state: &mut UiState,
    actions: &mut Vec<Action>,
    status: Status,
) {
    let count = app.visible_column(status).len();
    let dragging = state.drag.is_some_and(|d| d.active);
    let drop_target = dragging && ui.pointer_over(column_id(status));
    // The spine is laid out horizontally, then rotated about its center into
    // place along the top of the strip.
    let (strip_w, spine_len) = (px(34.0), px(180.0));
    // Ply draws a rotated element into a texture with the screen's clip still
    // applied, which wipes it out, so the spine floats free of the board's
    // clip instead. Left unclipped it would draw over the side panel or the
    // window edge once the board scrolls, so show it only while the whole
    // strip is on the visible board (as of last frame's layout).
    let spine_shown = match (ui.bounding_box(column_id(status)), ui.bounding_box("board")) {
        (Some(strip), Some(board)) => {
            strip.x >= board.x - 0.5 && strip.x + strip.width <= board.x + board.width + 0.5
        }
        _ => true,
    };
    ui.element()
        .id(column_id(status))
        .width(fixed!(strip_w))
        .height(grow!())
        .background_color(pal().panel)
        .corner_radius(8.0)
        .border(|b| {
            b.all(if drop_target { 2 } else { 1 })
                .color(if drop_target {
                    pal().accent
                } else {
                    pal().panel
                })
        })
        .children(|ui| {
            if ui.hovered() {
                state.pointer_cursor = true;
            }
            if ui.just_released() && !dragging {
                actions.push(Action::ToggleColumn(status));
            }
            if !spine_shown {
                return;
            }
            ui.element()
                .width(fixed!(spine_len))
                .height(fixed!(strip_w))
                .floating(|f| {
                    f.attach_parent()
                        .anchor((CenterX, CenterY), (CenterX, Top))
                        .offset((0.0, px(8.0) + spine_len / 2.0))
                        .passthrough()
                })
                .rotate_visual(|r| r.degrees(90.0))
                .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                .children(|ui| {
                    ui.element()
                        .width(fixed!(8.0))
                        .height(fixed!(8.0))
                        .corner_radius(4.0)
                        .background_color(status_color(status))
                        .empty();
                    ui.text(status.label(), |t| {
                        t.font_size(fs(14))
                            .font(&BOLD)
                            .color(pal().text)
                            .wrap_mode(WrapMode::None)
                    });
                    count_badge(ui, count);
                });
        });
}

fn column(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>, status: Status) {
    let tasks = app.visible_column(status);
    let dragging = state.drag.filter(|d| d.active);
    let drop_target = dragging.is_some() && ui.pointer_over(column_id(status));
    ui.element()
        .id(column_id(status))
        .width(grow!(px(210.0)))
        .height(grow!())
        .background_color(pal().panel)
        .corner_radius(8.0)
        .border(|b| {
            b.all(if drop_target { 2 } else { 1 })
                .color(if drop_target {
                    pal().accent
                } else {
                    pal().panel
                })
        })
        .layout(|l| l.direction(TopToBottom).padding(8).gap(8))
        .children(|ui| {
            // The whole header collapses the column, like clicking its strip
            // expands it; « just makes that visible. A click on a button also
            // counts as a click on the header, so + has to opt out.
            ui.element()
                .width(grow!())
                .height(fixed!(px(28.0)))
                .layout(|l| {
                    l.direction(LeftToRight)
                        .gap(8)
                        .align(Left, CenterY)
                        .padding((0, 0, 0, 4))
                })
                .children(|ui| {
                    if ui.hovered() {
                        state.pointer_cursor = true;
                    }
                    let header_clicked = ui.just_released();
                    ui.element()
                        .width(fixed!(8.0))
                        .height(fixed!(8.0))
                        .corner_radius(4.0)
                        .background_color(status_color(status))
                        .empty();
                    ui.text(status.label(), |t| {
                        t.font_size(fs(14))
                            .font(&BOLD)
                            .color(pal().text)
                            .wrap_mode(WrapMode::None)
                    });
                    count_badge(ui, tasks.len());
                    ui.element().width(grow!()).height(fixed!(1.0)).empty();
                    let collapse = button(ui, state, "«", Style::Quiet);
                    let add = button(ui, state, "+", Style::Quiet);
                    if add {
                        actions.push(Action::Edit(EditTarget::New(status)));
                    } else if collapse || header_clicked {
                        actions.push(Action::ToggleColumn(status));
                    }
                });
            ui.element()
                .id(("cards", status_index(status)))
                .width(grow!())
                .height(grow!())
                .overflow(|o| {
                    o.scroll_y()
                        .no_drag_scroll()
                        .scrollbar(|s| s.width(4.0).thumb_color(pal().border).hide_after_frames(90))
                })
                .layout(|l| l.direction(TopToBottom).gap(8))
                .children(|ui| {
                    for task in &tasks {
                        card(ui, app, state, task);
                    }
                });
        });
}

/// A column's task count, in a small pill next to its title.
fn count_badge(ui: &mut Ui, count: usize) {
    ui.element()
        .width(fit!())
        .height(fixed!(px(18.0)))
        .corner_radius(px(9.0))
        .background_color(pal().card)
        .layout(|l| l.padding((0, fs(7), 0, fs(7))).align(CenterX, CenterY))
        .children(|ui| {
            ui.text(&count.to_string(), |t| {
                t.font_size(fs(12))
                    .color(pal().muted)
                    .wrap_mode(WrapMode::None)
            });
        });
}

fn card(ui: &mut Ui, app: &App, state: &mut UiState, task: &Task) {
    let selected = app.selected == Some(task.id);
    let dragged = state.drag.is_some_and(|d| d.active && d.id == task.id);
    ui.element()
        .id(card_id(task.id))
        .width(grow!())
        .height(fit!())
        .children(|ui| {
            // Ply only tests a card against its nearest clip, the column's
            // card list, whose box runs on past the board's edge and under
            // the side panel. Check the board too, or hidden cards there
            // would light up and take clicks meant for the panel.
            let hovered = ui.hovered() && ui.pointer_over("board");
            if hovered && ui.just_pressed() {
                state.drag = Some(Drag {
                    id: task.id,
                    start: mouse_position(),
                    active: false,
                });
            }
            if hovered {
                state.pointer_cursor = true;
            }
            ui.element()
                .width(grow!())
                .height(fit!())
                .background_color(if dragged {
                    pal().panel
                } else if hovered {
                    pal().card_hover
                } else {
                    pal().card
                })
                .corner_radius(6.0)
                .border(|b| {
                    b.all(1).color(if selected {
                        pal().accent
                    } else if dragged {
                        pal().border
                    } else {
                        pal().card_edge
                    })
                })
                .layout(|l| l.direction(TopToBottom).padding(10).gap(6))
                .children(|ui| card_body(ui, task, dragged));
        });
}

fn card_body(ui: &mut Ui, task: &Task, faded: bool) {
    ui.text(&task.title, |t| {
        t.font_size(fs(14))
            .color(if faded { pal().muted } else { pal().text })
    });
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| l.direction(LeftToRight).gap(10))
        .children(|ui| {
            ui.text(&task.short_id(), |t| {
                t.font_size(fs(11))
                    .font(&MONO)
                    .color(pal().muted)
                    .wrap_mode(WrapMode::None)
            });
            if !task.links.is_empty() {
                ui.text(&plural(task.links.len(), "link"), |t| {
                    t.font_size(fs(11))
                        .color(pal().muted)
                        .wrap_mode(WrapMode::None)
                });
            }
            if !task.files.is_empty() {
                ui.text(&plural(task.files.len(), "file"), |t| {
                    t.font_size(fs(11))
                        .color(pal().muted)
                        .wrap_mode(WrapMode::None)
                });
            }
        });
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn drag_ghost(ui: &mut Ui, task: &Task) {
    let (mx, my) = mouse_position();
    ui.element()
        .width(fixed!(px(240.0)))
        .height(fit!())
        .floating(|f| {
            f.attach_root()
                .offset((mx + 12.0, my + 8.0))
                .z_index(100)
                .passthrough()
        })
        .background_color(pal().card_hover)
        .corner_radius(6.0)
        .border(|b| b.all(1).color(pal().accent))
        .layout(|l| l.direction(TopToBottom).padding(10).gap(6))
        .children(|ui| card_body(ui, task, false));
}

fn list_view(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let tasks = app.visible_list();
    ui.element()
        .width(grow!())
        .height(grow!())
        .background_color(pal().panel)
        .corner_radius(8.0)
        .layout(|l| l.direction(TopToBottom).padding(8))
        .children(|ui| {
            list_row(ui, None, ["Status", "ID", "Title", "Updated"], false);
            ui.element()
                .id("list")
                .width(grow!())
                .height(grow!())
                .overflow(|o| {
                    o.scroll_y()
                        .scrollbar(|s| s.width(4.0).thumb_color(pal().border).hide_after_frames(90))
                })
                .layout(|l| l.direction(TopToBottom))
                .children(|ui| {
                    if tasks.is_empty() {
                        ui.element()
                            .width(grow!())
                            .height(fit!())
                            .layout(|l| l.padding(16))
                            .children(|ui| {
                                ui.text("No tasks match.", |t| {
                                    t.font_size(fs(14)).color(pal().muted)
                                });
                            });
                    }
                    for task in tasks {
                        let updated = task
                            .updated
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string();
                        let selected = app.selected == Some(task.id);
                        let short = task.short_id();
                        let clicked = list_row(
                            ui,
                            Some((task.status, state)),
                            [task.status.label(), &short, &task.title, &updated],
                            selected,
                        );
                        if clicked {
                            actions.push(Action::Select(Some(task.id)));
                        }
                    }
                });
        });
}

/// One row of the list table; `status` is `None` for the header.
fn list_row(
    ui: &mut Ui,
    status: Option<(Status, &mut UiState)>,
    cells: [&str; 4],
    selected: bool,
) -> bool {
    let header = status.is_none();
    let mut clicked = false;
    ui.element().width(grow!()).height(fit!()).children(|ui| {
        let hovered = !header && ui.hovered();
        clicked = !header && ui.just_released();
        let color = match &status {
            Some((s, _)) => status_color(*s),
            None => pal().muted,
        };
        if let Some((_, state)) = status {
            state.pointer_cursor |= hovered;
        }
        ui.element()
            .width(grow!())
            .height(fit!())
            .background_color(if selected {
                pal().card_hover
            } else if hovered {
                pal().card
            } else {
                pal().panel
            })
            .corner_radius(4.0)
            .layout(|l| {
                l.direction(LeftToRight)
                    .padding((8, 10, 8, 10))
                    .gap(12)
                    .align(Left, CenterY)
            })
            .children(|ui| {
                let size = fs(if header { 12 } else { 14 });
                ui.element()
                    .width(fixed!(px(120.0)))
                    .height(fit!())
                    .children(|ui| {
                        ui.text(cells[0], |t| t.font_size(size).color(color));
                    });
                ui.element()
                    .width(fixed!(px(90.0)))
                    .height(fit!())
                    .children(|ui| {
                        ui.text(cells[1], |t| {
                            t.font_size(size).font(&MONO).color(pal().muted)
                        });
                    });
                ui.element().width(grow!()).height(fit!()).children(|ui| {
                    ui.text(cells[2], |t| {
                        t.font_size(size)
                            .color(if header { pal().muted } else { pal().text })
                    });
                });
                ui.element()
                    .width(fixed!(px(140.0)))
                    .height(fit!())
                    .children(|ui| {
                        ui.text(cells[3], |t| t.font_size(size).color(pal().muted));
                    });
            });
    });
    clicked
}

fn side_panel(ui: &mut Ui, id: &'static str, body: impl FnOnce(&mut Ui)) {
    ui.element()
        .id(id)
        .width(fixed!(
            (screen_width() * 0.42)
                .clamp(px(340.0), px(480.0))
                .min(screen_width() * 0.6)
        ))
        .height(grow!())
        .background_color(pal().panel)
        .corner_radius(8.0)
        .overflow(|o| {
            o.scroll_y()
                .scrollbar(|s| s.width(4.0).thumb_color(pal().border).hide_after_frames(90))
        })
        .layout(|l| l.direction(TopToBottom).padding(16).gap(10))
        .children(body);
}

fn detail_panel(ui: &mut Ui, task: &Task, state: &mut UiState, actions: &mut Vec<Action>) {
    side_panel(ui, "detail", |ui| {
        ui.element()
            .width(grow!())
            .height(fit!())
            .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
            .children(|ui| {
                status_dot(ui, task.status);
                ui.text(&task.short_id(), |t| {
                    t.font_size(fs(12)).font(&MONO).color(pal().muted)
                });
                ui.element().width(grow!()).height(fixed!(1.0)).empty();
                if button(ui, state, "Edit", Style::Normal) {
                    actions.push(Action::Edit(EditTarget::Existing(task.id)));
                }
                if state.confirm_delete == Some(task.id) {
                    if button(ui, state, "Really delete?", Style::Danger) {
                        state.confirm_delete = None;
                        actions.push(Action::Delete(task.id));
                    }
                } else if button(ui, state, "Delete", Style::Quiet) {
                    state.confirm_delete = Some(task.id);
                }
                if button(ui, state, "×", Style::Quiet) {
                    actions.push(Action::Select(None));
                }
            });

        ui.text(&task.title, |t| {
            t.font_size(fs(20)).font(&BOLD).color(pal().text)
        });

        status_dropdown(ui, task, state, actions);

        let fmt = |d: chrono::DateTime<chrono::Utc>| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        };
        let mut stamps = format!(
            "Created {}   ·   Updated {}",
            fmt(task.created),
            fmt(task.updated)
        );
        if let Some(closed) = task.closed {
            stamps.push_str(&format!("   ·   Closed {}", fmt(closed)));
        }
        label(ui, &stamps);

        section_title(ui, "Description");
        if task.description.trim().is_empty() {
            label(ui, "No description.");
        } else {
            render_markdown(ui, &task.description);
        }

        section_title(ui, "Links");
        for (i, link) in task.links.iter().enumerate() {
            ui.element()
                .width(grow!())
                .height(fit!())
                .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                .children(|ui| {
                    let kind_color = if link.kind == LinkKind::Source {
                        status_color(Status::InProgress)
                    } else {
                        pal().muted
                    };
                    ui.text(&link.kind.to_string(), |t| {
                        t.font_size(fs(11)).color(kind_color)
                    });
                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                        let hovered = ui.hovered();
                        state.pointer_cursor |= hovered;
                        if ui.just_released() {
                            actions.push(Action::OpenUrl(link.url.clone()));
                        }
                        ui.text(link.display_text(), |t| {
                            t.font_size(fs(13)).color(if hovered {
                                pal().text
                            } else {
                                pal().accent
                            })
                        });
                    });
                    if button(ui, state, "×", Style::Quiet) {
                        actions.push(Action::RemoveLink {
                            id: task.id,
                            index: i,
                        });
                    }
                });
        }
        ui.element()
            .width(grow!())
            .height(fit!())
            .layout(|l| l.direction(LeftToRight).gap(6).align(Left, CenterY))
            .children(|ui| {
                text_input(ui, "link_url", "https://…", grow!(), false);
                let kind = state.link_kind;
                if button(ui, state, &kind.to_string(), Style::Quiet) {
                    state.link_kind = if kind == LinkKind::Source {
                        LinkKind::Context
                    } else {
                        LinkKind::Source
                    };
                }
                if button(ui, state, "Add", Style::Normal) {
                    let url = ui.get_text_value("link_url").to_string();
                    ui.set_text_value("link_url", "");
                    actions.push(Action::AddLink {
                        id: task.id,
                        url,
                        kind,
                    });
                }
            });

        section_title(ui, "Files");
        for att in &task.files {
            ui.element()
                .width(grow!())
                .height(fit!())
                .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                .children(|ui| {
                    let kind = match att.kind {
                        FileKind::Image => "image",
                        FileKind::Markdown => "md",
                        FileKind::Text => "text",
                    };
                    ui.text(kind, |t| t.font_size(fs(11)).color(pal().muted));
                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                        let hovered = ui.hovered();
                        state.pointer_cursor |= hovered;
                        if ui.just_released() {
                            actions.push(Action::ViewFile(Some((task.id, att.id))));
                        }
                        ui.text(&att.name, |t| {
                            t.font_size(fs(13)).color(if hovered {
                                pal().text
                            } else {
                                pal().accent
                            })
                        });
                    });
                    if button(ui, state, "×", Style::Quiet) {
                        actions.push(Action::Detach {
                            id: task.id,
                            file: att.id,
                        });
                    }
                });
        }
        ui.element()
            .width(grow!())
            .height(fit!())
            .layout(|l| l.direction(LeftToRight).gap(6).align(Left, CenterY))
            .children(|ui| {
                text_input(
                    ui,
                    "attach_path",
                    "Path to a file to attach",
                    grow!(),
                    false,
                );
                if button(ui, state, "Attach", Style::Normal) {
                    let path = ui.get_text_value("attach_path").to_string();
                    if !path.trim().is_empty() {
                        ui.set_text_value("attach_path", "");
                        actions.push(Action::Attach { id: task.id, path });
                    }
                }
            });
    });
}

fn status_dot(ui: &mut Ui, status: Status) {
    ui.element()
        .width(fixed!(8.0))
        .height(fixed!(8.0))
        .corner_radius(4.0)
        .background_color(status_color(status))
        .empty();
}

/// Shows the task's status; clicking it opens a list of statuses to move the
/// task to.
fn status_dropdown(ui: &mut Ui, task: &Task, state: &mut UiState, actions: &mut Vec<Action>) {
    ui.element()
        .id("status_button")
        .width(fit!())
        .height(fit!())
        .children(|ui| {
            ui.element()
                .width(fit!())
                .height(fixed!(px(28.0)))
                .children(|ui| {
                    if ui.just_released() {
                        state.status_open = match state.status_open {
                            Some(_) => None,
                            None => Some(task.id),
                        };
                    }
                    let hovered = ui.hovered();
                    state.pointer_cursor |= hovered;
                    ui.element()
                        .width(fit!())
                        .height(grow!())
                        .background_color(if hovered {
                            pal().card_hover
                        } else {
                            pal().card
                        })
                        .corner_radius(6.0)
                        .border(|b| b.all(1).color(status_color(task.status)))
                        .layout(|l| {
                            l.direction(LeftToRight)
                                .gap(8)
                                .padding((0, fs(12), 0, fs(12)))
                                .align(Left, CenterY)
                        })
                        .children(|ui| {
                            status_dot(ui, task.status);
                            ui.text(task.status.label(), |t| {
                                t.font_size(fs(13))
                                    .color(status_color(task.status))
                                    .wrap_mode(WrapMode::None)
                            });
                            ui.text("▾", |t| t.font_size(fs(13)).color(pal().muted));
                        });
                });
            if state.status_open == Some(task.id) {
                status_menu(ui, task, state, actions);
            }
        });
}

fn status_menu(ui: &mut Ui, task: &Task, state: &mut UiState, actions: &mut Vec<Action>) {
    // Open upward if the list would run off the bottom of the window and
    // there's more room above.
    let height = Status::ALL.len() as f32 * (px(28.0) + 2.0) + 14.0;
    let up = ui.bounding_box("status_button").is_some_and(|b| {
        let below = screen_height() - (b.y + b.height);
        below < height + 8.0 && b.y > below
    });
    dropdown(ui, "status_menu", 0.0, up)
        .width(fit!(px(STATUS_MENU_MIN_WIDTH)))
        .height(fit!())
        .layout(|l| l.direction(TopToBottom).padding(6).gap(2))
        .children(|ui| {
            for status in Status::ALL {
                ui.element()
                    .width(grow!())
                    .height(fixed!(px(28.0)))
                    .children(|ui| {
                        if ui.just_released() {
                            state.status_open = None;
                            if task.status != status {
                                actions.push(Action::Move {
                                    id: task.id,
                                    status,
                                    position: None,
                                });
                            }
                        }
                        let hovered = ui.hovered();
                        state.pointer_cursor |= hovered;
                        let bg = if hovered {
                            pal().card_hover
                        } else if task.status == status {
                            pal().card
                        } else {
                            pal().panel
                        };
                        ui.element()
                            .width(grow!())
                            .height(grow!())
                            .background_color(bg)
                            .corner_radius(4.0)
                            .layout(|l| {
                                l.direction(LeftToRight)
                                    .gap(8)
                                    .padding((0, fs(10), 0, fs(10)))
                                    .align(Left, CenterY)
                            })
                            .children(|ui| {
                                status_dot(ui, status);
                                ui.text(status.label(), |t| {
                                    t.font_size(fs(13))
                                        .color(status_color(status))
                                        .wrap_mode(WrapMode::None)
                                });
                            });
                    });
            }
        });
}

fn editor_panel(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let heading = match app.editing {
        Some(EditTarget::New(status)) => format!("New task in {status}"),
        _ => "Edit task".to_string(),
    };
    side_panel(ui, "editor", |ui| {
        ui.text(&heading, |t| {
            t.font_size(fs(18)).font(&BOLD).color(pal().text)
        });
        label(ui, "Title");
        text_input(ui, "edit_title", "What needs doing?", grow!(), false);
        label(ui, "Description (Markdown)");
        input_frame(ui, grow!(), grow!(px(200.0)), false, |ui| {
            ui.element()
                .id("edit_desc")
                .width(grow!())
                .height(grow!())
                .text_input(|t| {
                    t.multiline()
                        .drag_select()
                        .font(&MONO)
                        .font_size(fs(13))
                        .text_color(pal().text)
                        .placeholder("Details, acceptance criteria, notes…")
                        .placeholder_color(pal().muted)
                        .cursor_color(pal().accent)
                })
                .empty();
        });
        ui.element()
            .width(grow!())
            .height(fit!())
            .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
            .children(|ui| {
                if button(ui, state, "Save", Style::Primary) {
                    actions.push(save_action(ui));
                }
                if button(ui, state, "Cancel", Style::Quiet) {
                    actions.push(Action::CancelEdit);
                }
                label(ui, "Ctrl+S to save · Esc to cancel");
            });
    });
}

fn render_markdown(ui: &mut Ui, src: &str) {
    for block in markdown::parse(src) {
        match block.kind {
            BlockKind::Heading(level) => {
                let size = fs(match level {
                    1 => 20,
                    2 => 17,
                    _ => 15,
                });
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .layout(|l| l.padding((4, 0, 0, 0)))
                    .children(|ui| {
                        ui.text(&block.text, |t| {
                            t.font_size(size).font(&BOLD).color(pal().text)
                        });
                    });
            }
            BlockKind::Paragraph => ui.text(&block.text, |t| {
                t.font_size(fs(14)).line_height(fs(20)).color(pal().text)
            }),
            BlockKind::ListItem { depth, marker } => {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .layout(|l| {
                        l.direction(LeftToRight).gap(6).padding((
                            0,
                            0,
                            0,
                            4 + fs(16) * depth as u16,
                        ))
                    })
                    .children(|ui| {
                        ui.element()
                            .width(fixed!(px(18.0)))
                            .height(fit!())
                            .children(|ui| {
                                ui.text(&marker, |t| t.font_size(fs(14)).color(pal().muted));
                            });
                        ui.element().width(grow!()).height(fit!()).children(|ui| {
                            ui.text(&block.text, |t| {
                                t.font_size(fs(14)).line_height(fs(20)).color(pal().text)
                            });
                        });
                    });
            }
            BlockKind::Code => {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .background_color(pal().input)
                    .corner_radius(6.0)
                    .layout(|l| l.padding(10))
                    .children(|ui| {
                        ui.text(&block.text, |t| {
                            t.font_size(fs(13))
                                .font(&MONO)
                                .line_height(fs(18))
                                .color(pal().text)
                        });
                    });
            }
            BlockKind::Quote => {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .border(|b| b.left(3).color(pal().border))
                    .layout(|l| l.padding((2, 0, 2, 12)))
                    .children(|ui| {
                        ui.text(&block.text, |t| {
                            t.font_size(fs(14)).line_height(fs(20)).color(pal().muted)
                        })
                    });
            }
            BlockKind::Rule => {
                ui.element()
                    .width(grow!())
                    .height(fixed!(1.0))
                    .background_color(pal().border)
                    .empty();
            }
        }
    }
}

fn file_viewer(
    ui: &mut Ui,
    app: &App,
    att: &Attachment,
    state: &mut UiState,
    actions: &mut Vec<Action>,
    (w, h): (f32, f32),
) {
    let path = app.store.file_path(att);
    // Scrim covering the window; the dialog sits centered inside it.
    ui.element()
        .width(fixed!(w))
        .height(fixed!(h))
        .floating(|f| f.attach_root().z_index(50))
        .background_color((0.0, 0.0, 0.0, 0.6))
        .layout(|l| l.align(CenterX, CenterY))
        .children(|ui| {
            ui.element()
                .width(fixed!((w * 0.8).min(1100.0)))
                .height(fixed!(h * 0.85))
                .background_color(pal().panel)
                .corner_radius(10.0)
                .border(|b| b.all(1).color(pal().border))
                .layout(|l| l.direction(TopToBottom).padding(16).gap(12))
                .children(|ui| {
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            ui.text(&att.name, |t| {
                                t.font_size(fs(16)).font(&BOLD).color(pal().text)
                            });
                            ui.element().width(grow!()).height(fixed!(1.0)).empty();
                            if button(ui, state, "Close", Style::Normal) {
                                actions.push(Action::ViewFile(None));
                            }
                        });
                    ui.element()
                        .id("file_body")
                        .width(grow!())
                        .height(grow!())
                        .overflow(|o| {
                            o.scroll()
                                .scrollbar(|s| s.width(6.0).thumb_color(pal().border))
                        })
                        .layout(|l| l.direction(TopToBottom).gap(8))
                        .children(|ui| match att.kind {
                            FileKind::Image => {
                                let tex = state.textures.entry(att.id).or_insert_with(|| {
                                    std::fs::read(&path)
                                        .ok()
                                        .map(|bytes| Texture2D::from_file_with_format(&bytes, None))
                                        .unwrap_or_else(Texture2D::empty)
                                });
                                if tex.width() > 0.0 {
                                    let tex = tex.clone();
                                    let ratio = tex.width() / tex.height();
                                    ui.element()
                                        .width(grow!(0.0, tex.width()))
                                        .height(fit!())
                                        .aspect_ratio(ratio)
                                        .image(tex)
                                        .empty();
                                } else {
                                    label(ui, "Couldn't load this image.");
                                }
                            }
                            kind => {
                                let text = state.file_text.entry(att.id).or_insert_with(|| {
                                    std::fs::read(&path)
                                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                                        .unwrap_or_else(|e| format!("Couldn't read file: {e}"))
                                });
                                if kind == FileKind::Markdown {
                                    render_markdown(ui, text);
                                } else {
                                    ui.text(text, |t| {
                                        t.font_size(fs(13))
                                            .font(&MONO)
                                            .line_height(fs(18))
                                            .color(pal().text)
                                            .wrap_mode(WrapMode::Newline)
                                    });
                                }
                            }
                        });
                });
        });
}

fn settings_modal(
    ui: &mut Ui,
    app: &App,
    state: &mut UiState,
    actions: &mut Vec<Action>,
    (w, h): (f32, f32),
) {
    let size = app.settings.font_size;
    let (dialog_w, pad) = (px(460.0).min(w * 0.9), 16);
    // Scrim covering the window; the dialog sits centered inside it.
    ui.element()
        .width(fixed!(w))
        .height(fixed!(h))
        .floating(|f| f.attach_root().z_index(60))
        .background_color((0.0, 0.0, 0.0, 0.6))
        .layout(|l| l.align(CenterX, CenterY))
        .children(|ui| {
            ui.element()
                .width(fixed!(dialog_w))
                .height(fit!(0.0, h * 0.85))
                .background_color(pal().panel)
                .corner_radius(10.0)
                .border(|b| b.all(1).color(pal().border))
                .overflow(|o| o.scroll_y())
                .layout(|l| l.direction(TopToBottom).padding(pad).gap(12))
                .children(|ui| {
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            let title = if state.show_keybindings {
                                "Keybindings"
                            } else {
                                "Settings"
                            };
                            ui.text(title, |t| t.font_size(fs(16)).font(&BOLD).color(pal().text));
                            ui.element().width(grow!()).height(fixed!(1.0)).empty();
                            let toggle = if state.show_keybindings {
                                "⚙ Settings"
                            } else {
                                "⌨ Keybindings"
                            };
                            if button(ui, state, toggle, Style::Quiet) {
                                state.show_keybindings = !state.show_keybindings;
                            }
                            if button(ui, state, "Close", Style::Normal) {
                                actions.push(Action::ShowSettings(false));
                            }
                        });
                    if state.show_keybindings {
                        for (keys, what) in KEYBINDINGS {
                            ui.element()
                                .width(grow!())
                                .height(fit!())
                                .layout(|l| l.direction(LeftToRight).gap(12).align(Left, CenterY))
                                .children(|ui| {
                                    ui.element()
                                        .width(fixed!(px(160.0)))
                                        .height(fit!())
                                        .children(|ui| {
                                            ui.text(keys, |t| {
                                                t.font_size(fs(13)).font(&MONO).color(pal().accent)
                                            });
                                        });
                                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                                        ui.text(what, |t| t.font_size(fs(14)).color(pal().text));
                                    });
                                });
                        }
                        return;
                    }
                    section_title(ui, "Global");
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            ui.element().width(grow!()).height(fit!()).children(|ui| {
                                ui.text("Theme", |t| t.font_size(fs(14)).color(pal().text));
                            });
                            for (theme, name) in [(Theme::Dark, "Dark"), (Theme::Light, "Light")] {
                                let style = if app.settings.theme == theme {
                                    Style::Primary
                                } else {
                                    Style::Normal
                                };
                                if button(ui, state, name, style) {
                                    actions.push(Action::SetTheme(theme));
                                }
                            }
                        });
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            ui.element().width(grow!()).height(fit!()).children(|ui| {
                                ui.text("Font size", |t| t.font_size(fs(14)).color(pal().text));
                            });
                            if button(ui, state, "−", Style::Normal) {
                                actions.push(Action::SetFontSize(size - 1));
                            }
                            ui.element()
                                .width(fixed!(px(32.0)))
                                .height(fit!())
                                .layout(|l| l.align(CenterX, CenterY))
                                .children(|ui| {
                                    ui.text(&size.to_string(), |t| {
                                        t.font_size(fs(14)).font(&MONO).color(pal().text)
                                    });
                                });
                            if button(ui, state, "+", Style::Normal) {
                                actions.push(Action::SetFontSize(size + 1));
                            }
                            if size != DEFAULT_FONT_SIZE && button(ui, state, "Reset", Style::Quiet)
                            {
                                actions.push(Action::SetFontSize(DEFAULT_FONT_SIZE));
                            }
                        });
                    label(ui, "Ctrl+= / Ctrl+- also change the font size.");
                    let collapse = app.settings.collapse_empty_columns;
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            ui.element().width(grow!()).height(fit!()).children(|ui| {
                                ui.text("Collapse empty columns", |t| {
                                    t.font_size(fs(14)).color(pal().text)
                                });
                            });
                            let (text, style) = if collapse {
                                ("On", Style::Primary)
                            } else {
                                ("Off", Style::Normal)
                            };
                            if button(ui, state, text, style) {
                                actions.push(Action::SetCollapseEmptyColumns(!collapse));
                            }
                        });
                    label(
                        ui,
                        "Empty columns start as strips. Click a column's header or « to collapse it; click a strip to expand it.",
                    );
                    hooks_settings(ui, app, state, actions);
                    match &app.settings_path {
                        Some(p) => path_label(
                            ui,
                            dialog_w - 2.0 * pad as f32,
                            "Saved to",
                            &p.display().to_string(),
                        ),
                        None => label(ui, "Not saved: neither XDG_CONFIG_HOME nor HOME is set."),
                    }
                });
        });
}

const PASS_ID_HELP: &[(&str, bool)] = &[
    (
        "When ticked, the task's full ID is added as the last argument, so a hook set to",
        false,
    ),
    ("notify-send Moved", true),
    ("runs as", false),
    ("notify-send Moved 01M6197HP62B5BKZRPNK3SCY8K", true),
    (
        "A script gets the ID as $1 (with bash -c '…' it's $0). Look the task up with tb show <ID>.",
        false,
    ),
];

fn hooks_settings(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    section_title(ui, "Hooks");
    label(
        ui,
        "Commands to run when a task is created or moved to another column, here or with tb. They run in the project folder without a shell, so for pipes or redirects use bash -c '…'.",
    );
    for (i, hook) in app.settings.hooks.iter().enumerate() {
        ui.element()
            .width(grow!())
            .height(fit!())
            .background_color(pal().card)
            .corner_radius(8.0)
            .border(|b| b.all(1).color(pal().card_edge))
            .layout(|l| l.direction(TopToBottom).padding(10).gap(8))
            .children(|ui| {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                    .children(|ui| {
                        ui.text("When a task is", |t| t.font_size(fs(14)).color(pal().text));
                        for (event, name) in
                            [(HookEvent::Created, "Created"), (HookEvent::Moved, "Moved")]
                        {
                            let style = if hook.event == event {
                                Style::Primary
                            } else {
                                Style::Normal
                            };
                            if button(ui, state, name, style) {
                                actions.push(Action::SetHookEvent(i, event));
                            }
                        }
                        ui.element().width(grow!()).height(fixed!(1.0)).empty();
                        if button(ui, state, "Remove", Style::Quiet) {
                            actions.push(Action::RemoveHook(i));
                        }
                    });
                text_input(
                    ui,
                    hook_input_id(i),
                    "Command, e.g. /path/to/script or bash -c '…'",
                    grow!(),
                    hooks::split_command(&hook.command).is_err(),
                );
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                    .children(|ui| {
                        if checkbox(ui, state, "Pass the task ID", hook.pass_task_id) {
                            actions.push(Action::SetHookPassTaskId(i, !hook.pass_task_id));
                        }
                        help_bubble(ui, PASS_ID_HELP);
                    });
            });
    }
    if button(ui, state, "+ Add hook", Style::Normal) {
        actions.push(Action::AddHook);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_are_left_alone() {
        assert_eq!(
            shorten_path("/home/me/dev/taskboard", 30),
            "/home/me/dev/taskboard"
        );
    }

    #[test]
    fn long_paths_keep_the_trailing_components_that_fit() {
        let path = "/home/someone/projects/clients/acme/taskboard";
        assert_eq!(shorten_path(path, 30), "…/clients/acme/taskboard");
        assert_eq!(shorten_path(path, 20), "…/acme/taskboard");
        assert!(shorten_path(path, 30).chars().count() <= 30);
    }

    #[test]
    fn the_last_component_is_kept_even_if_too_long() {
        assert_eq!(
            shorten_path("/srv/a-very-long-project-name", 10),
            "…/a-very-long-project-name"
        );
        assert_eq!(shorten_path("/srv/project/", 5), "…/project");
        assert_eq!(
            shorten_path("a-very-long-project-name", 10),
            "a-very-long-project-name"
        );
    }

    #[test]
    fn windows_paths_collapse_at_backslashes() {
        assert_eq!(
            shorten_path(r"C:\Users\someone\projects\clients\taskboard", 22),
            r"…\clients\taskboard"
        );
    }
}
