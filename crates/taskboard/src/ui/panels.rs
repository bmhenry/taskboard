//! The panel beside the board or list: a task's details, or the editor.

use iced::keyboard::{Key, key::Named};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{Column, Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Length};
use taskboard_core::{FileKind, Link, LinkKind, Status, Task, Ulid, default_link_kind};

use super::popup::popup;
use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::viewer::markdown;
use super::widgets::{El, btn, grow, input, label, link, section_title, status_dot};
use super::{EDIT_TITLE, Gui, Message};
use crate::app::{Action, EditTarget, Owner};

/// Unscaled minimum width of the status dropdown's list.
const STATUS_MENU_MIN_WIDTH: f32 = 160.0;

pub fn side_panel<'a>(p: Pal, width: Length, body: Column<'a, Message>) -> El<'a> {
    container(
        scrollable(container(body.spacing(10)).padding(16))
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .height(Length::Fill)
            .style(move |_, s| theme::scroll_style(p, s)),
    )
    .width(width)
    .height(Length::Fill)
    .style(move |_| theme::fill(p.panel, 8.0))
    .into()
}

pub fn detail<'a>(gui: &'a Gui, task: &'a Task, width: Length) -> El<'a> {
    let (p, ui) = (gui.pal(), &gui.ui);
    let delete = if ui.confirm_delete == Some(task.id) {
        btn(
            p,
            "Really delete?",
            Style::Danger,
            Message::App(Action::Delete(task.id)),
        )
    } else {
        btn(p, "Delete", Style::Quiet, Message::ConfirmDelete(task.id))
    };
    let top = row![
        status_dot(p, task.status),
        text(task.short_id())
            .size(12)
            .font(MONO)
            .color(rgb(p.muted)),
        grow(),
        btn(
            p,
            "Edit",
            Style::Normal,
            Message::App(Action::Edit(EditTarget::Existing(task.id)))
        ),
        delete,
        btn(p, "×", Style::Quiet, Message::App(Action::Select(None))),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

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

    let mut body = column![
        top,
        text(&task.title).size(20).font(BOLD),
        status_dropdown(gui, task),
        label(p, stamps),
        section_title(p, "Description"),
    ];
    body = if task.description.trim().is_empty() {
        body.push(label(p, "No description."))
    } else {
        body.push(markdown(p, &task.description))
    };
    let owner = Owner::Task(task.id);
    body = links_editor(gui, body, owner, &task.links);
    let files = task.files.iter().map(|f| (f.id, f.kind, f.name.as_str()));
    body = files_editor(gui, body, owner, files);
    side_panel(p, width, body)
}

/// Adds a task's or the draft's links, each removable, and a row to add one.
fn links_editor<'a>(
    gui: &'a Gui,
    mut body: Column<'a, Message>,
    owner: Owner,
    links: &'a [Link],
) -> Column<'a, Message> {
    let p = gui.pal();
    body = body.push(section_title(p, "Links"));
    for (i, l) in links.iter().enumerate() {
        let kind_color = if l.kind == LinkKind::Source {
            p.status_color(Status::InProgress)
        } else {
            p.muted
        };
        body = body.push(
            row![
                text(l.kind.to_string()).size(11).color(rgb(kind_color)),
                container(link(
                    p,
                    l.display_text(),
                    13,
                    Message::App(Action::OpenUrl(l.url.clone()))
                ))
                .width(Length::Fill),
                btn(
                    p,
                    "×",
                    Style::Quiet,
                    Message::App(Action::RemoveLink { owner, index: i })
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let kind = match gui.ui.link_kind {
        Some((o, kind)) if o == owner => kind,
        _ => default_link_kind(links),
    };
    let toggled = if kind == LinkKind::Source {
        LinkKind::Context
    } else {
        LinkKind::Source
    };
    body.push(
        row![
            input(
                p,
                "link_url",
                "https://…",
                &gui.ui.link_url,
                false,
                Message::LinkUrl
            )
            .on_submit(Message::AddLink(owner, kind))
            .width(Length::Fill),
            btn(
                p,
                kind.to_string(),
                Style::Quiet,
                Message::SetLinkKind(owner, toggled)
            ),
            btn(p, "Add", Style::Normal, Message::AddLink(owner, kind)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
}

/// Adds a task's or the draft's files as (id, kind, name), each removable,
/// and a row to attach one. A task's files open in the viewer when clicked;
/// the draft's aren't copied in yet, so there's nothing to view.
fn files_editor<'a>(
    gui: &'a Gui,
    mut body: Column<'a, Message>,
    owner: Owner,
    files: impl Iterator<Item = (Ulid, FileKind, &'a str)>,
) -> Column<'a, Message> {
    let p = gui.pal();
    body = body.push(section_title(p, "Files"));
    for (id, kind, name) in files {
        let kind = match kind {
            FileKind::Image => "image",
            FileKind::Markdown => "md",
            FileKind::Text => "text",
        };
        let name: El = match owner {
            Owner::Task(task) => link(
                p,
                name,
                13,
                Message::App(Action::ViewFile(Some((task, id)))),
            ),
            Owner::Draft => text(name).size(13).into(),
        };
        body = body.push(
            row![
                text(kind).size(11).color(rgb(p.muted)),
                container(name).width(Length::Fill),
                btn(
                    p,
                    "×",
                    Style::Quiet,
                    Message::App(Action::Detach { owner, file: id })
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    body.push(
        row![
            input(
                p,
                "attach_path",
                "Path to a file to attach",
                &gui.ui.attach_path,
                false,
                Message::AttachPath
            )
            .on_submit(Message::Attach(owner))
            .width(Length::Fill),
            btn(p, "Attach", Style::Normal, Message::Attach(owner)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
}

/// Shows the task's status; clicking it opens a list of statuses to move the
/// task to.
fn status_dropdown<'a>(gui: &'a Gui, task: &'a Task) -> El<'a> {
    let p = gui.pal();
    let color = p.status_color(task.status);
    let face = button(
        row![
            status_dot(p, task.status),
            text(task.status.label())
                .size(13)
                .color(rgb(color))
                .wrapping(Wrapping::None),
            text("▾").size(13).color(rgb(p.muted)),
        ]
        .spacing(8)
        .height(Length::Fill)
        .align_y(Alignment::Center),
    )
    .height(28)
    .padding([0, 12])
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(rgb(if hovered {
                p.card_hover
            } else {
                p.card
            }))),
            border: Border {
                color: rgb(color),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    })
    .on_press(Message::ToggleStatusMenu(task.id));
    let menu = (gui.ui.status_open == Some(task.id)).then(|| status_menu(p, task));
    container(popup(face, menu).on_dismiss(Message::CloseStatusMenu)).into()
}

fn status_menu<'a>(p: Pal, task: &'a Task) -> El<'a> {
    let items = Status::ALL.into_iter().map(|status| {
        let current = task.status == status;
        let on_press = if current {
            Message::CloseStatusMenu
        } else {
            Message::App(Action::Move {
                id: task.id,
                status,
                position: None,
            })
        };
        button(
            row![
                status_dot(p, status),
                text(status.label())
                    .size(13)
                    .color(rgb(p.status_color(status)))
                    .wrapping(Wrapping::None),
            ]
            .spacing(8)
            .height(Length::Fill)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(28)
        .padding([0, 10])
        .style(move |_, s| {
            let hovered = matches!(s, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(Background::Color(rgb(if hovered {
                    p.card_hover
                } else if current {
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
        .on_press(on_press)
        .into()
    });
    // The zero-height space holds the list at its minimum width.
    let list = column(items)
        .push(Space::new().width(STATUS_MENU_MIN_WIDTH))
        .spacing(2)
        .width(Length::Shrink);
    container(list)
        .width(Length::Shrink)
        .padding(6)
        .style(move |_| theme::boxed(p.panel, p.border, 8.0))
        .into()
}

pub fn editor<'a>(gui: &'a Gui, width: Length) -> El<'a> {
    let (app, p, ui) = (&gui.app, gui.pal(), &gui.ui);
    let heading = match app.editing {
        Some(EditTarget::New(status)) => format!("New task in {status}"),
        _ => "Edit task".to_string(),
    };
    let desc = iced::widget::text_editor(&ui.edit_desc)
        .placeholder("Details, acceptance criteria, notes…")
        .on_action(Message::EditDesc)
        .font(MONO)
        .size(13)
        .padding([7, 8])
        .min_height(200)
        .height(Length::Shrink)
        // Ctrl+S and Ctrl+Enter save, handled for the whole window.
        .key_binding(|press: KeyPress| {
            let ctrl = press.modifiers.control() || press.modifiers.logo();
            match press.key.as_ref() {
                Key::Named(Named::Enter) | Key::Character("s") if ctrl => None,
                _ => Binding::from_key_press(press),
            }
        })
        .style(move |_, status| theme::editor_style(p, status));
    let mut body = column![
        text(heading).size(18).font(BOLD),
        label(p, "Title"),
        input(
            p,
            EDIT_TITLE,
            "What needs doing?",
            &ui.edit_title,
            false,
            Message::EditTitle
        )
        .width(Length::Fill),
        label(p, "Description (Markdown)"),
        desc,
    ];
    // An existing task's links and files are changed from its details.
    if matches!(app.editing, Some(EditTarget::New(_))) {
        let draft = &app.draft;
        body = links_editor(gui, body, Owner::Draft, &draft.links);
        let files = draft.files.iter().map(|f| (f.id, f.kind, f.name.as_str()));
        body = files_editor(gui, body, Owner::Draft, files);
    }
    body = body.push(
        row![
            btn(p, "Save", Style::Primary, Message::Save),
            btn(p, "Cancel", Style::Quiet, Message::App(Action::CancelEdit)),
            label(p, "Ctrl+S to save · Esc to cancel"),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    side_panel(p, width, body)
}
