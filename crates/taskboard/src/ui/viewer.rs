//! Markdown rendering and the attached-file viewer.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{column, container, image, row, scrollable, text, text_editor};
use iced::{Border, Length, Padding, Size};
use taskboard_core::{Attachment, FileKind};

use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::widgets::{El, btn, grow, label, save_keys};
use super::{FILE_EDITOR, Gui, Message};
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
    let mut header = row![text(&att.name).size(16).font(BOLD), grow()]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if let Some(content) = &gui.ui.file_edit {
        if gui.file_edit_dirty() {
            header = header.push(label(p, "Unsaved changes"));
        }
        header = header.extend([
            btn(p, "Save", Style::Primary, Message::SaveFile),
            btn(p, "Cancel", Style::Quiet, Message::CancelFileEdit),
        ]);
        let editor = text_editor(content)
            .id(FILE_EDITOR)
            .on_action(Message::EditFileText)
            .font(MONO)
            .size(13)
            .padding([7, 8])
            .height(Length::Fill)
            .key_binding(save_keys)
            .style(move |_, status| theme::editor_style(p, status));
        return dialog(p, column![header, editor].spacing(12), window);
    }
    if att.kind != FileKind::Image && gui.ui.file_editable {
        header = header.push(btn(p, "Edit", Style::Normal, Message::EditFile));
    }
    header = header.push(btn(
        p,
        "Close",
        Style::Normal,
        Message::App(Action::ViewFile(None)),
    ));
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
    dialog(p, column![header, body].spacing(12), window)
}

/// The viewer's frame around `content`, sized to the window.
fn dialog<'a>(p: Pal, content: iced::widget::Column<'a, Message>, window: Size) -> El<'a> {
    container(content)
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

/// Reads a text file for the viewer, or says why it couldn't, and whether
/// the text can be edited: only UTF-8 survives being saved back unchanged.
pub fn read_text(path: &std::path::Path) -> (String, bool) {
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => (text, true),
            Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
        },
        Err(e) => (format!("Couldn't read file: {e}"), false),
    }
}
