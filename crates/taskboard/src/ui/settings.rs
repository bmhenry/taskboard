//! The settings modal, with a page listing the keybindings.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{Column, column, container, row, scrollable, text};
use iced::{Alignment, Border, Length, Size};
use taskboard_core::HookEvent;
use taskboard_core::hooks;
use taskboard_core::settings::{DEFAULT_FONT_SIZE, Theme};

use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::widgets::{
    El, btn, checkbox, grow, help_bubble, input, label, path_label, section_title,
};
use super::{Gui, KEYBINDINGS, Message};
use crate::app::Action;

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

pub fn settings(gui: &Gui, window: Size) -> El<'_> {
    let (app, p, ui) = (&gui.app, gui.pal(), &gui.ui);
    let size = app.settings.font_size;
    let (title, toggle) = if ui.show_keybindings {
        ("Keybindings", "⚙ Settings")
    } else {
        ("Settings", "⌨ Keybindings")
    };
    let mut body = column![
        row![
            text(title).size(16).font(BOLD),
            grow(),
            btn(p, toggle, Style::Quiet, Message::ToggleKeybindings),
            btn(
                p,
                "Close",
                Style::Normal,
                Message::App(Action::ShowSettings(false))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(12);

    if ui.show_keybindings {
        for (keys, what) in KEYBINDINGS {
            body = body.push(
                row![
                    container(text(*keys).size(13).font(MONO).color(rgb(p.accent))).width(160),
                    container(text(*what).size(14)).width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
    } else {
        let mut theme_row = row![setting_name("Theme")]
            .spacing(8)
            .align_y(Alignment::Center);
        for (theme, name) in [(Theme::Dark, "Dark"), (Theme::Light, "Light")] {
            let style = if app.settings.theme == theme {
                Style::Primary
            } else {
                Style::Normal
            };
            theme_row = theme_row.push(btn(p, name, style, Message::App(Action::SetTheme(theme))));
        }
        let mut size_row = row![
            setting_name("Font size"),
            btn(
                p,
                "−",
                Style::Normal,
                Message::App(Action::SetFontSize(size - 1))
            ),
            container(text(size.to_string()).size(14).font(MONO)).center_x(32),
            btn(
                p,
                "+",
                Style::Normal,
                Message::App(Action::SetFontSize(size + 1))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if size != DEFAULT_FONT_SIZE {
            size_row = size_row.push(btn(
                p,
                "Reset",
                Style::Quiet,
                Message::App(Action::SetFontSize(DEFAULT_FONT_SIZE)),
            ));
        }
        let collapse = app.settings.collapse_empty_columns;
        let (on_off, style) = if collapse {
            ("On", Style::Primary)
        } else {
            ("Off", Style::Normal)
        };
        body = body
            .push(section_title(p, "Global"))
            .push(theme_row)
            .push(size_row)
            .push(label(p, "Ctrl+= / Ctrl+- also change the font size."));
        body = body
            .push(
                row![
                    setting_name("Collapse empty columns"),
                    btn(
                        p,
                        on_off,
                        style,
                        Message::App(Action::SetCollapseEmptyColumns(!collapse))
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .push(label(
                p,
                "Empty columns start as strips. Click a column's header or « to collapse it; click a strip to expand it.",
            ));
        body = hooks_settings(gui, body);
        body = body.push(match &app.settings_path {
            Some(path) => path_label(p, "Saved to", &path.display().to_string()),
            None => label(p, "Not saved: neither XDG_CONFIG_HOME nor HOME is set."),
        });
    }

    container(
        scrollable(container(body).padding(16))
            .direction(Direction::Vertical(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .style(move |_, s| theme::scroll_style(p, s)),
    )
    .width(460.0_f32.min(window.width * 0.9))
    .max_height(window.height * 0.85)
    .style(move |_| container::Style {
        border: Border {
            color: rgb(p.border),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..theme::fill(p.panel, 10.0)
    })
    .into()
}

fn setting_name(name: &str) -> El<'_> {
    container(text(name).size(14)).width(Length::Fill).into()
}

fn hooks_settings<'a>(gui: &'a Gui, mut body: Column<'a, Message>) -> Column<'a, Message> {
    let p: Pal = gui.pal();
    body = body.push(section_title(p, "Hooks")).push(label(
        p,
        "Commands to run when a task is created or moved to another column, here or with tb. They run in the project folder without a shell, so for pipes or redirects use bash -c '…'.",
    ));
    for (i, hook) in gui.app.settings.hooks.iter().enumerate() {
        let mut events = row![text("When a task is").size(14)]
            .spacing(8)
            .align_y(Alignment::Center);
        for (event, name) in [(HookEvent::Created, "Created"), (HookEvent::Moved, "Moved")] {
            let style = if hook.event == event {
                Style::Primary
            } else {
                Style::Normal
            };
            events = events.push(btn(
                p,
                name,
                style,
                Message::App(Action::SetHookEvent(i, event)),
            ));
        }
        events = events.push(grow()).push(btn(
            p,
            "Remove",
            Style::Quiet,
            Message::App(Action::RemoveHook(i)),
        ));
        let command = input(
            p,
            format!("hook-{i}"),
            "Command, e.g. /path/to/script or bash -c '…'",
            &hook.command,
            hooks::split_command(&hook.command).is_err(),
            move |s| Message::HookCommand(i, s),
        )
        .on_submit(Message::App(Action::SaveSettings))
        .width(Length::Fill);
        let pass_id = row![
            checkbox(
                p,
                "Pass the task ID",
                hook.pass_task_id,
                Message::App(Action::SetHookPassTaskId(i, !hook.pass_task_id))
            ),
            help_bubble(p, PASS_ID_HELP),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        body = body.push(
            container(column![events, command, pass_id].spacing(8))
                .padding(10)
                .width(Length::Fill)
                .style(move |_| theme::boxed(p.card, p.card_edge, 8.0)),
        );
    }
    body.push(btn(
        p,
        "+ Add hook",
        Style::Normal,
        Message::App(Action::AddHook),
    ))
}
