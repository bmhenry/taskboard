//! Small building blocks shared by the views.

use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, mouse_area, row, text, text_input, tooltip};
use iced::{Alignment, Background, Border, Element, Length, mouse};
use taskboard_core::Status;

use super::Message;
use super::theme::{self, BOLD, MONO, Pal, Style, rgb};

pub type El<'a> = Element<'a, Message>;

pub fn btn<'a>(
    p: Pal,
    label: impl text::IntoFragment<'a>,
    style: Style,
    on_press: Message,
) -> El<'a> {
    button(
        text(label)
            .size(13)
            .wrapping(Wrapping::None)
            .height(Length::Fill)
            .align_y(Vertical::Center),
    )
    .height(28)
    .padding([0, 10])
    .style(move |_, status| theme::button_style(p, style, status))
    .on_press(on_press)
    .into()
}

/// Clickable text in the accent color that turns to the text color when hovered.
pub fn link<'a>(
    p: Pal,
    label: impl text::IntoFragment<'a>,
    size: u32,
    on_press: Message,
) -> El<'a> {
    button(text(label).size(size))
        .padding(0)
        .style(move |_, status| button::Style {
            text_color: rgb(match status {
                button::Status::Hovered | button::Status::Pressed => p.text,
                _ => p.accent,
            }),
            ..button::Style::default()
        })
        .on_press(on_press)
        .into()
}

/// A toggleable pill; `on` draws it filled with `color`.
pub fn chip<'a>(p: Pal, label: &'a str, color: u32, on: bool, on_press: Message) -> El<'a> {
    button(
        text(label)
            .size(12)
            .wrapping(Wrapping::None)
            .height(Length::Fill)
            .align_y(Vertical::Center),
    )
    .height(26)
    .padding([0, 12])
    .style(move |_, _| button::Style {
        background: Some(Background::Color(rgb(if on { color } else { p.panel }))),
        text_color: rgb(if on { p.bg } else { color }),
        border: Border {
            color: rgb(color),
            width: 1.0,
            radius: 13.0.into(),
        },
        ..button::Style::default()
    })
    .on_press(on_press)
    .into()
}

pub fn input<'a>(
    p: Pal,
    id: impl Into<iced::widget::Id>,
    placeholder: &str,
    value: &str,
    invalid: bool,
    on_input: impl Fn(String) -> Message + 'a,
) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .id(id)
        .on_input(on_input)
        .size(14)
        .padding([6, 8])
        .style(move |_, status| theme::input_style(p, invalid, status))
}

/// A box that's ticked when `on`, followed by `label`.
pub fn checkbox<'a>(p: Pal, label: &'a str, on: bool, on_press: Message) -> El<'a> {
    let tick = container(if on {
        text("✓").size(12).color(rgb(0xFFFFFF))
    } else {
        text("")
    })
    .center(16)
    .style(move |_| {
        theme::boxed(
            if on { p.primary } else { p.input },
            if on { p.primary } else { p.border },
            4.0,
        )
    });
    mouse_area(
        row![tick, text(label).size(14)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .on_press(on_press)
    .interaction(mouse::Interaction::Pointer)
    .into()
}

/// A small "?" that shows `paragraphs` in a bubble below it while hovered.
/// Paragraphs marked `true` are code and drawn in the monospace font.
pub fn help_bubble<'a>(p: Pal, paragraphs: &'a [(&'a str, bool)]) -> El<'a> {
    let mark = container(text("?").size(11).font(BOLD).color(rgb(p.muted)))
        .center(18)
        .style(move |_| container::Style {
            border: Border {
                color: rgb(p.muted),
                width: 1.0,
                radius: 9.0.into(),
            },
            ..container::Style::default()
        });
    let bubble = column(paragraphs.iter().map(|&(para, code)| {
        if code {
            text(para).size(12).font(MONO).color(rgb(p.accent)).into()
        } else {
            text(para).size(13).into()
        }
    }))
    .spacing(6)
    .width(360);
    tooltip(
        mark,
        container(bubble)
            .padding(10)
            .style(move |_| theme::boxed(p.card, p.border, 8.0)),
        tooltip::Position::Bottom,
    )
    .gap(6)
    .into()
}

pub fn label<'a>(p: Pal, s: impl text::IntoFragment<'a>) -> El<'a> {
    text(s).size(12).color(rgb(p.muted)).into()
}

/// A muted `prefix` followed by `path`, broken anywhere if it won't fit.
pub fn path_label<'a>(p: Pal, prefix: &str, path: &str) -> El<'a> {
    text(format!("{prefix} {path}"))
        .size(12)
        .color(rgb(p.muted))
        .wrapping(Wrapping::WordOrGlyph)
        .into()
}

pub fn section_title<'a>(p: Pal, s: &'a str) -> El<'a> {
    container(text(s).size(13).font(BOLD).color(rgb(p.muted)))
        .padding(iced::Padding::ZERO.top(8).bottom(2))
        .into()
}

/// A column's task count, in a small pill next to its title.
pub fn count_badge<'a>(p: Pal, count: usize) -> El<'a> {
    container(
        text(count.to_string())
            .size(12)
            .color(rgb(p.muted))
            .wrapping(Wrapping::None),
    )
    .height(18)
    .padding([0, 7])
    .center_y(18)
    .style(move |_| theme::fill(p.card, 9.0))
    .into()
}

pub fn status_dot<'a>(p: Pal, status: Status) -> El<'a> {
    dot(p.status_color(status))
}

pub fn dot<'a>(color: u32) -> El<'a> {
    container(Space::new())
        .width(8)
        .height(8)
        .style(move |_| theme::fill(color, 4.0))
        .into()
}

/// Empty space that takes up whatever width is left in a row.
pub fn grow<'a>() -> El<'a> {
    Space::new().width(Length::Fill).into()
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Collapses `path` to `…` plus as many trailing components (with their
/// leading separator) as fit in `max_chars`. The last component is always
/// kept, even if it alone is longer.
pub fn shorten_path(path: &str, max_chars: usize) -> String {
    const SEPARATORS: [char; 2] = ['/', '\\'];
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    let path = path.trim_end_matches(SEPARATORS);
    let mut seps = path.rmatch_indices(SEPARATORS).map(|(i, _)| i);
    let Some(mut start) = seps.next() else {
        return path.to_string();
    };
    for sep in seps {
        if path[sep..].chars().count() + 1 > max_chars {
            break;
        }
        start = sep;
    }
    format!("…{}", &path[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_are_left_alone() {
        assert_eq!(
            shorten_path("/home/me/dev/taskboard", 30),
            "/home/me/dev/taskboard"
        );
    }

    #[test]
    fn long_paths_keep_the_trailing_components_that_fit() {
        let path = "/home/someone/projects/clients/acme/taskboard";
        assert_eq!(shorten_path(path, 30), "…/clients/acme/taskboard");
        assert_eq!(shorten_path(path, 20), "…/acme/taskboard");
        assert!(shorten_path(path, 30).chars().count() <= 30);
    }

    #[test]
    fn the_last_component_is_kept_even_if_too_long() {
        assert_eq!(
            shorten_path("/srv/a-very-long-project-name", 10),
            "…/a-very-long-project-name"
        );
        assert_eq!(shorten_path("/srv/project/", 5), "…/project");
        assert_eq!(
            shorten_path("a-very-long-project-name", 10),
            "a-very-long-project-name"
        );
    }

    #[test]
    fn windows_paths_collapse_at_backslashes() {
        assert_eq!(
            shorten_path(r"C:\Users\someone\projects\clients\taskboard", 22),
            r"…\clients\taskboard"
        );
    }
}
