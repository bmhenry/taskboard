//! Markdown rendering and the attached-file viewer.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Wrapping;
use std::collections::HashSet;

use iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, text, text_editor,
};
use iced::{Border, Length, Padding, Size, mouse};
use taskboard_core::{Attachment, FileKind, Ulid};

use super::theme::{self, BOLD, MONO, Pal, Style, rgb};
use super::widgets::{El, btn, grow, label, save_keys};
use super::{FILE_EDITOR, Gui, Message, dialog_width};
use crate::app::Action;
use crate::markdown::{self, Block, BlockId, BlockKind};

/// Which rendered markdown a block is in, for keeping which of its
/// collapsible blocks are open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Doc {
    /// The viewed file.
    File,
    /// A task's description.
    Description(Ulid),
}

/// `src` rendered, as part of `doc`.
pub fn markdown<'a>(gui: &Gui, doc: Doc, src: &str) -> El<'a> {
    let md = Md {
        p: gui.pal(),
        doc,
        toggled: &gui.ui.toggled,
    };
    md.blocks(markdown::parse(src))
}

/// What rendering a markdown document needs.
#[derive(Clone, Copy)]
struct Md<'g> {
    p: Pal,
    doc: Doc,
    toggled: &'g HashSet<(Doc, BlockId)>,
}

impl Md<'_> {
    fn blocks<'a>(self, blocks: Vec<Block>) -> El<'a> {
        column(blocks.into_iter().map(|b| self.block(b)))
            .spacing(10)
            .into()
    }

    /// A block, with its children under it: a heading's section flush with
    /// it, a list item's indented to line up with its text, or for a
    /// sub-list, with its own marker.
    fn block<'a>(self, block: Block) -> El<'a> {
        let p = self.p;
        if let BlockKind::Details { open } = block.kind {
            return self.details(block, open);
        }
        let section = matches!(block.kind, BlockKind::Heading(_));
        let own = md_leaf(p, block.kind, block.text);
        if block.children.is_empty() {
            return own;
        }
        let children = block.children.into_iter().map(|child| -> El<'a> {
            let indent = match child.kind {
                _ if section => 0.0,
                BlockKind::ListItem { .. } => 16.0,
                _ => 28.0,
            };
            container(self.block(child))
                .padding(Padding::ZERO.left(indent))
                .into()
        });
        column![own, column(children).spacing(10)]
            .spacing(10)
            .into()
    }

    /// A details block: its summary, which opens and closes it, over what
    /// it holds when open, boxed together.
    fn details<'a>(self, block: Block, starts_open: bool) -> El<'a> {
        let p = self.p;
        let key = (self.doc, block.id);
        let open = starts_open != self.toggled.contains(&key);
        let summary = if block.text.is_empty() {
            "Details".to_string()
        } else {
            block.text
        };
        let chevron = if open { "▾" } else { "▸" };
        let head = button(
            row![
                container(text(chevron).size(13).color(rgb(p.muted))).width(14),
                text(summary).size(14).font(BOLD).color(rgb(p.text)),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
        )
        .on_press(Message::ToggleSection(key.0, key.1))
        .padding([6, 10])
        .width(Length::Fill)
        .style(theme::bare_button);
        let head = mouse_area(head).interaction(mouse::Interaction::Pointer);
        let mut body = column![head];
        if open && !block.children.is_empty() {
            body = body.push(
                container(self.blocks(block.children))
                    .padding(Padding::ZERO.left(30).right(10).bottom(10)),
            );
        }
        container(body)
            .width(Length::Fill)
            .style(move |_| theme::boxed(p.panel, p.border, 6.0))
            .into()
    }
}

/// A block's own text, without its children.
fn md_leaf<'a>(p: Pal, kind: BlockKind, body: String) -> El<'a> {
    match kind {
        BlockKind::Heading(level) => {
            let size = match level {
                1 => 20,
                2 => 17,
                _ => 15,
            };
            container(text(body).size(size).font(BOLD))
                .padding(Padding::ZERO.top(4))
                .into()
        }
        BlockKind::Paragraph => text(body).size(14).line_height(20.0 / 14.0).into(),
        BlockKind::ListItem { marker } => row![
            container(text(marker).size(14).color(rgb(p.muted))).width(18),
            container(text(body).size(14).line_height(20.0 / 14.0)).width(Length::Fill),
        ]
        .spacing(6)
        .padding(Padding::ZERO.left(4))
        .into(),
        BlockKind::Code => container(text(body).size(13).font(MONO).line_height(18.0 / 13.0))
            .width(Length::Fill)
            .padding(10)
            .style(move |_| theme::fill(p.input, 6.0))
            .into(),
        BlockKind::Quote => row![
            container(iced::widget::Space::new())
                .width(3)
                .height(Length::Fill)
                .style(move |_| theme::fill(p.border, 0.0)),
            text(body)
                .size(14)
                .line_height(20.0 / 14.0)
                .color(rgb(p.muted)),
        ]
        .spacing(12)
        .height(Length::Shrink)
        .padding([2, 0])
        .into(),
        BlockKind::Chart => super::chart::chart(p, &body),
        // Drawn by `Md::details`.
        BlockKind::Details { .. } => text(body).into(),
        BlockKind::Table(t) => super::table::md_table(p, t),
        BlockKind::Rule => container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(1)
            .style(move |_| theme::fill(p.border, 0.0))
            .into(),
    }
}

pub fn file_viewer<'a>(gui: &'a Gui, att: &'a Attachment, window: Size) -> El<'a> {
    let p = gui.pal();
    let header = row![text(&att.name).size(16).font(BOLD), grow()];
    let close = btn(
        p,
        "Close",
        Style::Normal,
        Message::App(Action::ViewFile(None)),
    );
    dialog(p, file_contents(gui, att, header, Some(close)), window)
}

/// A file open in a tab of the task's details, which name it.
pub fn file_tab<'a>(gui: &'a Gui, att: &'a Attachment) -> El<'a> {
    let header = row![grow()];
    container(file_contents(gui, att, header, None))
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// `header`, with the viewer's buttons added and then `close` unless the
/// file is being edited, over the viewed file or its editor.
fn file_contents<'a>(
    gui: &'a Gui,
    att: &'a Attachment,
    header: iced::widget::Row<'a, Message>,
    close: Option<El<'a>>,
) -> iced::widget::Column<'a, Message> {
    let p = gui.pal();
    let mut header = header.spacing(8).align_y(iced::Alignment::Center);
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
        return column![header, editor].spacing(12);
    }
    let rendered = gui.ui.file_rendered;
    if att.kind != FileKind::Image {
        // Names the view it switches to.
        let other = if rendered {
            "Show Source"
        } else {
            "Show Rendered"
        };
        header = header.push(btn(p, other, Style::Normal, Message::RenderFile(!rendered)));
        if gui.ui.file_editable {
            header = header.push(btn(p, "Edit", Style::Normal, Message::EditFile));
        }
    }
    header = header.push(close);
    let plain = att.kind != FileKind::Image && !rendered;
    let body: El = match att.kind {
        // Until it's read, or why it couldn't be.
        FileKind::Image => match &gui.ui.file_image {
            Some(handle) => image(handle.clone()).into(),
            None => label(p, gui.ui.file_text.as_deref().unwrap_or_default()),
        },
        _ => {
            let content = gui.ui.file_text.as_deref().unwrap_or_default();
            if rendered {
                markdown(gui, Doc::File, content)
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
    let direction = if plain {
        Direction::Both {
            vertical: bar(),
            horizontal: bar(),
        }
    } else {
        Direction::Vertical(bar())
    };
    let body = scrollable(container(body).padding(iced::Padding::ZERO.right(8).bottom(8)))
        .direction(direction)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_, s| theme::scroll_style(p, s));
    column![header, body].spacing(12)
}

/// The viewer's frame around `content`, sized to the window.
fn dialog<'a>(p: Pal, content: iced::widget::Column<'a, Message>, window: Size) -> El<'a> {
    container(content)
        .width(dialog_width(window.width, (window.width * 0.8).min(1100.0)))
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

/// A viewed file's contents, as read by the platform.
#[derive(Debug, Clone)]
pub enum Contents {
    /// Text, or why the file couldn't be read. Only UTF-8 is `editable`,
    /// since only it survives being saved back unchanged.
    Text {
        text: String,
        editable: bool,
    },
    Image(image::Handle),
}

/// What the viewer shows for a file of `kind` that was `read`.
pub fn contents(kind: FileKind, read: Result<Vec<u8>, String>) -> Contents {
    let text = |text, editable| Contents::Text { text, editable };
    match (kind, read) {
        (FileKind::Image, Ok(bytes)) => Contents::Image(image::Handle::from_bytes(bytes)),
        (FileKind::Image, Err(_)) => text("Couldn't load this image.".into(), false),
        (_, Ok(bytes)) => match String::from_utf8(bytes) {
            Ok(utf8) => text(utf8, true),
            Err(e) => text(String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
        },
        (_, Err(e)) => text(format!("Couldn't read file: {e}"), false),
    }
}
