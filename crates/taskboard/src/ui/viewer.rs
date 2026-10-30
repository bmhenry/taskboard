//! Markdown rendering and the attached-file viewer.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{column, container, image, row, scrollable, text};
use iced::{Border, Length, Padding, Size};
use taskboard_core::{Attachment, FileKind};

use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::widgets::{El, btn, grow, label};
use super::{Gui, Message};
use crate::app::Action;
use crate::markdown::{self, BlockKind};

pub fn markdown<'a>(p: Pal, src: &str) -> El<'a> {
    let blocks = markdown::parse(src).into_iter().map(|block| -> El<'a> {
        match block.kind {
            BlockKind::Heading(level) => {
                let size = match level {
                    1 => 20,
                    2 => 17,
                    _ => 15,
                };
                container(text(block.text).size(size).font(BOLD))
                    .padding(Padding::ZERO.top(4))
                    .into()
            }
            BlockKind::Paragraph => text(block.text).size(14).line_height(20.0 / 14.0).into(),
            BlockKind::ListItem { depth, marker } => row![
                container(text(marker).size(14).color(rgb(p.muted))).width(18),
                container(text(block.text).size(14).line_height(20.0 / 14.0)).width(Length::Fill),
            ]
            .spacing(6)
            .padding(Padding::ZERO.left(4.0 + 16.0 * depth as f32))
            .into(),
            BlockKind::Code => container(
                text(block.text)
                    .size(13)
                    .font(MONO)
                    .line_height(18.0 / 13.0),
            )
            .width(Length::Fill)
            .padding(10)
            .style(move |_| theme::fill(p.input, 6.0))
            .into(),
            BlockKind::Quote => row![
                container(iced::widget::Space::new())
                    .width(3)
                    .height(Length::Fill)
                    .style(move |_| theme::fill(p.border, 0.0)),
                text(block.text)
                    .size(14)
                    .line_height(20.0 / 14.0)
                    .color(rgb(p.muted)),
            ]
            .spacing(12)
            .height(Length::Shrink)
            .padding([2, 0])
            .into(),
            BlockKind::Rule => container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(1)
                .style(move |_| theme::fill(p.border, 0.0))
                .into(),
        }
    });
    column(blocks).spacing(10).into()
}

pub fn file_viewer<'a>(gui: &'a Gui, att: &'a Attachment, window: Size) -> El<'a> {
    let (app, p) = (&gui.app, gui.pal());
    let path = app.store.file_path(att);
    let header = row![
        text(&att.name).size(16).font(BOLD),
        grow(),
        btn(
            p,
            "Close",
            Style::Normal,
            Message::App(Action::ViewFile(None))
        ),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    let body: El = match att.kind {
        FileKind::Image if path.is_file() => image(image::Handle::from_path(path)).into(),
        FileKind::Image => label(p, "Couldn't load this image."),
        kind => {
            let content = gui.ui.file_text.as_deref().unwrap_or_default();
            if kind == FileKind::Markdown {
                markdown(p, content)
            } else {
                text(content)
                    .size(13)
                    .font(MONO)
                    .line_height(18.0 / 13.0)
                    .wrapping(Wrapping::None)
                    .into()
            }
        }
    };
    let bar = || Scrollbar::new().width(6).scroller_width(6);
    // Only plain text, which isn't wrapped, scrolls sideways; markdown needs
    // a width to wrap in.
    let direction = match att.kind {
        FileKind::Text => Direction::Both {
            vertical: bar(),
            horizontal: bar(),
        },
        _ => Direction::Vertical(bar()),
    };
    let body = scrollable(container(body).padding(iced::Padding::ZERO.right(8).bottom(8)))
        .direction(direction)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_, s| theme::scroll_style(p, s));
    container(column![header, body].spacing(12))
        .width((window.width * 0.8).min(1100.0))
        .height(window.height * 0.85)
        .padding(16)
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

/// Reads a text file for the viewer, or says why it couldn't.
pub fn read_text(path: &std::path::Path) -> String {
    std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_else(|e| format!("Couldn't read file: {e}"))
}
