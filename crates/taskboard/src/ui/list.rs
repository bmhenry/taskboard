//! The list view, and the In Progress view's sidebar.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Length, Padding};
use taskboard_core::{Status, Task};

use super::board::card_body;
use super::theme::{self, BOLD, MONO, Pal, rgb};
use super::widgets::{El, count_badge, label, missing_project_badge};
use super::{Gui, Message, panels};
use crate::app::Action;

pub fn list(gui: &Gui) -> El<'_> {
    let (app, p) = (&gui.app, gui.pal());
    let tasks = app.visible_list();
    let mut rows = column![];
    if tasks.is_empty() {
        rows =
            rows.push(container(text("No tasks match.").size(14).color(rgb(p.muted))).padding(16));
    }
    for task in tasks {
        let updated = task
            .updated
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        let selected = app.selected == Some(task.id);
        let missing = app
            .project_missing(&task.project)
            .then(|| missing_project_badge(p, &task.project, 14.0));
        let cells = list_row(
            p,
            Some(task.status),
            missing,
            [
                task.status.label().to_string(),
                task.short_id(),
                task.title.clone(),
                app.board.project_name(&task.project),
                updated,
            ],
        );
        rows = rows.push(
            button(cells)
                .padding(0)
                .width(Length::Fill)
                .style(move |_, status| {
                    let hovered =
                        matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: Some(Background::Color(rgb(if selected {
                            p.card_hover
                        } else if hovered {
                            p.card
                        } else {
                            p.panel
                        }))),
                        border: Border {
                            radius: 4.0.into(),
                            ..Border::default()
                        },
                        ..button::Style::default()
                    }
                })
                .on_press(Message::App(Action::Select(Some(task.id)))),
        );
    }
    let header = list_row(
        p,
        None,
        None,
        ["Status", "ID", "Title", "Project", "Updated"].map(String::from),
    );
    container(column![
        header,
        scrollable(rows)
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4)
            ))
            .height(Length::Fill)
            .style(move |_, s| theme::scroll_style(p, s)),
    ])
    .padding(8)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| theme::fill(p.panel, 8.0))
    .into()
}

/// One row of the list table; `status` is `None` for the header. `badge` goes
/// before the title.
fn list_row<'a>(
    p: Pal,
    status: Option<Status>,
    badge: Option<El<'a>>,
    cells: [String; 5],
) -> El<'a> {
    let header = status.is_none();
    let size = if header { 12 } else { 14 };
    let first = status.map_or(p.muted, |s| p.status_color(s));
    let [status_cell, id, title, project, updated] = cells;
    row![
        container(text(status_cell).size(size).color(rgb(first))).width(120),
        container(text(id).size(size).font(MONO).color(rgb(p.muted))).width(90),
        container(
            row![]
                .push(badge)
                .push(
                    text(title)
                        .size(size)
                        .color(rgb(if header { p.muted } else { p.text }))
                )
                .spacing(6)
                .align_y(Alignment::Center)
        )
        .width(Length::Fill),
        container(
            text(project)
                .size(size)
                .color(rgb(p.muted))
                .wrapping(Wrapping::None)
        )
        .width(160)
        .clip(true),
        container(text(updated).size(size).color(rgb(p.muted))).width(140),
    ]
    .spacing(12)
    .padding([8, 10])
    .align_y(Alignment::Center)
    .into()
}

/// In-progress tasks grouped by project in a sidebar, with the selected
/// task's details (or the editor) filling the rest.
pub fn in_progress(gui: &Gui) -> El<'_> {
    let (app, p) = (&gui.app, gui.pal());
    let groups = app.in_progress_groups();
    let mut items = column![].spacing(8);
    if groups.is_empty() {
        items = items.push(container(label(p, "No tasks in progress.")).padding(6));
    }
    for (project, tasks) in groups {
        items = items.push(
            row![
                text(project).size(13).font(BOLD).color(rgb(p.muted)),
                count_badge(p, tasks.len())
            ]
            .spacing(8)
            .padding(Padding::new(4.0).bottom(0))
            .align_y(Alignment::Center),
        );
        items = items.push(
            column(tasks.into_iter().map(|t| in_progress_item(gui, t)))
                .spacing(6)
                .padding(Padding::ZERO.left(12)),
        );
    }
    let sidebar = container(
        scrollable(container(items).padding(10))
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .height(Length::Fill)
            .style(move |_, s| theme::scroll_style(p, s)),
    )
    .width(300)
    .height(Length::Fill)
    .style(move |_| theme::fill(p.panel, 8.0));

    let pane = if app.editing.is_some() {
        panels::editor(gui, Length::Fill)
    } else if let Some(task) = app.selected_task() {
        panels::detail(gui, task, Length::Fill)
    } else {
        panels::side_panel(
            p,
            Length::Fill,
            column![label(p, "Select a task to see its details.")],
        )
    };
    row![sidebar, pane].spacing(12).height(Length::Fill).into()
}

/// A task in the In Progress sidebar; clicking it selects the task.
fn in_progress_item<'a>(gui: &'a Gui, task: &'a Task) -> El<'a> {
    let p = gui.pal();
    let selected = gui.app.selected == Some(task.id);
    // The source link pill inside takes its own clicks.
    button(
        container(card_body(
            p,
            task,
            None,
            gui.app.project_missing(&task.project),
        ))
        .padding(10)
        .width(Length::Fill),
    )
    .padding(0)
    .width(Length::Fill)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(rgb(if hovered {
                p.card_hover
            } else {
                p.card
            }))),
            text_color: rgb(p.text),
            border: Border {
                color: rgb(if selected { p.accent } else { p.card_edge }),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    })
    .on_press(Message::App(Action::Select(Some(task.id))))
    .into()
}
