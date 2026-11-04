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
///
/// `t` needn't be tidy: it gets a column for each header cell, alignment or
/// cell of its widest row, with missing ones empty or left-aligned.
pub fn md_table<'a>(p: Pal, t: Table) -> El<'a> {
    let width = columns(&t);
    if width == 0 {
        // Iced's table can't lay out no columns.
        return iced::widget::Space::new().into();
    }
    let align = |i: usize| t.align.get(i).copied().unwrap_or(Align::Left);
    let at = |row: &[String], i: usize| row.get(i).cloned().unwrap_or_default();
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
    let rows = &t.rows;
    let columns = (0..width).map(|i| {
        let a = align(i);
        table::column(cell(at(&t.head, i), a, true), move |r: usize| {
            cell(at(&rows[r], i), a, false)
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

/// How many columns `t` is drawn with: enough for its header, its
/// alignments and its widest row.
fn columns(t: &Table) -> usize {
    t.rows
        .iter()
        .map(Vec::len)
        .chain([t.head.len(), t.align.len()])
        .max()
        .unwrap_or(0)
}

fn horizontal(align: Align) -> Horizontal {
    match align {
        Align::Left => Horizontal::Left,
        Align::Center => Horizontal::Center,
        Align::Right => Horizontal::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_cover_the_widest_part() {
        let row = |n: usize| vec![String::new(); n];
        let t = Table {
            align: vec![Align::Right],
            head: row(2),
            rows: vec![row(1), row(4)],
        };
        assert_eq!(columns(&t), 4);
        assert_eq!(columns(&Table::default()), 0);
    }

    #[test]
    fn builds_ragged_tables() {
        // Building calls each column's view on every row: short rows and a
        // missing alignment mustn't panic, nor an empty table.
        let p = theme::palette(taskboard_core::settings::Theme::Dark, false);
        let t = Table {
            align: Vec::new(),
            head: vec!["a".into(), "b".into()],
            rows: vec![vec![], vec!["x".into(), "y".into(), "z".into()]],
        };
        let _ = md_table(p, t);
        let _ = md_table(p, Table::default());
    }
}
