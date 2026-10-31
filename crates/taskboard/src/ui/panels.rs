//! The panel beside the board or list: a task's details, or the editor.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{Column, Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Length};
use taskboard_core::{
    FileKind, Link, LinkKind, Status, Task, Ulid, default_link_kind, new_file_name,
};

use super::popup::popup;
use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::viewer::markdown;
use super::widgets::{
    El, btn, epic_badge, grow, input, label, link, path_label, save_keys, section_title,
    shorten_path, status_dot, warning_icon,
};
use super::{EDIT_TITLE, Gui, Message, NEW_FILE_NAME};
use crate::app::{Action, EditTarget, Owner, ProjectInput};

/// Unscaled minimum width of the status dropdown's list.
const STATUS_MENU_MIN_WIDTH: f32 = 160.0;
/// Unscaled width and greatest height of the lists of projects and of epics.
const PROJECT_MENU_WIDTH: f32 = 360.0;
const PROJECT_MENU_MAX_HEIGHT: f32 = 320.0;
/// The longest project path shown in the list of projects.
const PROJECT_MENU_PATH_CHARS: usize = 48;

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
    let board = &gui.app.board;
    let top = row![
        status_dot(p, task.status),
        text(task.short_id())
            .size(12)
            .font(MONO)
            .color(rgb(p.muted)),
    ]
    .push(task.epic.then(|| epic_badge(p, board.progress(task.id))))
    .extend([
        grow(),
        btn(
            p,
            "Edit",
            Style::Normal,
            Message::App(Action::Edit(EditTarget::Existing(task.id))),
        ),
        delete,
        btn(p, "×", Style::Quiet, Message::App(Action::Select(None))),
    ])
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

    let project = format!("{} ·", gui.app.board.project_name(&task.project));
    let mut body = column![
        top,
        text(&task.title).size(20).font(BOLD),
        status_dropdown(gui, task),
        path_label(p, &project, &task.project.display().to_string()),
    ];
    if gui.app.project_missing(&task.project) {
        body = body.push(missing_project(gui, &task.project));
    }
    body = body.extend([label(p, stamps), section_title(p, "Description")]);
    body = if task.description.trim().is_empty() {
        body.push(label(p, "No description."))
    } else {
        body.push(markdown(p, &task.description))
    };
    body = if task.epic {
        epic_children(gui, body, task)
    } else {
        task_epic(gui, body, task)
    };
    let owner = Owner::Task(task.id);
    body = links_editor(gui, body, owner, &task.links);
    let files = task.files.iter().map(|f| (f.id, f.kind, f.name.as_str()));
    body = files_editor(gui, body, owner, files);
    side_panel(p, width, body)
}

/// Adds an epic's child tasks, each opening on a click and removable, and
/// the way to demote it once it has none.
fn epic_children<'a>(
    gui: &'a Gui,
    mut body: Column<'a, Message>,
    task: &'a Task,
) -> Column<'a, Message> {
    let p = gui.pal();
    body = body.push(section_title(p, "Child tasks"));
    let children = gui.app.board.children(task.id);
    if children.is_empty() {
        return body.extend([
            label(p, "No child tasks yet. Pick this epic in a task's details."),
            row![btn(
                p,
                "Demote to task",
                Style::Normal,
                Message::App(Action::Demote(task.id))
            )]
            .into(),
        ]);
    }
    for child in children {
        body = body.push(
            row![
                status_dot(p, child.status),
                container(link(
                    p,
                    &child.title,
                    13,
                    Message::App(Action::Select(Some(child.id)))
                ))
                .width(Length::Fill),
                btn(
                    p,
                    "×",
                    Style::Quiet,
                    Message::App(Action::SetParent {
                        id: child.id,
                        parent: None
                    })
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    body.push(label(
        p,
        "An epic can be demoted once it has no child tasks.",
    ))
}

/// Adds the epic a task belongs to, which opens on a click and can be left,
/// or else the ways to join one or to become one.
fn task_epic<'a>(
    gui: &'a Gui,
    mut body: Column<'a, Message>,
    task: &'a Task,
) -> Column<'a, Message> {
    let p = gui.pal();
    body = body.push(section_title(p, "Epic"));
    if let Some(epic) = gui.app.board.parent_of(task) {
        return body.push(
            row![
                status_dot(p, epic.status),
                container(link(
                    p,
                    &epic.title,
                    13,
                    Message::App(Action::Select(Some(epic.id)))
                ))
                .width(Length::Fill),
                btn(
                    p,
                    "×",
                    Style::Quiet,
                    Message::App(Action::SetParent {
                        id: task.id,
                        parent: None
                    })
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let face = btn(
        p,
        "Choose epic ▾",
        Style::Normal,
        Message::ToggleEpicMenu(task.id),
    );
    let menu = (gui.ui.epic_open == Some(task.id)).then(|| epic_menu(gui, task));
    body.push(
        row![
            popup(face, menu).on_dismiss(Message::CloseEpicMenu),
            btn(
                p,
                "Promote to epic",
                Style::Normal,
                Message::App(Action::Promote(task.id))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
}

/// The epics in `task`'s project; picking one makes the task its child.
fn epic_menu<'a>(gui: &'a Gui, task: &'a Task) -> El<'a> {
    let p = gui.pal();
    let epics = gui.app.board.epics(&task.project);
    let list: El = if epics.is_empty() {
        container(label(
            p,
            "No epics in this project. Promote a task to make one.",
        ))
        .padding(6)
        .into()
    } else {
        let items = epics.into_iter().map(|epic| {
            button(
                row![status_dot(p, epic.status), text(&epic.title).size(13)]
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 10])
            .style(move |_, s| {
                let hovered = matches!(s, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: Some(Background::Color(rgb(if hovered {
                        p.card_hover
                    } else {
                        p.panel
                    }))),
                    text_color: rgb(p.text),
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ChooseEpic(task.id, epic.id))
            .into()
        });
        scrollable(column(items).spacing(2))
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .style(move |_, s| theme::scroll_style(p, s))
            .into()
    };
    container(list)
        .width(PROJECT_MENU_WIDTH)
        .max_height(PROJECT_MENU_MAX_HEIGHT)
        .padding(6)
        .style(move |_| theme::boxed(p.panel, p.border, 8.0))
        .into()
}

/// Says the project at `path` has no folder any more, with ways to point it
/// at its new folder or to delete it.
fn missing_project<'a>(gui: &'a Gui, path: &'a std::path::Path) -> El<'a> {
    let (app, p, ui) = (&gui.app, gui.pal(), &gui.ui);
    let count = app.board.tasks.iter().filter(|t| t.project == path).count();
    let tasks = super::widgets::plural(count, "task");
    let delete = if ui.confirm_delete_project.as_deref() == Some(path) {
        btn(
            p,
            format!("Really delete the project and its {tasks}?"),
            Style::Danger,
            Message::App(Action::DeleteProject(path.to_path_buf())),
        )
    } else {
        btn(
            p,
            "Delete project…",
            Style::Quiet,
            Message::ConfirmDeleteProject(path.to_path_buf()),
        )
    };
    let from = path.to_path_buf();
    let body = column![
        row![
            warning_icon(p, 16.0),
            text("Project folder is missing").size(14).font(BOLD),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        label(
            p,
            format!(
                "{} isn't there any more. If the project moved, point Taskboard at its new folder; otherwise you can delete the project and its {tasks}.",
                path.display()
            )
        ),
        row![
            input(
                p,
                "relocate_path",
                "The project's new folder",
                &ui.relocate_path,
                false,
                Message::RelocatePath
            )
            .on_submit(Message::Relocate(from.clone()))
            .width(Length::Fill),
            btn(p, "Browse…", Style::Normal, Message::BrowseRelocate(from.clone())),
            btn(p, "Relocate", Style::Primary, Message::Relocate(from)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        row![delete],
    ]
    .spacing(8);
    container(body)
        .padding(12)
        .width(Length::Fill)
        .style(move |_| theme::boxed(p.panel, p.warning, 8.0))
        .into()
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
    let mut attach = row![
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
        btn(p, "Browse…", Style::Normal, Message::Browse(owner)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    // A new file is created in the store right away, so the draft has none.
    if let Owner::Task(task) = owner {
        let face = btn(p, "New…", Style::Normal, Message::ToggleNewFile(task));
        let dialog = (gui.ui.new_file_for == Some(task)).then(|| new_file(gui, task));
        attach = attach.push(popup(face, dialog).on_dismiss(Message::CloseNewFile));
    }
    body.push(attach)
}

/// Asks for the name of a new file to create and attach to `task`, saying
/// what the file will be called.
fn new_file(gui: &Gui, task: Ulid) -> El<'_> {
    let (p, name) = (gui.pal(), &gui.ui.new_file_name);
    let checked = new_file_name(name);
    let hint = match &checked {
        _ if name.trim().is_empty() => label(p, "Without an extension, it's a .md file."),
        Ok(name) => label(p, format!("Creates {name}")),
        Err(e) => text(e.to_string()).size(12).color(rgb(p.danger)).into(),
    };
    let body = column![
        text("New file").size(14).font(BOLD),
        input(
            p,
            NEW_FILE_NAME,
            "notes.md",
            name,
            !name.trim().is_empty() && checked.is_err(),
            Message::NewFileName
        )
        .on_submit(Message::CreateFile(task)),
        hint,
        row![
            btn(p, "Create", Style::Primary, Message::CreateFile(task)),
            btn(p, "Cancel", Style::Quiet, Message::CloseNewFile),
        ]
        .spacing(8),
    ]
    .spacing(8);
    container(body)
        .width(280)
        .padding(12)
        .style(move |_| theme::boxed(p.panel, p.border, 8.0))
        .into()
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
        .key_binding(save_keys)
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
        label(p, "Project"),
        project_field(gui),
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

/// The editor's project input, with a list of known projects hanging from it,
/// a button to pick a folder, and a line saying what the input names.
fn project_field(gui: &Gui) -> El<'_> {
    let (app, p, ui) = (&gui.app, gui.pal(), &gui.ui);
    let face = row![
        input(
            p,
            "edit_project",
            "Project name, or a folder's path",
            &ui.edit_project,
            false,
            Message::EditProject
        )
        .width(Length::Fill),
        btn(p, "▾", Style::Normal, Message::ToggleProjectMenu),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let menu = ui.project_menu_open.then(|| project_menu(gui));
    let picker = row![
        container(popup(face, menu).on_dismiss(Message::CloseProjectMenu)).width(Length::Fill),
        btn(p, "Browse…", Style::Normal, Message::BrowseProject),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let hint = match app.project_input(&ui.edit_project) {
        ProjectInput::Empty => label(p, "Pick a project, browse for a folder, or type its path."),
        ProjectInput::Known(path) => {
            let name = app.board.project_name(&path);
            path_label(p, &format!("{name} ·"), &path.display().to_string())
        }
        ProjectInput::New(path) => path_label(p, "New project:", &path.display().to_string()),
        ProjectInput::Missing(path) => path_label(
            p,
            "New folder, created on save:",
            &path.display().to_string(),
        ),
    };
    column![picker, hint].spacing(6).into()
}

/// The projects on the board, by name; picking one fills the project input.
fn project_menu(gui: &Gui) -> El<'_> {
    let p = gui.pal();
    let items = gui
        .app
        .board
        .project_names()
        .into_iter()
        .map(|(project, name)| {
            let path = project.path.display().to_string();
            button(
                column![
                    text(name).size(13).wrapping(Wrapping::None),
                    text(shorten_path(&path, PROJECT_MENU_PATH_CHARS))
                        .size(11)
                        .color(rgb(p.muted))
                        .wrapping(Wrapping::None),
                ]
                .spacing(2),
            )
            .width(Length::Fill)
            .padding([5, 10])
            .style(move |_, s| {
                let hovered = matches!(s, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: Some(Background::Color(rgb(if hovered {
                        p.card_hover
                    } else {
                        p.panel
                    }))),
                    text_color: rgb(p.text),
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ChooseProject(project.path.clone()))
            .into()
        });
    let list: El = if gui.app.board.projects.is_empty() {
        container(label(
            p,
            "No projects yet. Browse for a folder or type its path.",
        ))
        .padding(6)
        .into()
    } else {
        scrollable(column(items).spacing(2))
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .style(move |_, s| theme::scroll_style(p, s))
            .into()
    };
    container(list)
        .width(PROJECT_MENU_WIDTH)
        .max_height(PROJECT_MENU_MAX_HEIGHT)
        .padding(6)
        .style(move |_| theme::boxed(p.panel, p.border, 8.0))
        .into()
}
