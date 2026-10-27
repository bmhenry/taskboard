//! Builds the Ply element tree each frame from [`App`] state and returns the
//! [`Action`]s the user triggered. All state changes go through those actions.

use std::collections::HashMap;

use ply_engine::layout::Sizing;
use ply_engine::prelude::*;
use taskboard_core::settings::DEFAULT_FONT_SIZE;
use taskboard_core::{Attachment, DateField, FileKind, LinkKind, Status, Task, Ulid};

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

pub const BG: u32 = 0x15171C;
const PANEL: u32 = 0x1C1F26;
const CARD: u32 = 0x252932;
const CARD_HOVER: u32 = 0x2D323D;
const INPUT: u32 = 0x111318;
const BORDER: u32 = 0x343A46;
const TEXT: u32 = 0xE6E8EE;
const MUTED: u32 = 0x8F96A3;
const ACCENT: u32 = 0x6EA8FE;
const DANGER: u32 = 0xE5534B;

/// Body text size the hardcoded sizes below are designed around; the user's
/// font size setting scales every size relative to it.
const BASE_FONT_SIZE: f32 = 14.0;

thread_local! {
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
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
    match status {
        Status::Open => 0x8F96A3,
        Status::Backlog => 0x6EA8FE,
        Status::InProgress => 0xE3B341,
        Status::NeedsReview => 0xD2A8FF,
        Status::InReview => 0xF778BA,
        Status::Closed => 0x56D364,
    }
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
    if !app.settings_open {
        state.show_keybindings = false;
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
    close_filters_on_outside_click(ui, state);
    handle_keys(ui, app, state, &mut actions);
    handle_drag(ui, app, state, &mut actions);

    let (w, h) = (screen_width(), screen_height());
    ui.element()
        .width(fixed!(w))
        .height(fixed!(h))
        .background_color(BG)
        .layout(|l| l.direction(TopToBottom))
        .children(|ui| {
            toolbar(ui, app, state, &mut actions);
            if let Some(err) = &app.error {
                error_bar(ui, err, &mut actions);
            }
            ui.element()
                .width(grow!())
                .height(grow!())
                .layout(|l| l.direction(LeftToRight).gap(12).padding((0, 12, 12, 12)))
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

/// Closes the filter dropdown when a press lands outside it and its button,
/// using last frame's layout.
fn close_filters_on_outside_click(ui: &mut Ui, state: &mut UiState) {
    if state.filters_open
        && is_mouse_button_pressed(MouseButton::Left)
        && !ui.pointer_over("filter_menu")
        && !ui.pointer_over("filter_button")
    {
        close_filters(ui, state);
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
    ("Esc", "Close a dialog, cancel editing, or deselect"),
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
                (Style::Primary, false) => (0x2F5FAE, 0xFFFFFF),
                (Style::Primary, true) => (0x3A71CC, 0xFFFFFF),
                (Style::Danger, false) => (0x5A2422, 0xFFD7D4),
                (Style::Danger, true) => (0x7A2E2A, 0xFFFFFF),
                (Style::Quiet, false) => (PANEL, MUTED),
                (Style::Quiet, true) => (CARD_HOVER, TEXT),
                (Style::Normal, false) => (CARD, TEXT),
                (Style::Normal, true) => (CARD_HOVER, TEXT),
            };
            ui.element()
                .width(fit!())
                .height(grow!())
                .background_color(bg)
                .corner_radius(6.0)
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
        .height(fixed!(px(24.0)))
        .corner_radius(px(12.0))
        .background_color(if on { color } else { PANEL })
        .border(|b| b.all(1).color(color))
        .layout(|l| l.padding((0, 10, 0, 10)).align(CenterX, CenterY))
        .children(|ui| {
            clicked = ui.just_released();
            if ui.hovered() {
                state.pointer_cursor = true;
            }
            ui.text(label, |t| {
                t.font_size(fs(12))
                    .color(if on { BG } else { color })
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
        .background_color(INPUT)
        .corner_radius(6.0)
        .border(|b| b.all(1).color(if invalid { DANGER } else { BORDER }))
        .layout(|l| l.padding((7, 8, 7, 8)))
        .children(input);
}

fn text_input(ui: &mut Ui, id: &'static str, placeholder: &str, width: Sizing, invalid: bool) {
    input_frame(ui, width, fixed!(px(30.0)), invalid, |ui| {
        ui.element()
            .id(id)
            .width(grow!())
            .height(grow!())
            .text_input(|t| {
                t.placeholder(placeholder)
                    .font_size(fs(14))
                    .text_color(TEXT)
                    .placeholder_color(MUTED)
                    .cursor_color(ACCENT)
            })
            .empty();
    });
}

fn label(ui: &mut Ui, text: &str) {
    ui.text(text, |t| t.font_size(fs(12)).color(MUTED));
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| l.padding((8, 0, 2, 0)))
        .children(|ui| {
            ui.text(text, |t| t.font_size(fs(13)).font(&BOLD).color(MUTED));
        });
}

fn toolbar(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| {
            l.direction(LeftToRight)
                .gap(10)
                .padding((12, 12, 6, 12))
                .align(Left, CenterY)
        })
        .children(|ui| {
            ui.text("Taskboard", |t| t.font_size(fs(18)).font(&BOLD).color(TEXT));
            ui.element()
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
                    ui.text(&root, |t| {
                        t.font_size(fs(12)).color(MUTED).wrap_mode(WrapMode::None)
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
                .padding((6, 12, 12, 12))
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

fn filter_menu(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let width = px(330.0);
    // Shift left if hanging off the button's left edge would leave the window.
    let shift = ui
        .bounding_box("filter_button")
        .map(|b| (screen_width() - 12.0 - (b.x + width)).min(0.0))
        .unwrap_or(0.0);
    ui.element()
        .id("filter_menu")
        .width(fixed!(width))
        .height(fit!())
        .floating(|f| {
            f.attach_parent()
                .anchor((Left, Top), (Left, Bottom))
                .offset((shift, 4.0))
                .z_index(40)
        })
        .background_color(PANEL)
        .corner_radius(8.0)
        .border(|b| b.all(1).color(BORDER))
        .layout(|l| l.direction(TopToBottom).padding(12).gap(8))
        .children(|ui| {
            label(ui, "Status");
            ui.element()
                .width(grow!())
                .height(fit!())
                .layout(|l| l.direction(LeftToRight).gap(6).wrap().wrap_gap(6))
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
        .background_color(0x3A1D1C)
        .layout(|l| {
            l.direction(LeftToRight)
                .padding((8, 12, 8, 12))
                .gap(12)
                .align(Left, CenterY)
        })
        .children(|ui| {
            ui.element().width(grow!()).height(fit!()).children(|ui| {
                ui.text(err, |t| t.font_size(fs(13)).color(0xFFD7D4));
            });
            ui.element().width(fit!()).height(fit!()).children(|ui| {
                if ui.just_released() {
                    actions.push(Action::DismissError);
                }
                ui.text("Dismiss", |t| t.font_size(fs(13)).color(TEXT));
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
                .scrollbar(|s| s.width(6.0).thumb_color(BORDER).hide_after_frames(90))
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
    ui.element()
        .id(column_id(status))
        .width(fixed!(strip_w))
        .height(grow!())
        .background_color(PANEL)
        .corner_radius(8.0)
        .border(|b| {
            b.all(if drop_target { 2 } else { 1 })
                .color(if drop_target { ACCENT } else { PANEL })
        })
        .children(|ui| {
            if ui.hovered() {
                state.pointer_cursor = true;
            }
            if ui.just_released() && !dragging {
                actions.push(Action::ToggleColumn(status));
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
                            .color(TEXT)
                            .wrap_mode(WrapMode::None)
                    });
                    ui.text(&count.to_string(), |t| t.font_size(fs(12)).color(MUTED));
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
        .background_color(PANEL)
        .corner_radius(8.0)
        .border(|b| {
            b.all(if drop_target { 2 } else { 1 })
                .color(if drop_target { ACCENT } else { PANEL })
        })
        .layout(|l| l.direction(TopToBottom).padding(8).gap(8))
        .children(|ui| {
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
                    ui.element()
                        .width(fixed!(8.0))
                        .height(fixed!(8.0))
                        .corner_radius(4.0)
                        .background_color(status_color(status))
                        .empty();
                    ui.text(status.label(), |t| {
                        t.font_size(fs(14))
                            .font(&BOLD)
                            .color(TEXT)
                            .wrap_mode(WrapMode::None)
                    });
                    ui.text(&tasks.len().to_string(), |t| {
                        t.font_size(fs(12)).color(MUTED)
                    });
                    ui.element().width(grow!()).height(fixed!(1.0)).empty();
                    if button(ui, state, "«", Style::Quiet) {
                        actions.push(Action::ToggleColumn(status));
                    }
                    if button(ui, state, "+", Style::Quiet) {
                        actions.push(Action::Edit(EditTarget::New(status)));
                    }
                });
            ui.element()
                .id(("cards", status_index(status)))
                .width(grow!())
                .height(grow!())
                .overflow(|o| {
                    o.scroll_y()
                        .no_drag_scroll()
                        .scrollbar(|s| s.width(4.0).thumb_color(BORDER).hide_after_frames(90))
                })
                .layout(|l| l.direction(TopToBottom).gap(8))
                .children(|ui| {
                    for task in &tasks {
                        card(ui, app, state, task);
                    }
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
            let hovered = ui.hovered();
            if ui.just_pressed() {
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
                    PANEL
                } else if hovered {
                    CARD_HOVER
                } else {
                    CARD
                })
                .corner_radius(6.0)
                .border(|b| {
                    b.all(1).color(if selected {
                        ACCENT
                    } else if dragged {
                        BORDER
                    } else {
                        CARD
                    })
                })
                .layout(|l| l.direction(TopToBottom).padding(10).gap(6))
                .children(|ui| card_body(ui, task, dragged));
        });
}

fn card_body(ui: &mut Ui, task: &Task, faded: bool) {
    ui.text(&task.title, |t| {
        t.font_size(fs(14)).color(if faded { MUTED } else { TEXT })
    });
    ui.element()
        .width(grow!())
        .height(fit!())
        .layout(|l| l.direction(LeftToRight).gap(10))
        .children(|ui| {
            ui.text(&task.short_id(), |t| {
                t.font_size(fs(11))
                    .font(&MONO)
                    .color(MUTED)
                    .wrap_mode(WrapMode::None)
            });
            if !task.links.is_empty() {
                ui.text(&plural(task.links.len(), "link"), |t| {
                    t.font_size(fs(11)).color(MUTED).wrap_mode(WrapMode::None)
                });
            }
            if !task.files.is_empty() {
                ui.text(&plural(task.files.len(), "file"), |t| {
                    t.font_size(fs(11)).color(MUTED).wrap_mode(WrapMode::None)
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
        .background_color(CARD_HOVER)
        .corner_radius(6.0)
        .border(|b| b.all(1).color(ACCENT))
        .layout(|l| l.direction(TopToBottom).padding(10).gap(6))
        .children(|ui| card_body(ui, task, false));
}

fn list_view(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let tasks = app.visible_list();
    ui.element()
        .width(grow!())
        .height(grow!())
        .background_color(PANEL)
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
                        .scrollbar(|s| s.width(4.0).thumb_color(BORDER).hide_after_frames(90))
                })
                .layout(|l| l.direction(TopToBottom))
                .children(|ui| {
                    if tasks.is_empty() {
                        ui.element()
                            .width(grow!())
                            .height(fit!())
                            .layout(|l| l.padding(16))
                            .children(|ui| {
                                ui.text("No tasks match.", |t| t.font_size(fs(14)).color(MUTED));
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
            None => MUTED,
        };
        if let Some((_, state)) = status {
            state.pointer_cursor |= hovered;
        }
        ui.element()
            .width(grow!())
            .height(fit!())
            .background_color(if selected {
                CARD_HOVER
            } else if hovered {
                CARD
            } else {
                PANEL
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
                        ui.text(cells[1], |t| t.font_size(size).font(&MONO).color(MUTED));
                    });
                ui.element().width(grow!()).height(fit!()).children(|ui| {
                    ui.text(cells[2], |t| {
                        t.font_size(size).color(if header { MUTED } else { TEXT })
                    });
                });
                ui.element()
                    .width(fixed!(px(140.0)))
                    .height(fit!())
                    .children(|ui| {
                        ui.text(cells[3], |t| t.font_size(size).color(MUTED));
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
        .background_color(PANEL)
        .corner_radius(8.0)
        .overflow(|o| {
            o.scroll_y()
                .scrollbar(|s| s.width(4.0).thumb_color(BORDER).hide_after_frames(90))
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
                ui.element()
                    .width(fixed!(8.0))
                    .height(fixed!(8.0))
                    .corner_radius(4.0)
                    .background_color(status_color(task.status))
                    .empty();
                ui.text(&task.short_id(), |t| {
                    t.font_size(fs(12)).font(&MONO).color(MUTED)
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

        ui.text(&task.title, |t| t.font_size(fs(20)).font(&BOLD).color(TEXT));

        ui.element()
            .width(grow!())
            .height(fit!())
            .layout(|l| l.direction(LeftToRight).gap(6).wrap().wrap_gap(6))
            .children(|ui| {
                for status in Status::ALL {
                    if chip(
                        ui,
                        state,
                        status.label(),
                        status_color(status),
                        task.status == status,
                    ) && task.status != status
                    {
                        actions.push(Action::Move {
                            id: task.id,
                            status,
                            position: None,
                        });
                    }
                }
            });

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
                        0xE3B341
                    } else {
                        MUTED
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
                            t.font_size(fs(13))
                                .color(if hovered { TEXT } else { ACCENT })
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
                    ui.text(kind, |t| t.font_size(fs(11)).color(MUTED));
                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                        let hovered = ui.hovered();
                        state.pointer_cursor |= hovered;
                        if ui.just_released() {
                            actions.push(Action::ViewFile(Some((task.id, att.id))));
                        }
                        ui.text(&att.name, |t| {
                            t.font_size(fs(13))
                                .color(if hovered { TEXT } else { ACCENT })
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

fn editor_panel(ui: &mut Ui, app: &App, state: &mut UiState, actions: &mut Vec<Action>) {
    let heading = match app.editing {
        Some(EditTarget::New(status)) => format!("New task in {status}"),
        _ => "Edit task".to_string(),
    };
    side_panel(ui, "editor", |ui| {
        ui.text(&heading, |t| t.font_size(fs(18)).font(&BOLD).color(TEXT));
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
                        .text_color(TEXT)
                        .placeholder("Details, acceptance criteria, notes…")
                        .placeholder_color(MUTED)
                        .cursor_color(ACCENT)
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
                        ui.text(&block.text, |t| t.font_size(size).font(&BOLD).color(TEXT));
                    });
            }
            BlockKind::Paragraph => ui.text(&block.text, |t| {
                t.font_size(fs(14)).line_height(fs(20)).color(TEXT)
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
                                ui.text(&marker, |t| t.font_size(fs(14)).color(MUTED));
                            });
                        ui.element().width(grow!()).height(fit!()).children(|ui| {
                            ui.text(&block.text, |t| {
                                t.font_size(fs(14)).line_height(fs(20)).color(TEXT)
                            });
                        });
                    });
            }
            BlockKind::Code => {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .background_color(INPUT)
                    .corner_radius(6.0)
                    .layout(|l| l.padding(10))
                    .children(|ui| {
                        ui.text(&block.text, |t| {
                            t.font_size(fs(13))
                                .font(&MONO)
                                .line_height(fs(18))
                                .color(TEXT)
                        });
                    });
            }
            BlockKind::Quote => {
                ui.element()
                    .width(grow!())
                    .height(fit!())
                    .border(|b| b.left(3).color(BORDER))
                    .layout(|l| l.padding((2, 0, 2, 12)))
                    .children(|ui| {
                        ui.text(&block.text, |t| {
                            t.font_size(fs(14)).line_height(fs(20)).color(MUTED)
                        })
                    });
            }
            BlockKind::Rule => {
                ui.element()
                    .width(grow!())
                    .height(fixed!(1.0))
                    .background_color(BORDER)
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
                .background_color(PANEL)
                .corner_radius(10.0)
                .border(|b| b.all(1).color(BORDER))
                .layout(|l| l.direction(TopToBottom).padding(16).gap(12))
                .children(|ui| {
                    ui.element()
                        .width(grow!())
                        .height(fit!())
                        .layout(|l| l.direction(LeftToRight).gap(8).align(Left, CenterY))
                        .children(|ui| {
                            ui.text(&att.name, |t| t.font_size(fs(16)).font(&BOLD).color(TEXT));
                            ui.element().width(grow!()).height(fixed!(1.0)).empty();
                            if button(ui, state, "Close", Style::Normal) {
                                actions.push(Action::ViewFile(None));
                            }
                        });
                    ui.element()
                        .id("file_body")
                        .width(grow!())
                        .height(grow!())
                        .overflow(|o| o.scroll().scrollbar(|s| s.width(6.0).thumb_color(BORDER)))
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
                                            .color(TEXT)
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
    // Scrim covering the window; the dialog sits centered inside it.
    ui.element()
        .width(fixed!(w))
        .height(fixed!(h))
        .floating(|f| f.attach_root().z_index(60))
        .background_color((0.0, 0.0, 0.0, 0.6))
        .layout(|l| l.align(CenterX, CenterY))
        .children(|ui| {
            ui.element()
                .width(fixed!(px(460.0).min(w * 0.9)))
                .height(fit!(0.0, h * 0.85))
                .background_color(PANEL)
                .corner_radius(10.0)
                .border(|b| b.all(1).color(BORDER))
                .overflow(|o| o.scroll_y())
                .layout(|l| l.direction(TopToBottom).padding(16).gap(12))
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
                            ui.text(title, |t| t.font_size(fs(16)).font(&BOLD).color(TEXT));
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
                                                t.font_size(fs(13)).font(&MONO).color(ACCENT)
                                            });
                                        });
                                    ui.element().width(grow!()).height(fit!()).children(|ui| {
                                        ui.text(what, |t| t.font_size(fs(14)).color(TEXT));
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
                                ui.text("Font size", |t| t.font_size(fs(14)).color(TEXT));
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
                                        t.font_size(fs(14)).font(&MONO).color(TEXT)
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
                                    t.font_size(fs(14)).color(TEXT)
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
                        "Empty columns start as strips. « collapses any column; click a strip to expand it.",
                    );
                    let saved_to = match &app.settings_path {
                        Some(p) => format!("Saved to {}", p.display()),
                        None => "Not saved: neither XDG_CONFIG_HOME nor HOME is set.".to_string(),
                    };
                    label(ui, &saved_to);
                });
        });
}
