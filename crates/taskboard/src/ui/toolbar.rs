//! The toolbar: title, project path, view switcher, search and filters.

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, responsive, row, text};
use iced::{Alignment, Length, Padding};
use taskboard_core::{DateField, Status, parse_date};

use super::popup::popup;
use super::theme::{self, BOLD, Pal, Style, rgb};
use super::widgets::{El, btn, chip, input, label, shorten_path};
use super::{EDGE, Gui, Message, SEARCH};
use crate::app::{Action, EditTarget, View};

/// The longest project path the toolbar shows before collapsing it.
const MAX_PATH_CHARS: usize = 30;
/// A generous average glyph width for paths, as a fraction of the font size.
const PATH_CHAR_WIDTH: f32 = 0.65;

pub fn toolbar(gui: &Gui, window_width: f32) -> El<'_> {
    let (app, p) = (&gui.app, gui.pal());
    let root = app
        .store
        .dir()
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    // The path gets whatever room the buttons leave.
    let path = container(responsive(move |size| {
        let room = (size.width / (12.0 * PATH_CHAR_WIDTH)) as usize;
        text(shorten_path(&root, room.min(MAX_PATH_CHARS)))
            .size(12)
            .color(rgb(p.muted))
            .wrapping(Wrapping::None)
            .into()
    }))
    .width(Length::Fill)
    .height(16)
    .clip(true);

    let mut top = row![text("Taskboard").size(18).font(BOLD), path]
        .spacing(10)
        .align_y(Alignment::Center);
    for (view, name) in [
        (View::Board, "Board"),
        (View::List, "List"),
        (View::InProgress, "In Progress"),
    ] {
        let style = if app.view == view {
            Style::Primary
        } else {
            Style::Normal
        };
        top = top.push(btn(p, name, style, Message::App(Action::SetView(view))));
    }
    top = top
        .push(btn(
            p,
            "+ New task",
            Style::Normal,
            Message::App(Action::Edit(EditTarget::New(Status::Open))),
        ))
        .push(btn(
            p,
            "⚙ Settings",
            Style::Quiet,
            Message::App(Action::ShowSettings(true)),
        ));

    let search = container(
        input(
            p,
            SEARCH,
            "Search title, description, links  (Ctrl+F)",
            &app.filter.text,
            false,
            Message::Search,
        )
        .width(Length::Fill),
    )
    // Up to 360 wide, leaving room for the buttons beside it.
    .width((window_width - 2.0 * EDGE - 200.0).clamp(110.0, 360.0));
    let mut filters = row![search, filter_button(gui)]
        .spacing(8)
        .align_y(Alignment::Center);
    if !app.filter.is_empty() {
        filters = filters.push(btn(p, "Clear", Style::Quiet, Message::ClearFilters));
    }

    column![
        container(top).padding(Padding::new(EDGE).bottom(6)),
        container(filters).padding(Padding::new(EDGE).top(6).bottom(12)),
    ]
    .into()
}

/// The filter button, with the status and date filters in a dropdown below it.
fn filter_button(gui: &Gui) -> El<'_> {
    let (f, p) = (&gui.app.filter, gui.pal());
    let active = f.statuses.len() + f.after.is_some() as usize + f.before.is_some() as usize;
    let (label, style) = if active > 0 {
        (format!("Filter ({active}) ▾"), Style::Primary)
    } else {
        ("Filter ▾".to_string(), Style::Normal)
    };
    let menu = gui.ui.filters_open.then(|| filter_menu(gui));
    popup(btn(p, label, style, Message::ToggleFilters), menu)
        .on_dismiss(Message::CloseFilters)
        .into()
}

fn filter_menu(gui: &Gui) -> El<'_> {
    let (app, p) = (&gui.app, gui.pal());
    let chips = Status::ALL.into_iter().map(|status| {
        let on = app.filter.statuses.contains(&status);
        chip(
            p,
            status.label(),
            p.status_color(status),
            on,
            Message::App(Action::ToggleStatusFilter(status)),
        )
    });
    let (field_label, next) = match app.filter.date_field {
        DateField::Created => ("Created ▾", DateField::Updated),
        DateField::Updated => ("Updated ▾", DateField::Closed),
        DateField::Closed => ("Closed ▾", DateField::Created),
    };
    let bad = |s: &str| !s.trim().is_empty() && parse_date(s).is_err();
    let (after, before) = &app.date_inputs;
    let menu = column![
        label(p, "Status"),
        row(chips).spacing(8).wrap().vertical_spacing(8),
        row![
            container(label(p, "Date")).width(Length::Fill),
            btn(
                p,
                field_label,
                Style::Quiet,
                Message::App(Action::SetDateField(next))
            ),
        ]
        .padding(Padding::ZERO.top(6))
        .align_y(Alignment::Center),
        row![
            input(
                p,
                "date_after",
                "from YYYY-MM-DD",
                after,
                bad(after),
                Message::DateAfter
            )
            .width(Length::Fill),
            input(
                p,
                "date_before",
                "to YYYY-MM-DD",
                before,
                bad(before),
                Message::DateBefore
            )
            .width(Length::Fill),
        ]
        .spacing(8),
    ]
    .spacing(8);
    container(menu)
        .width(330)
        .padding(14)
        .style(move |_| theme::boxed(p.panel, p.border, 8.0))
        .into()
}

pub fn error_bar<'a>(p: Pal, err: &'a str) -> El<'a> {
    let dismiss = button(text("Dismiss").size(13))
        .padding(0)
        .style(move |_, _| button::Style {
            text_color: rgb(p.text),
            ..button::Style::default()
        })
        .on_press(Message::App(Action::DismissError));
    container(
        row![
            container(text(err).size(13).color(rgb(p.danger_text))).width(Length::Fill),
            dismiss,
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(move |_| theme::fill(p.error_bar, 0.0))
    .into()
}
