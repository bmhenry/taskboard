//! Rendered markdown tables.

use iced::alignment::Horizontal;
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{container, scrollable, table, text, themer};
use iced::{Border, Length};

use super::theme::{self, BOLD, Pal, rgb};
use super::widgets::El;
use crate::markdown::{Align, Table};

/// The widest a cell gets before its text wraps.
const MAX_CELL_WIDTH: f32 = 360.0;

/// `t` in a bordered grid, its header row in bold, that scrolls sideways
/// when it's wider than the space it's in.
pub fn md_table<'a>(p: Pal, t: Table) -> El<'a> {
    let Table { align, head, rows } = t;
    let cell = move |body: String, align: Align, bold: bool| -> El<'a> {
        let mut t = text(body)
            .size(14)
            .line_height(20.0 / 14.0)
            .align_x(horizontal(align));
        if bold {
            t = t.font(BOLD);
        }
        container(t).max_width(MAX_CELL_WIDTH).into()
    };
    let columns = head
        .into_iter()
        .zip(&align)
        .enumerate()
        .map(|(i, (h, &a))| {
            let rows = &rows;
            table::column(cell(h, a, true), move |r: usize| {
                cell(rows[r][i].clone(), a, false)
            })
            .align_x(horizontal(a))
        });
    let grid = table(columns, 0..rows.len()).padding_x(10).padding_y(6);
    // The table draws its lines in the theme's strong background color.
    let grid = themer(Some(theme::separator_theme(p)), grid);
    let boxed = container(grid).style(move |_| container::Style {
        border: Border {
            color: rgb(p.border),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    });
    let bar = Scrollbar::new().width(6).scroller_width(6).spacing(4);
    scrollable(boxed)
        .direction(Direction::Horizontal(bar))
        .width(Length::Fill)
        .style(move |_, s| theme::scroll_style(p, s))
        .into()
}

fn horizontal(align: Align) -> Horizontal {
    match align {
        Align::Left => Horizontal::Left,
        Align::Center => Horizontal::Center,
        Align::Right => Horizontal::Right,
    }
}
