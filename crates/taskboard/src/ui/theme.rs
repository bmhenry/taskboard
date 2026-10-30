//! Colors, fonts and widget styles.

use iced::border::Radius;
use iced::widget::{button, container, scrollable, text_editor, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme as IcedTheme, font};
use taskboard_core::Status;
use taskboard_core::settings::Theme;

pub const REGULAR_BYTES: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
pub const BOLD_BYTES: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf");
pub const MONO_BYTES: &[u8] = include_bytes!("../../assets/fonts/DejaVuSansMono.ttf");

pub const REGULAR: Font = Font::with_name("DejaVu Sans");
pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..REGULAR
};
pub const MONO: Font = Font::with_name("DejaVu Sans Mono");

/// The colors the UI draws with, one set per [`Theme`].
pub struct Palette {
    /// Window background, behind the panels.
    pub bg: u32,
    pub panel: u32,
    pub card: u32,
    pub card_hover: u32,
    /// Outline of cards and card-colored buttons, so they hold their shape
    /// on a background close to their own color.
    pub card_edge: u32,
    pub input: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub danger: u32,
    /// Indexed like [`Status::ALL`].
    pub status: [u32; 6],
    pub primary: u32,
    pub primary_hover: u32,
    pub danger_button: u32,
    pub danger_button_hover: u32,
    /// Text on the danger button and the error bar.
    pub danger_text: u32,
    pub danger_text_hover: u32,
    pub error_bar: u32,
}

static DARK: Palette = Palette {
    bg: 0x15171C,
    panel: 0x1C1F26,
    card: 0x252932,
    card_hover: 0x2D323D,
    card_edge: 0x252932,
    input: 0x111318,
    border: 0x343A46,
    text: 0xE6E8EE,
    muted: 0x8F96A3,
    accent: 0x6EA8FE,
    danger: 0xE5534B,
    status: [0x8F96A3, 0x6EA8FE, 0xE3B341, 0xD2A8FF, 0xF778BA, 0x56D364],
    primary: 0x2F5FAE,
    primary_hover: 0x3A71CC,
    danger_button: 0x5A2422,
    danger_button_hover: 0x7A2E2A,
    danger_text: 0xFFD7D4,
    danger_text_hover: 0xFFFFFF,
    error_bar: 0x3A1D1C,
};

/// Neutral grays in the style of Apple's site: a white window, columns only a
/// hair off white, and white cards held apart by thin edges rather than heavy
/// fills. Status colors are deeper than the dark theme's so they read on white.
static LIGHT: Palette = Palette {
    bg: 0xFFFFFF,
    panel: 0xF5F5F7,
    card: 0xFFFFFF,
    card_hover: 0xEDEDF0,
    card_edge: 0xDCDCE1,
    input: 0xFFFFFF,
    border: 0xD2D2D7,
    text: 0x1D1D1F,
    muted: 0x6E6E73,
    accent: 0x0071E3,
    danger: 0xD70015,
    status: [0x6E6E73, 0x0071E3, 0xB25F00, 0x8944AB, 0xC4307A, 0x248A3D],
    primary: 0x0071E3,
    primary_hover: 0x0062C4,
    danger_button: 0xFFE5E5,
    danger_button_hover: 0xFFD0D0,
    danger_text: 0xB0000F,
    danger_text_hover: 0x8A000C,
    error_bar: 0xFFEBEB,
};

pub type Pal = &'static Palette;

pub fn palette(theme: Theme) -> Pal {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// The Iced theme behind widgets the UI doesn't style itself.
pub fn iced_theme(theme: Theme) -> IcedTheme {
    let p = palette(theme);
    IcedTheme::custom(
        "Taskboard".to_string(),
        iced::theme::Palette {
            background: rgb(p.bg),
            text: rgb(p.text),
            primary: rgb(p.primary),
            success: rgb(p.status[5]),
            warning: rgb(p.status[2]),
            danger: rgb(p.danger),
        },
    )
}

pub fn rgb(c: u32) -> Color {
    Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl Palette {
    pub fn status_color(&self, status: Status) -> u32 {
        self.status[status_index(status)]
    }
}

pub fn status_index(status: Status) -> usize {
    Status::ALL.iter().position(|s| *s == status).unwrap_or(0)
}

/// A box with a background, a 1px edge and rounded corners.
pub fn boxed(bg: u32, edge: u32, radius: f32) -> container::Style {
    boxed_edge(bg, edge, 1.0, radius)
}

pub fn boxed_edge(bg: u32, edge: u32, width: f32, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(rgb(bg))),
        border: Border {
            color: rgb(edge),
            width,
            radius: radius.into(),
        },
        ..container::Style::default()
    }
}

pub fn fill(bg: u32, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(rgb(bg))),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Style {
    Normal,
    Primary,
    Danger,
    Quiet,
}

pub fn button_style(p: Pal, style: Style, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (bg, fg) = match (style, hovered) {
        (Style::Primary, false) => (p.primary, 0xFFFFFF),
        (Style::Primary, true) => (p.primary_hover, 0xFFFFFF),
        (Style::Danger, false) => (p.danger_button, p.danger_text),
        (Style::Danger, true) => (p.danger_button_hover, p.danger_text_hover),
        (Style::Quiet, false) => (p.panel, p.muted),
        (Style::Quiet, true) => (p.card_hover, p.text),
        (Style::Normal, false) => (p.card, p.text),
        (Style::Normal, true) => (p.card_hover, p.text),
    };
    button::Style {
        background: Some(Background::Color(rgb(bg))),
        text_color: rgb(fg),
        border: Border {
            color: rgb(if style == Style::Normal {
                p.card_edge
            } else {
                bg
            }),
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A button that draws nothing itself, for clickable content styled by hand.
pub fn bare_button(_: &IcedTheme, _: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: Color::TRANSPARENT,
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn input_style(p: Pal, invalid: bool, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(rgb(p.input)),
        border: Border {
            color: rgb(if invalid {
                p.danger
            } else if focused {
                p.accent
            } else {
                p.border
            }),
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: rgb(p.muted),
        placeholder: rgb(p.muted),
        value: rgb(p.text),
        selection: selection(p),
    }
}

pub fn editor_style(p: Pal, status: text_editor::Status) -> text_editor::Style {
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: Background::Color(rgb(p.input)),
        border: Border {
            color: rgb(if focused { p.accent } else { p.border }),
            width: 1.0,
            radius: 6.0.into(),
        },
        placeholder: rgb(p.muted),
        value: rgb(p.text),
        selection: selection(p),
    }
}

fn selection(p: Pal) -> Color {
    Color {
        a: 0.35,
        ..rgb(p.accent)
    }
}

/// A thin scrollbar in the border color, on no rail.
pub fn scroll_style(p: Pal, status: scrollable::Status) -> scrollable::Style {
    let hovered = !matches!(status, scrollable::Status::Active { .. });
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(if hovered { rgb(p.muted) } else { rgb(p.border) }),
            border: Border {
                radius: Radius::from(3.0),
                ..Border::default()
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(rgb(p.card)),
            border: Border {
                color: rgb(p.border),
                width: 1.0,
                radius: 12.0.into(),
            },
            shadow: Shadow::default(),
            icon: rgb(p.text),
        },
    }
}
