//! The board view: a column per status, with cards that drag between them.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::{LazyLock, Mutex};

use iced::advanced::text::Paragraph as _;
use iced::alignment::Vertical;
use iced::widget::canvas::{self, Frame, Geometry, Path};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::{LineHeight, Shaping, Wrapping};
use iced::widget::{
    Id, button, column, container, hover, image, mouse_area, row, scrollable, text,
};
use iced::{Alignment, Length, Pixels, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};
use taskboard_core::{LinkKind, Status, Task, Ulid};

use super::theme::{self, BOLD, MONO, Pal, Style, rgb, status_index};
use super::widgets::{El, btn, count_badge, dot, grow, plural};
use super::{EDGE, Gui, Message, side_width};
use crate::app::{Action, DRAG_THRESHOLD, EditTarget, View};
use crate::source_link::{self, Site};

const COLUMN_MIN_WIDTH: f32 = 210.0;
const COLUMN_GAP: f32 = 10.0;
const STRIP_WIDTH: f32 = 34.0;

pub fn column_id(status: Status) -> Id {
    Id::from(format!("column-{}", status_index(status)))
}

pub fn column_ids() -> Vec<Id> {
    Status::ALL.into_iter().map(column_id).collect()
}

/// Which status each found column zone belongs to.
pub fn columns_from(zones: &[(Id, Rectangle)]) -> Vec<(Status, Rectangle)> {
    zones
        .iter()
        .filter_map(|(id, bounds)| {
            let status = Status::ALL.into_iter().find(|s| column_id(*s) == *id)?;
            Some((status, *bounds))
        })
        .collect()
}

pub fn card_id(id: Ulid) -> Id {
    Id::from(format!("card-{}", id))
}

pub fn board(gui: &Gui, window_width: f32) -> El<'_> {
    let app = &gui.app;
    // Columns share the board's width, down to a minimum, past which it scrolls.
    let mut room = window_width - 2.0 * EDGE;
    if app.editing.is_some() || app.selected_task().is_some() {
        room -= side_width(window_width) + 12.0;
    }
    let collapsed = Status::ALL
        .iter()
        .filter(|s| app.column_collapsed(**s))
        .count();
    let expanded = Status::ALL.len() - collapsed;
    let gaps = (Status::ALL.len() - 1) as f32 * COLUMN_GAP;
    let column_width = ((room - gaps - collapsed as f32 * STRIP_WIDTH) / expanded.max(1) as f32)
        .max(COLUMN_MIN_WIDTH);

    let columns = Status::ALL.into_iter().map(|status| {
        if app.column_collapsed(status) {
            collapsed_column(gui, status)
        } else {
            board_column(gui, status, column_width)
        }
    });
    scrollable(row(columns).spacing(COLUMN_GAP).height(Length::Fill))
        .id("board")
        .direction(Direction::Horizontal(
            Scrollbar::new().width(6).scroller_width(6),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_, status| theme::scroll_style(gui.pal(), status))
        .into()
}

/// The column the dragged card is over, if any.
fn drop_target(gui: &Gui, status: Status) -> bool {
    gui.ui
        .drag
        .as_ref()
        .is_some_and(|d| !d.cancelled && d.over == Some(status))
}

fn column_style(p: Pal, target: bool) -> container::Style {
    if target {
        theme::boxed_edge(p.panel, p.accent, 2.0, 8.0)
    } else {
        theme::boxed(p.panel, p.panel, 8.0)
    }
}

/// A collapsed column drawn as a thin strip with its title along it like a
/// book spine. Clicking expands it; it still accepts dropped cards.
fn collapsed_column(gui: &Gui, status: Status) -> El<'_> {
    let p = gui.pal();
    let target = drop_target(gui, status);
    let spine = canvas::Canvas::new(Spine {
        p,
        status,
        count: gui.app.visible_column(status).len(),
    })
    .width(Length::Fill)
    .height(Length::Fill);
    mouse_area(
        container(spine)
            .id(column_id(status))
            .width(STRIP_WIDTH)
            .height(Length::Fill)
            .style(move |_| column_style(p, target)),
    )
    .on_press(Message::App(Action::ToggleColumn(status)))
    .interaction(mouse::Interaction::Pointer)
    .into()
}

fn board_column(gui: &Gui, status: Status, width: f32) -> El<'_> {
    let (app, p) = (&gui.app, gui.pal());
    let tasks = app.visible_column(status);
    let target = drop_target(gui, status);

    // The whole header collapses the column, like clicking its strip expands
    // it; « just makes that visible. The buttons and In Progress's title,
    // which opens the In Progress view, take their own clicks.
    let title: El = if status == Status::InProgress {
        button(
            text(status.label())
                .size(14)
                .font(BOLD)
                .wrapping(Wrapping::None),
        )
        .padding(0)
        .style(move |_, s| button::Style {
            text_color: rgb(match s {
                button::Status::Hovered | button::Status::Pressed => p.accent,
                _ => p.text,
            }),
            ..button::Style::default()
        })
        .on_press(Message::App(Action::SetView(View::InProgress)))
        .into()
    } else {
        text(status.label())
            .size(14)
            .font(BOLD)
            .wrapping(Wrapping::None)
            .into()
    };
    // Clipped, as a narrow column can't fit the title and both buttons.
    let header = mouse_area(
        container(
            row![
                dot(p.status_color(status)),
                title,
                count_badge(p, tasks.len()),
                grow(),
                btn(
                    p,
                    "«",
                    Style::Quiet,
                    Message::App(Action::ToggleColumn(status))
                ),
                btn(
                    p,
                    "+",
                    Style::Quiet,
                    Message::App(Action::Edit(EditTarget::New(status)))
                ),
            ]
            .spacing(8)
            .height(28)
            .padding(iced::Padding::ZERO.left(4))
            .align_y(Alignment::Center),
        )
        .clip(true),
    )
    .on_press(Message::App(Action::ToggleColumn(status)))
    .interaction(mouse::Interaction::Pointer);

    let cards = scrollable(column(tasks.iter().map(|t| card(gui, t))).spacing(8))
        .direction(Direction::Vertical(
            Scrollbar::new().width(4).scroller_width(4),
        ))
        .height(Length::Fill)
        .style(move |_, s| theme::scroll_style(p, s));

    container(column![header, cards].spacing(8))
        .id(column_id(status))
        .padding(8)
        .width(width)
        .height(Length::Fill)
        .style(move |_| column_style(p, target))
        .into()
}

fn card<'a>(gui: &'a Gui, task: &'a Task) -> El<'a> {
    let (p, id) = (gui.pal(), task.id);
    let selected = gui.app.selected == Some(id);
    let hovered = gui.ui.hovered_card == Some(id);
    // Only the copy under the pointer is drawn while dragging, so style it
    // as the card being carried.
    let dragged = gui
        .ui
        .drag
        .as_ref()
        .is_some_and(|d| d.id == id && !d.cancelled);
    let (bg, edge) = if dragged {
        (p.card_hover, p.accent)
    } else if selected {
        (if hovered { p.card_hover } else { p.card }, p.accent)
    } else {
        (if hovered { p.card_hover } else { p.card }, p.card_edge)
    };
    let face = container(card_body(p, task))
        .padding(10)
        .width(Length::Fill)
        .style(move |_| theme::boxed(bg, edge, 6.0));
    let hoverable = mouse_area(face)
        .on_enter(Message::HoverCard(id, true))
        .on_exit(Message::HoverCard(id, false))
        .interaction(mouse::Interaction::Pointer);
    // A press on the source link pill is taken by the pill, so it neither
    // starts a drag nor selects the card.
    let droppable = iced_drop::droppable(hoverable)
        .drag_threshold(DRAG_THRESHOLD)
        .drag_hide(true)
        .on_press(Message::App(Action::Select(Some(id))))
        .on_drag(move |point, _| Message::DragMoved(id, point))
        .on_drop(move |point, _| Message::Dropped(id, point));
    container(droppable).id(card_id(id)).into()
}

/// A card's contents: title, id, link and file counts, and source link.
pub fn card_body<'a>(p: Pal, task: &'a Task) -> El<'a> {
    let mut meta = row![
        text(task.short_id())
            .size(11)
            .font(MONO)
            .color(rgb(p.muted))
            .wrapping(Wrapping::None)
    ]
    .spacing(10);
    if !task.links.is_empty() {
        meta = meta.push(small(p, plural(task.links.len(), "link")));
    }
    if !task.files.is_empty() {
        meta = meta.push(small(p, plural(task.files.len(), "file")));
    }
    let mut body = column![text(&task.title).size(14), meta].spacing(6);
    if let Some(link) = task.links.iter().find(|l| l.kind == LinkKind::Source) {
        body = body.push(source_pill(p, &link.url));
    }
    body.into()
}

fn small<'a>(p: Pal, s: String) -> El<'a> {
    text(s)
        .size(11)
        .color(rgb(p.muted))
        .wrapping(Wrapping::None)
        .into()
}

/// A round pill with the source site's logo and a short form of the link.
fn source_pill<'a>(p: Pal, url: &str) -> El<'a> {
    let site = source_link::site(url);
    let label = source_link::short_label(url);
    let face = move |hovered: bool| -> El<'a> {
        let color = if hovered { p.text } else { p.muted };
        let mark: El = match logo(site, color) {
            Some(handle) => image(handle).width(12).height(12).into(),
            None => text("↗").size(12).color(rgb(color)).into(),
        };
        // Clipped in case a large font pushes it past a narrow card's edge.
        container(
            row![
                mark,
                text(label.clone())
                    .size(11)
                    .color(rgb(if hovered { p.text } else { p.accent }))
                    .wrapping(Wrapping::None)
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .height(20)
        .padding(iced::Padding::ZERO.left(7).right(8))
        .center_y(20)
        .clip(true)
        .style(move |_| theme::boxed(p.panel, p.card_edge, 10.0))
        .into()
    };
    button(hover(face(false), face(true)))
        .padding(0)
        .style(theme::bare_button)
        .on_press(Message::App(Action::OpenUrl(url.to_string())))
        .into()
}

/// The source site's logo in `color`, or None for sites without one. The
/// logos are white, so tinting keeps their shape in the alpha channel.
fn logo(site: Site, color: u32) -> Option<image::Handle> {
    static CACHE: LazyLock<Mutex<HashMap<(Site, u32), image::Handle>>> =
        LazyLock::new(Default::default);
    let bytes: &[u8] = match site {
        Site::GitHub => include_bytes!("../../assets/logos/github.png"),
        Site::GitLab => include_bytes!("../../assets/logos/gitlab.png"),
        Site::Bitbucket => include_bytes!("../../assets/logos/bitbucket.png"),
        Site::Jira => include_bytes!("../../assets/logos/jira.png"),
        Site::Other => return None,
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = cache.get(&(site, color)) {
        return Some(handle.clone());
    }
    let mut img = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let [r, g, b, _] = rgb(color).into_rgba8();
    for px in img.pixels_mut() {
        px.0 = [r, g, b, px.0[3]];
    }
    let handle = image::Handle::from_rgba(img.width(), img.height(), img.into_raw());
    cache.insert((site, color), handle.clone());
    Some(handle)
}

/// Draws a collapsed column's dot, title and count down the strip.
struct Spine {
    p: Pal,
    status: Status,
    count: usize,
}

impl canvas::Program<Message> for Spine {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let p = self.p;
        let mut frame = Frame::new(renderer, bounds.size());
        // Lay the spine out left to right, then turn it to run down the strip.
        frame.translate(Vector::new(bounds.width / 2.0, 8.0));
        frame.rotate(FRAC_PI_2);

        frame.fill(
            &Path::circle(Point::new(4.0, 0.0), 4.0),
            rgb(p.status_color(self.status)),
        );
        let label = self.status.label();
        let label_x = 16.0;
        frame.fill_text(spine_text(label, label_x, 14.0, BOLD, p.text));

        let count = self.count.to_string();
        let badge_x = label_x + text_width(label, 14.0, BOLD) + 8.0;
        let badge_w = text_width(&count, 12.0, super::theme::REGULAR) + 14.0;
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(badge_x, -9.0),
                Size::new(badge_w, 18.0),
                9.0.into(),
            ),
            rgb(p.card),
        );
        frame.fill_text(spine_text(
            &count,
            badge_x + 7.0,
            12.0,
            super::theme::REGULAR,
            p.muted,
        ));
        vec![frame.into_geometry()]
    }
}

/// Text starting at `x` along the spine, centered on its middle line.
fn spine_text(content: &str, x: f32, size: f32, font: iced::Font, color: u32) -> canvas::Text {
    canvas::Text {
        content: content.to_string(),
        position: Point::new(x, 0.0),
        color: rgb(color),
        size: Pixels(size),
        font,
        align_y: Vertical::Center,
        ..canvas::Text::default()
    }
}

fn text_width(content: &str, size: f32, font: iced::Font) -> f32 {
    <Renderer as iced::advanced::text::Renderer>::Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: LineHeight::default(),
        font,
        align_x: iced::advanced::text::Alignment::Left,
        align_y: Vertical::Top,
        shaping: Shaping::Basic,
        wrapping: Wrapping::None,
    })
    .min_width()
}
