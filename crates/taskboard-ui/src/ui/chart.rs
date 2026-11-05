//! Charts: a parsed chart spec (`crate::chart_spec`, see `docs/charts.md`)
//! drawn on an iced canvas, with axes, gridlines and a tooltip for the mark
//! nearest the pointer, over a legend of its series. A spec that doesn't parse is shown as its
//! error over its source, never as nothing.
//!
//! [`chart`] is the whole widget; it's generic over the message type and
//! keeps no state outside the canvas, so markdown and dashboards can both
//! use it. The maths (scales, ticks, which mark is nearest) is plain
//! functions, tested below; [`Chart::draw`] only paints what they work out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use iced::alignment::Vertical;
use iced::widget::canvas::{self, Frame, Geometry, LineJoin, Path, Stroke};
use iced::widget::{Space, column, container, row, stack, text};
use iced::{
    Alignment, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Renderer, Size,
    Theme, mouse,
};

use super::theme::{self, BOLD, Pal, REGULAR, rgb};
use super::widgets::{LINE_HEIGHT, Note, source_note, text_width};
use crate::chart_spec::{self, Channel, ChartError, Datum, FieldType, Mark, Spec};

/// Plot height when the spec doesn't give one.
const DEFAULT_PLOT_HEIGHT: f32 = 220.0;
/// Tick labels, legend entries and the tooltip.
const SMALL: f32 = 12.0;
const TITLE_SIZE: f32 = 14.0;
/// Room above the plot for the title.
const TITLE_HEIGHT: f32 = 26.0;
/// Room above the plot for the y axis's title.
const Y_TITLE_HEIGHT: f32 = 20.0;
/// Room under the plot: tick, label, axis title.
const X_AXIS_HEIGHT: f32 = 44.0;
const TICK: f32 = 4.0;
/// Widest a category label gets before it's cut short.
const MAX_LABEL: f32 = 140.0;
/// Room right of the plot, for the last x tick label to stick out into.
const RIGHT_MARGIN: f32 = 12.0;
/// Most ticks a numeric axis gets, whatever its numbers.
const MAX_TICKS: f64 = 200.0;
/// Pixels between ticks on each axis, at least.
const X_TICK_SPACING: f32 = 80.0;
const Y_TICK_SPACING: f32 = 40.0;
/// How far from a bar or point the pointer can be and still hover it.
const HOVER_REACH: f32 = 24.0;

/// The chart `src` describes, or its error and source if it's invalid.
pub fn chart<'a, M: 'a>(p: Pal, src: &str) -> Element<'a, M> {
    let prepared = prepare(src);
    let prepared = match &*prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            return source_note(p, Note::Error, "Chart error", &e.message, e.line, src);
        }
    };
    let (width, height) = (prepared.width, prepared.height);
    let model = prepared.model.clone();
    let layer = |hover| {
        canvas::Canvas::new(Chart {
            model: model.clone(),
            p,
            hover,
        })
        .width(width)
        .height(height)
    };
    // The hover layer is a canvas of its own on top, since text in a canvas
    // is drawn over all its shapes, and the tooltip has to cover the axis
    // labels. A fixed width wider than the space there is is cut to fit.
    let plot = stack![layer(false), layer(true)]
        .width(width)
        .height(height);
    let mut body = column![plot].spacing(6);
    if let Some(legend) = legend(p, &model) {
        body = body.push(legend);
    }
    container(body).width(Length::Fill).into()
}

/// A parsed chart and its size, as [`chart`] needs it.
struct Prepared {
    model: Rc<Model>,
    width: Length,
    height: f32,
}

/// How many parsed charts [`prepare`] keeps.
const PREPARED_CHARTS: usize = 64;

thread_local! {
    /// Parsed charts by source, so redrawing a view doesn't parse its
    /// charts again. Markdown is itself parsed on every view, so the parse
    /// can't be kept on its blocks.
    static PREPARED: RefCell<HashMap<String, Rc<Result<Prepared, ChartError>>>> =
        RefCell::new(HashMap::new());
}

/// `src` parsed and worked out, from the cache when it's been seen.
fn prepare(src: &str) -> Rc<Result<Prepared, ChartError>> {
    PREPARED.with(|cache| {
        if let Some(hit) = cache.borrow().get(src) {
            return hit.clone();
        }
        let parsed = Rc::new(chart_spec::parse(src).map(|spec| {
            let model = Model::new(&spec);
            let plot_h = spec.height.unwrap_or(DEFAULT_PLOT_HEIGHT);
            let width = match spec.width {
                Some(w) => {
                    let (_, ticks) = axis_ticks(&model.y.domain, plot_h, Dir::Y);
                    Length::Fixed(left_margin(&ticks, MAX_LABEL) + w + RIGHT_MARGIN)
                }
                None => Length::Fill,
            };
            Prepared {
                height: model.top() + plot_h + X_AXIS_HEIGHT,
                model: Rc::new(model),
                width,
            }
        }));
        let mut cache = cache.borrow_mut();
        if cache.len() >= PREPARED_CHARTS {
            cache.clear();
        }
        cache.insert(src.to_string(), parsed.clone());
        parsed
    })
}

/// The colour legend under the chart, wrapping onto more lines as needed,
/// when there's a colour channel.
fn legend<'a, M: 'a>(p: Pal, model: &Model) -> Option<Element<'a, M>> {
    let (title, names) = model.legend.as_ref()?;
    let title: Element<'a, M> = text(title.clone())
        .size(SMALL)
        .font(BOLD)
        .color(rgb(p.text))
        .into();
    let entries = names.iter().enumerate().map(|(i, name)| -> Element<'a, M> {
        let color = p.series[i % p.series.len()];
        let swatch = container(Space::new())
            .width(10)
            .height(10)
            .style(move |_| theme::fill(color, 2.0));
        row![swatch, text(name.clone()).size(SMALL).color(rgb(p.text))]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
    });
    Some(
        row(std::iter::once(title).chain(entries))
            .spacing(14)
            .align_y(Alignment::Center)
            .wrap()
            .into(),
    )
}

/// Where a value sits along an axis, before it's scaled to pixels: a number
/// (times are seconds since 1970) or a category's index.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pos {
    Num(f64),
    Cat(usize),
}

impl Pos {
    /// For sorting along an axis.
    fn key(self) -> f64 {
        match self {
            Pos::Num(v) => v,
            Pos::Cat(i) => i as f64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    X,
    Y,
}

impl Dir {
    fn other(self) -> Dir {
        match self {
            Dir::X => Dir::Y,
            Dir::Y => Dir::X,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Domain {
    /// Numbers, or times when `time`, from `lo` to `hi`.
    Linear { lo: f64, hi: f64, time: bool },
    /// Categories, in the order they first appear in the data.
    Band(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
struct Axis {
    title: String,
    domain: Domain,
}

/// One drawn mark: a bar, a point, or a vertex of a line or area.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    x: Pos,
    y: Pos,
    series: usize,
    /// The tooltip's lines: each channel's title and this row's value.
    tip: Vec<(String, String)>,
}

/// A spec worked out into what's drawn, independent of the chart's size.
#[derive(Debug, Clone, PartialEq)]
struct Model {
    mark: Mark,
    title: Option<String>,
    x: Axis,
    y: Axis,
    /// The axis the measured value is on: bars grow along it, areas fill
    /// from its zero, and lines run along the other one.
    value: Dir,
    /// The colour legend's title and series names, when there's a colour.
    legend: Option<(String, Vec<String>)>,
    items: Vec<Item>,
    /// For bars on a numeric or time base axis: the smallest gap between
    /// two bars' positions, which sets how wide they can be.
    base_gap: Option<f64>,
}

impl Model {
    fn new(spec: &Spec) -> Model {
        let enc = &spec.encoding;
        let value =
            if enc.y.kind == FieldType::Quantitative || enc.x.kind != FieldType::Quantitative {
                Dir::Y
            } else {
                Dir::X
            };
        let base = value.other();
        let mut x_cats = Vec::new();
        let mut y_cats = Vec::new();
        let mut series: Vec<String> = Vec::new();
        let mut items = Vec::new();
        for row in &spec.data.values {
            let known = (x_cats.len(), y_cats.len());
            let (Some(x), Some(y)) = (
                position(&enc.x, row.get(&enc.x.field), &mut x_cats),
                position(&enc.y, row.get(&enc.y.field), &mut y_cats),
            ) else {
                // A row that isn't drawn adds no categories.
                x_cats.truncate(known.0);
                y_cats.truncate(known.1);
                continue;
            };
            let mut tip = vec![
                (enc.x.title().to_string(), tip_value(row.get(&enc.x.field))),
                (enc.y.title().to_string(), tip_value(row.get(&enc.y.field))),
            ];
            let series = match &enc.color {
                Some(c) => {
                    let name = tip_value(row.get(&c.field));
                    tip.push((c.title().to_string(), name.clone()));
                    index_of(&mut series, name)
                }
                None => 0,
            };
            items.push(Item { x, y, series, tip });
        }

        let zero = matches!(spec.mark, Mark::Bar | Mark::Area);
        let base_gap = if spec.mark == Mark::Bar {
            min_gap(items.iter().map(|i| at(i, base)))
        } else {
            None
        };
        let axis = |dir: Dir, channel: &Channel, cats: Vec<String>| {
            let domain = match channel.kind {
                FieldType::Nominal | FieldType::Ordinal => Domain::Band(cats),
                kind => {
                    let time = kind == FieldType::Temporal;
                    let values = items.iter().filter_map(|i| match at(i, dir) {
                        Pos::Num(v) => Some(v),
                        Pos::Cat(_) => None,
                    });
                    let (mut lo, mut hi) = extent(values, zero && dir == value);
                    // Bars at the ends need room for their width.
                    if dir == base && spec.mark == Mark::Bar {
                        let pad = base_gap.unwrap_or(if time { 86_400.0 } else { 1.0 }) / 2.0;
                        lo -= pad;
                        hi += pad;
                    }
                    if lo == hi && zero && dir == value {
                        // Every value is 0.
                        hi = 1.0;
                    } else if lo == hi {
                        let pad = if time {
                            86_400.0
                        } else {
                            lo.abs().max(1.0) / 2.0
                        };
                        lo -= pad;
                        hi += pad;
                    }
                    Domain::Linear { lo, hi, time }
                }
            };
            Axis {
                title: channel.title().to_string(),
                domain,
            }
        };
        Model {
            mark: spec.mark,
            title: spec.title.clone(),
            x: axis(Dir::X, &enc.x, x_cats),
            y: axis(Dir::Y, &enc.y, y_cats),
            value,
            legend: enc.color.as_ref().map(|c| (c.title().to_string(), series)),
            items,
            base_gap,
        }
    }

    fn base(&self) -> Dir {
        self.value.other()
    }

    fn series_count(&self) -> usize {
        self.legend.as_ref().map_or(1, |(_, s)| s.len().max(1))
    }

    /// The space above the plot.
    fn top(&self) -> f32 {
        let title = if self.title.is_some() {
            TITLE_HEIGHT
        } else {
            0.0
        };
        // The y axis's title, then room for the half of a tick label that
        // sticks out above the top tick.
        title + Y_TITLE_HEIGHT + 8.0
    }
}

/// The space left of the plot for the y axis with `ticks`, their labels
/// cut to `cap`.
fn left_margin(ticks: &[(Pos, String)], cap: f32) -> f32 {
    let widest = ticks
        .iter()
        .map(|(_, l)| text_width(l, SMALL, REGULAR).min(cap))
        .fold(0.0, f32::max);
    // Labels, gap, tick.
    4.0 + widest + 4.0 + TICK
}

/// A row's value as a position, adding a category to `cats` when it's new.
/// None when the row has no value for the channel.
fn position(channel: &Channel, value: Option<&Datum>, cats: &mut Vec<String>) -> Option<Pos> {
    let value = value.filter(|v| **v != Datum::Null)?;
    match channel.kind {
        FieldType::Quantitative => value.as_number().map(Pos::Num),
        FieldType::Temporal => value
            .as_time()
            .map(|t| Pos::Num(t.and_utc().timestamp_millis() as f64 / 1000.0)),
        FieldType::Nominal | FieldType::Ordinal => Some(Pos::Cat(index_of(cats, value.label()))),
    }
}

fn index_of(list: &mut Vec<String>, name: String) -> usize {
    match list.iter().position(|n| *n == name) {
        Some(i) => i,
        None => {
            list.push(name);
            list.len() - 1
        }
    }
}

fn tip_value(value: Option<&Datum>) -> String {
    match value {
        None | Some(Datum::Null) => "(none)".to_string(),
        Some(v) => v.label(),
    }
}

fn at(item: &Item, dir: Dir) -> Pos {
    match dir {
        Dir::X => item.x,
        Dir::Y => item.y,
    }
}

/// The least and greatest of `values`, with 0 when `zero`.
fn extent(values: impl Iterator<Item = f64>, zero: bool) -> (f64, f64) {
    let start = if zero {
        (0.0, 0.0)
    } else {
        (f64::INFINITY, f64::NEG_INFINITY)
    };
    let (lo, hi) = values.fold(start, |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo > hi { (0.0, 0.0) } else { (lo, hi) }
}

/// The smallest gap between distinct numeric positions, if there are two.
fn min_gap(positions: impl Iterator<Item = Pos>) -> Option<f64> {
    let mut v: Vec<f64> = positions
        .filter_map(|p| match p {
            Pos::Num(v) => Some(v),
            Pos::Cat(_) => None,
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v.windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .min_by(f64::total_cmp)
}

/// Round tick values from `lo` to `hi`, about `count` of them at most, a
/// step of 1, 2 or 5 times a power of ten apart, with the step. Numbers too
/// far apart to step between (or that would need more than [`MAX_TICKS`])
/// get just their two ends.
fn nice_ticks(lo: f64, hi: f64, count: usize) -> (Vec<f64>, f64) {
    let ends = || (vec![lo, hi], hi - lo);
    let step = nice_step((hi - lo) / count.max(1) as f64);
    let first = (lo / step - 1e-9).ceil();
    let last = (hi / step + 1e-9).floor();
    if !(first.is_finite() && last.is_finite() && last - first <= MAX_TICKS) {
        return ends();
    }
    let ticks = (first as i64..=last as i64)
        .map(|i| clean(i as f64 * step, step))
        .collect();
    (ticks, step)
}

/// The smallest of 1, 2, 5 or 10 times a power of ten that's at least `raw`.
fn nice_step(raw: f64) -> f64 {
    if !(raw > 0.0 && raw.is_finite()) {
        return 1.0;
    }
    let power = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|s| *s >= raw * (1.0 - 1e-9))
        .unwrap_or(10.0 * power);
    clean(step, step)
}

/// `v`, a multiple of `step`, without float noise like 0.30000000000000004:
/// rounded to a millionth of the step's power of ten.
fn clean(v: f64, step: f64) -> f64 {
    let scale = 10f64.powf(6.0 - step.abs().log10().floor());
    if !(scale.is_finite() && scale > 0.0) {
        return v;
    }
    let rounded = (v * scale).round() / scale;
    if !rounded.is_finite() {
        v
    } else if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// `lo..hi` widened to whole `step`s.
fn nice_domain(lo: f64, hi: f64, step: f64) -> (f64, f64) {
    (
        clean((lo / step + 1e-9).floor() * step, step),
        clean((hi / step - 1e-9).ceil() * step, step),
    )
}

/// The power of ten `v` is in: 2 for 250, -3 for 0.005.
fn exponent(v: f64) -> i32 {
    v.abs().log10().floor() as i32
}

/// How many digits after the point `v`'s mantissa needs: 0 for 2e5, 1 for
/// 2.5e5.
fn mantissa_decimals(v: f64) -> i32 {
    let mantissa = v.abs() / 10f64.powi(exponent(v));
    (0..6)
        .find(|d| {
            let scaled = mantissa * 10f64.powi(*d);
            (scaled - scaled.round()).abs() < 1e-6 * scaled
        })
        .unwrap_or(6)
}

/// Labels for numeric ticks `step` apart: as many decimals as the step
/// needs, thousands as k, M and B once they're 10 000 or more, and
/// scientific notation for the very big and the very small.
fn number_labels(ticks: &[f64], step: f64) -> Vec<String> {
    let biggest = ticks.iter().fold(0.0f64, |m, t| m.max(t.abs()));
    if !(step.is_finite() && step > 0.0 && biggest.is_finite()) {
        return ticks.iter().map(|t| format!("{t:e}")).collect();
    }
    let digits = mantissa_decimals(step) - exponent(step);
    if biggest >= 1e12 || digits > 6 {
        // Enough digits after the point to tell the ticks apart.
        let top = if biggest > 0.0 {
            exponent(biggest)
        } else {
            exponent(step)
        };
        let decimals = (top + digits).clamp(0, 12) as usize;
        return ticks
            .iter()
            .map(|t| {
                if *t == 0.0 {
                    "0".to_string()
                } else {
                    format!("{:.*e}", decimals, t)
                }
            })
            .collect();
    }
    let (div, suffix) = match biggest {
        b if b >= 1e9 => (1e9, "B"),
        b if b >= 1e6 => (1e6, "M"),
        b if b >= 1e4 => (1e3, "k"),
        _ => (1.0, ""),
    };
    let step = step / div;
    let decimals = (mantissa_decimals(step) - exponent(step)).max(0) as usize;
    ticks
        .iter()
        .map(|t| {
            let s = format!("{:.*}", decimals, t / div);
            let s = if s.trim_start_matches(['-', '0', '.']).is_empty() {
                format!("{:.*}", decimals, 0.0)
            } else {
                s
            };
            format!("{s}{suffix}")
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeUnit {
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl TimeUnit {
    fn seconds(self) -> f64 {
        match self {
            TimeUnit::Second => 1.0,
            TimeUnit::Minute => 60.0,
            TimeUnit::Hour => 3600.0,
            TimeUnit::Day => 86_400.0,
            TimeUnit::Month => 30.44 * 86_400.0,
            TimeUnit::Year => 365.25 * 86_400.0,
        }
    }
}

/// Tick intervals for time axes, shortest first.
const TIME_STEPS: &[(TimeUnit, i64)] = &[
    (TimeUnit::Second, 1),
    (TimeUnit::Second, 5),
    (TimeUnit::Second, 15),
    (TimeUnit::Second, 30),
    (TimeUnit::Minute, 1),
    (TimeUnit::Minute, 5),
    (TimeUnit::Minute, 15),
    (TimeUnit::Minute, 30),
    (TimeUnit::Hour, 1),
    (TimeUnit::Hour, 3),
    (TimeUnit::Hour, 6),
    (TimeUnit::Hour, 12),
    (TimeUnit::Day, 1),
    (TimeUnit::Day, 2),
    (TimeUnit::Day, 7),
    (TimeUnit::Month, 1),
    (TimeUnit::Month, 3),
    (TimeUnit::Month, 6),
    (TimeUnit::Year, 1),
    (TimeUnit::Year, 2),
    (TimeUnit::Year, 5),
    (TimeUnit::Year, 10),
    (TimeUnit::Year, 25),
    (TimeUnit::Year, 50),
    (TimeUnit::Year, 100),
];

fn to_time(secs: f64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp_millis((secs * 1000.0).round() as i64).map(|t| t.naive_utc())
}

fn to_secs(t: NaiveDateTime) -> f64 {
    t.and_utc().timestamp_millis() as f64 / 1000.0
}

/// Ticks on calendar boundaries between times `lo` and `hi` (seconds), at
/// most about `count`, labelled to suit their interval.
fn time_ticks(lo: f64, hi: f64, count: usize) -> Vec<(f64, String)> {
    let span = hi - lo;
    let &(unit, k) = TIME_STEPS
        .iter()
        .find(|(u, k)| span / (u.seconds() * *k as f64) <= count.max(1) as f64)
        .unwrap_or(TIME_STEPS.last().unwrap());
    let (Some(start), Some(_)) = (to_time(lo), to_time(hi)) else {
        return Vec::new();
    };
    let mut times = Vec::new();
    match unit {
        TimeUnit::Month | TimeUnit::Year => {
            let months = if unit == TimeUnit::Year { 12 * k } else { k };
            // Months since year 0, from the first month start at or after lo.
            let mut m = start.year() as i64 * 12 + start.month0() as i64;
            let at_start = start.day() == 1 && start.time() == NaiveTime::MIN;
            if !at_start {
                m += 1;
            }
            m += (months - m.rem_euclid(months)) % months;
            let month_start = |m: i64| {
                NaiveDate::from_ymd_opt(m.div_euclid(12) as i32, m.rem_euclid(12) as u32 + 1, 1)
            };
            while let Some(t) = month_start(m) {
                let t = t.and_time(NaiveTime::MIN);
                if to_secs(t) > hi + 1e-6 {
                    break;
                }
                times.push(t);
                m += months;
            }
        }
        _ => {
            let step = unit.seconds() * k as f64;
            let mut t = (lo / step - 1e-9).ceil() * step;
            while t <= hi + 1e-6 {
                if let Some(time) = to_time(t) {
                    times.push(time);
                }
                t += step;
            }
        }
    }
    times
        .into_iter()
        .map(|t| {
            let midnight = t.time() == NaiveTime::MIN;
            let label = match unit {
                TimeUnit::Second => t.format("%H:%M:%S").to_string(),
                TimeUnit::Minute | TimeUnit::Hour if !midnight => t.format("%H:%M").to_string(),
                TimeUnit::Minute | TimeUnit::Hour | TimeUnit::Day => t.format("%b %-d").to_string(),
                TimeUnit::Month => t.format("%b %Y").to_string(),
                TimeUnit::Year => t.year().to_string(),
            };
            (to_secs(t), label)
        })
        .collect()
}

/// An axis's domain fitted to its ticks, and the ticks, for an axis `len`
/// pixels long. Category axes tick each category.
fn axis_ticks(domain: &Domain, len: f32, dir: Dir) -> (Domain, Vec<(Pos, String)>) {
    let spacing = match dir {
        Dir::X => X_TICK_SPACING,
        Dir::Y => Y_TICK_SPACING,
    };
    let count = ((len / spacing).floor() as usize).max(2);
    match domain {
        Domain::Band(cats) => (
            domain.clone(),
            cats.iter()
                .enumerate()
                .map(|(i, c)| (Pos::Cat(i), c.clone()))
                .collect(),
        ),
        Domain::Linear { lo, hi, time: true } => (
            domain.clone(),
            time_ticks(*lo, *hi, count)
                .into_iter()
                .map(|(t, l)| (Pos::Num(t), l))
                .collect(),
        ),
        Domain::Linear {
            lo,
            hi,
            time: false,
        } => {
            let (_, step) = nice_ticks(*lo, *hi, count);
            let (lo, hi) = match nice_domain(*lo, *hi, step) {
                (nlo, nhi) if nlo.is_finite() && nhi.is_finite() && nhi > nlo => (nlo, nhi),
                _ => (*lo, *hi),
            };
            let (ticks, step) = nice_ticks(lo, hi, count);
            let labels = number_labels(&ticks, step);
            (
                Domain::Linear {
                    lo,
                    hi,
                    time: false,
                },
                ticks.into_iter().map(Pos::Num).zip(labels).collect(),
            )
        }
    }
}

/// Maps positions on an axis to pixels: `start` is where the domain's low
/// end (or the first category) goes, `end` the high end (or the last).
#[derive(Debug, Clone, PartialEq)]
struct Scale {
    domain: Domain,
    start: f32,
    end: f32,
}

impl Scale {
    fn at(&self, pos: Pos) -> f32 {
        match (&self.domain, pos) {
            (Domain::Linear { lo, hi, .. }, Pos::Num(v)) => {
                // Halved, so values near f64::MAX don't overflow.
                let t = if hi > lo {
                    (v / 2.0 - lo / 2.0) / (hi / 2.0 - lo / 2.0)
                } else {
                    0.5
                };
                self.start + (self.end - self.start) * t as f32
            }
            (Domain::Band(cats), Pos::Cat(i)) => {
                self.start + (self.end - self.start) * (i as f32 + 0.5) / cats.len().max(1) as f32
            }
            // Can't happen: positions come from the axis's own channel.
            _ => self.start,
        }
    }

    /// A category's width in pixels; 0 on a numeric axis.
    fn band(&self) -> f32 {
        match &self.domain {
            Domain::Band(cats) => (self.end - self.start).abs() / cats.len().max(1) as f32,
            Domain::Linear { .. } => 0.0,
        }
    }

    /// Pixels per unit of the domain.
    fn per_unit(&self) -> f32 {
        match &self.domain {
            Domain::Linear { lo, hi, .. } if hi > lo => {
                ((self.end - self.start) as f64 / (hi / 2.0 - lo / 2.0) / 2.0).abs() as f32
            }
            _ => 0.0,
        }
    }

    /// The pixel for 0, kept inside the axis.
    fn zero(&self) -> f32 {
        let z = self.at(Pos::Num(0.0));
        z.clamp(self.start.min(self.end), self.start.max(self.end))
    }
}

/// Where everything goes in a chart of a given size.
struct Layout {
    plot: Rectangle,
    /// How wide a y tick label can be.
    label_cap: f32,
    x: Scale,
    y: Scale,
    x_ticks: Vec<(f32, String)>,
    y_ticks: Vec<(f32, String)>,
    /// Each item's mark: a bar's rectangle, or a point with no size.
    marks: Vec<Rectangle>,
}

impl Layout {
    fn new(model: &Model, size: Size) -> Layout {
        let top = model.top();
        let plot_h = (size.height - top - X_AXIS_HEIGHT).max(10.0);
        let (y_domain, y_ticks) = axis_ticks(&model.y.domain, plot_h, Dir::Y);
        // Long category names get less of a narrow chart.
        let label_cap = (size.width * 0.3).clamp(30.0, MAX_LABEL);
        let left = left_margin(&y_ticks, label_cap);
        let plot = Rectangle {
            x: left,
            y: top,
            width: (size.width - left - RIGHT_MARGIN).max(10.0),
            height: plot_h,
        };
        let (x_domain, x_ticks) = axis_ticks(&model.x.domain, plot.width, Dir::X);
        let x = Scale {
            domain: x_domain,
            start: plot.x,
            end: plot.x + plot.width,
        };
        // Numbers go up; categories read down from the top.
        let (start, end) = match y_domain {
            Domain::Band(_) => (plot.y, plot.y + plot.height),
            Domain::Linear { .. } => (plot.y + plot.height, plot.y),
        };
        let y = Scale {
            domain: y_domain,
            start,
            end,
        };
        let x_ticks = x_ticks.into_iter().map(|(p, l)| (x.at(p), l)).collect();
        let y_ticks = y_ticks.into_iter().map(|(p, l)| (y.at(p), l)).collect();
        let marks = marks(model, &x, &y);
        Layout {
            plot,
            label_cap,
            x,
            y,
            x_ticks,
            y_ticks,
            marks,
        }
    }

    fn scale(&self, dir: Dir) -> &Scale {
        match dir {
            Dir::X => &self.x,
            Dir::Y => &self.y,
        }
    }

    /// The item the pointer at `cursor` is on or nearest, if any: along the
    /// line for lines and areas, otherwise within reach of a bar or point.
    fn hovered(&self, model: &Model, cursor: Point) -> Option<usize> {
        let reach = self.plot.expand(HOVER_REACH);
        if !reach.contains(cursor) {
            return None;
        }
        let snap = match model.mark {
            Mark::Line | Mark::Area if self.plot.contains(cursor) => Snap::Along(model.base()),
            Mark::Line | Mark::Area => return None,
            Mark::Bar | Mark::Point => Snap::Within(HOVER_REACH),
        };
        nearest(&self.marks, cursor, snap)
    }
}

/// Each item's mark, in pixels.
fn marks(model: &Model, x: &Scale, y: &Scale) -> Vec<Rectangle> {
    let point = |item: &Item| Rectangle::new(Point::new(x.at(item.x), y.at(item.y)), Size::ZERO);
    if model.mark != Mark::Bar {
        return model.items.iter().map(point).collect();
    }
    let base = model.base();
    let (base_scale, value_scale) = match base {
        Dir::X => (x, y),
        Dir::Y => (y, x),
    };
    let group = if base_scale.band() > 0.0 {
        base_scale.band() * 0.8
    } else {
        match model.base_gap {
            Some(gap) => gap as f32 * base_scale.per_unit() * 0.8,
            None => 40.0,
        }
    }
    .clamp(2.0, 120.0 * model.series_count() as f32);
    let n = model.series_count();
    let slot = group / n as f32;
    // A 1px gap between neighbouring bars, when there's room for one.
    let thick = if slot > 4.0 { slot - 1.0 } else { slot };
    let zero = value_scale.zero();
    model
        .items
        .iter()
        .map(|item| {
            let offset = (item.series as f32 - (n as f32 - 1.0) / 2.0) * slot;
            let center = base_scale.at(at(item, base)) + offset;
            let v = value_scale.at(at(item, model.value));
            let (b0, len_b) = (center - thick / 2.0, thick);
            let (v0, len_v) = (v.min(zero), (v - zero).abs());
            match base {
                Dir::X => Rectangle::new(Point::new(b0, v0), Size::new(len_b, len_v)),
                Dir::Y => Rectangle::new(Point::new(v0, b0), Size::new(len_v, len_b)),
            }
        })
        .collect()
}

/// How the pointer picks a mark.
#[derive(Debug, Clone, Copy)]
enum Snap {
    /// The mark nearest along this axis, then nearest across it.
    Along(Dir),
    /// The nearest mark no further than this.
    Within(f32),
}

/// The index of the mark in `marks` the pointer at `p` picks.
fn nearest(marks: &[Rectangle], p: Point, snap: Snap) -> Option<usize> {
    let distance = |r: &Rectangle| -> (f32, f32) {
        let dx = (r.x - p.x).max(p.x - (r.x + r.width)).max(0.0);
        let dy = (r.y - p.y).max(p.y - (r.y + r.height)).max(0.0);
        match snap {
            Snap::Along(Dir::X) => (dx, dy),
            Snap::Along(Dir::Y) => (dy, dx),
            Snap::Within(_) => (dx.hypot(dy), 0.0),
        }
    };
    let (i, (d, _)) = marks
        .iter()
        .map(distance)
        .enumerate()
        .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))?;
    match snap {
        Snap::Within(reach) if d > reach => None,
        _ => Some(i),
    }
}

/// `s` cut short with an ellipsis to fit in `width`.
fn fit(s: &str, width: f32, size: f32) -> String {
    if text_width(s, size, REGULAR) <= width {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| chars[..n].iter().collect::<String>() + "…";
    // The most characters that fit with the ellipsis: between `lo` (fits,
    // or none) and `hi` (doesn't).
    let (mut lo, mut hi) = (0, chars.len());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if text_width(&cut(mid), size, REGULAR) <= width {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lo == 0 && text_width("…", size, REGULAR) > width {
        String::new()
    } else {
        cut(lo)
    }
}

/// The canvas program: a model, drawn in a theme. The chart is two of
/// them, the chart itself and over it, when `hover`, the hovered mark and
/// its tooltip.
struct Chart {
    model: Rc<Model>,
    p: Pal,
    hover: bool,
}

/// A canvas's state: what it last laid out and drew, and for the hover
/// layer, the item under the pointer.
#[derive(Default)]
struct State {
    hover: Option<usize>,
    /// The base layer's drawing, until the layout changes.
    cache: canvas::Cache,
    layout: RefCell<Option<Laid>>,
}

/// A layout, and what it was made for.
struct Laid {
    model: Rc<Model>,
    p: Pal,
    size: Size,
    layout: Rc<Layout>,
}

impl State {
    /// `chart`'s layout at `size`, kept until either changes. Clears the
    /// drawing when it's made again.
    fn layout(&self, chart: &Chart, size: Size) -> Rc<Layout> {
        let mut laid = self.layout.borrow_mut();
        if let Some(l) = &*laid
            && Rc::ptr_eq(&l.model, &chart.model)
            && l.p == chart.p
            && l.size == size
        {
            return l.layout.clone();
        }
        self.cache.clear();
        let layout = Rc::new(Layout::new(&chart.model, size));
        *laid = Some(Laid {
            model: chart.model.clone(),
            p: chart.p,
            size,
            layout: layout.clone(),
        });
        layout
    }
}

impl<M> canvas::Program<M> for Chart {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<M>> {
        if !self.hover
            || !matches!(
                event,
                Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft)
            )
        {
            return None;
        }
        let hovered = match cursor.position_in(bounds) {
            // Outside, nothing's hovered, and there's nothing to work out.
            None if state.hover.is_none() => return None,
            None => None,
            Some(at) => state.layout(self, bounds.size()).hovered(&self.model, at),
        };
        if hovered == state.hover {
            return None;
        }
        state.hover = hovered;
        Some(canvas::Action::request_redraw())
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let size = bounds.size();
        if !self.hover {
            let layout = state.layout(self, size);
            return vec![state.cache.draw(renderer, size, |frame| {
                self.draw_axes(frame, &layout);
                self.draw_marks(frame, &layout);
            })];
        }
        let Some((i, at)) = state.hover.zip(cursor.position_in(bounds)) else {
            return Vec::new();
        };
        if i >= self.model.items.len() {
            return Vec::new();
        }
        let layout = state.layout(self, size);
        let mut frame = Frame::new(renderer, size);
        self.draw_hover(&mut frame, &layout, i, at, size);
        vec![frame.into_geometry()]
    }
}

impl Chart {
    fn color(&self, series: usize) -> Color {
        rgb(self.p.series[series % self.p.series.len()])
    }

    fn draw_axes(&self, frame: &mut Frame, l: &Layout) {
        let p = self.p;
        let plot = l.plot;
        if let Some(title) = &self.model.title {
            frame.fill_text(label(
                title,
                Point::new(0.0, 2.0),
                TITLE_SIZE,
                BOLD,
                rgb(p.text),
            ));
        }
        let grid = Stroke::default()
            .with_color(Color {
                a: 0.6,
                ..rgb(p.border)
            })
            .with_width(1.0);
        let axis = Stroke::default().with_color(rgb(p.border)).with_width(1.0);
        let muted = rgb(p.muted);
        let bottom = plot.y + plot.height;

        // Gridlines across numeric axes only; categories need none.
        let x_numeric = matches!(l.x.domain, Domain::Linear { .. });
        let y_numeric = matches!(l.y.domain, Domain::Linear { .. });
        for (px, _) in &l.x_ticks {
            if x_numeric {
                frame.stroke(
                    &Path::line(Point::new(*px, plot.y), Point::new(*px, bottom)),
                    grid,
                );
            }
        }
        for (py, _) in &l.y_ticks {
            if y_numeric {
                let right = plot.x + plot.width;
                frame.stroke(
                    &Path::line(Point::new(plot.x, *py), Point::new(right, *py)),
                    grid,
                );
            }
        }
        frame.stroke(
            &Path::line(
                Point::new(plot.x, bottom),
                Point::new(plot.x + plot.width, bottom),
            ),
            axis,
        );
        frame.stroke(
            &Path::line(Point::new(plot.x, plot.y), Point::new(plot.x, bottom)),
            axis,
        );

        // X ticks and labels, each cut to its share of the axis.
        let room = match l.x.band() {
            b if b > 0.0 => b - 4.0,
            _ => X_TICK_SPACING - 8.0,
        };
        for (px, text) in &l.x_ticks {
            frame.stroke(
                &Path::line(Point::new(*px, bottom), Point::new(*px, bottom + TICK)),
                axis,
            );
            let t = label(
                &fit(text, room, SMALL),
                Point::new(*px, bottom + TICK + 3.0),
                SMALL,
                REGULAR,
                muted,
            );
            // The end labels are kept inside the chart.
            let mut t = place(t, 0.5, 0.0);
            let w = text_width(&t.content, SMALL, REGULAR);
            t.position.x = t.position.x.min(frame.width() - w).max(0.0);
            frame.fill_text(t);
        }
        let t = label(
            &self.model.x.title,
            Point::new(plot.x + plot.width / 2.0, bottom + X_AXIS_HEIGHT - 2.0),
            SMALL,
            BOLD,
            muted,
        );
        frame.fill_text(place(t, 0.5, 1.0));

        for (py, text) in &l.y_ticks {
            frame.stroke(
                &Path::line(Point::new(plot.x - TICK, *py), Point::new(plot.x, *py)),
                axis,
            );
            let t = label(
                &fit(text, l.label_cap, SMALL),
                Point::new(plot.x - TICK - 4.0, *py),
                SMALL,
                REGULAR,
                muted,
            );
            frame.fill_text(place(t, 1.0, 0.5));
        }
        // The y title above its axis, level, so it reads without turning
        // the head (and the software renderer redraws turned text badly).
        let t = label(
            &self.model.y.title,
            Point::new(0.0, plot.y - 8.0),
            SMALL,
            BOLD,
            muted,
        );
        frame.fill_text(place(t, 0.0, 1.0));
    }

    fn draw_marks(&self, frame: &mut Frame, l: &Layout) {
        let model = &self.model;
        let bg = rgb(self.p.bg);
        match model.mark {
            Mark::Bar => {
                for (item, r) in model.items.iter().zip(&l.marks) {
                    let radius = (r.width.min(r.height) / 2.0).min(2.0);
                    frame.fill(
                        &Path::rounded_rectangle(r.position(), r.size(), radius.into()),
                        self.color(item.series),
                    );
                }
            }
            Mark::Point => {
                for (item, r) in model.items.iter().zip(&l.marks) {
                    frame.fill(&Path::circle(r.center(), 5.0), bg);
                    frame.fill(&Path::circle(r.center(), 4.0), self.color(item.series));
                }
            }
            Mark::Line | Mark::Area => {
                let base = model.base();
                let several = model.series_count() > 1;
                for s in 0..model.series_count() {
                    let mut pts: Vec<(f64, Point)> = model
                        .items
                        .iter()
                        .zip(&l.marks)
                        .filter(|(i, _)| i.series == s)
                        .map(|(i, r)| (at(i, base).key(), r.center()))
                        .collect();
                    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let color = self.color(s);
                    if pts.len() == 1 {
                        frame.fill(&Path::circle(pts[0].1, 3.0), color);
                        continue;
                    }
                    let Some(&(_, first)) = pts.first() else {
                        continue;
                    };
                    let last = pts[pts.len() - 1].1;
                    if model.mark == Mark::Area {
                        let zero = l.scale(model.value).zero();
                        let foot = |p: Point| match model.value {
                            Dir::Y => Point::new(p.x, zero),
                            Dir::X => Point::new(zero, p.y),
                        };
                        let area = Path::new(|b| {
                            b.move_to(foot(first));
                            for (_, p) in &pts {
                                b.line_to(*p);
                            }
                            b.line_to(foot(last));
                            b.close();
                        });
                        let alpha = if several { 0.22 } else { 0.3 };
                        frame.fill(&area, Color { a: alpha, ..color });
                    }
                    let line = Path::new(|b| {
                        b.move_to(first);
                        for (_, p) in &pts[1..] {
                            b.line_to(*p);
                        }
                    });
                    frame.stroke(
                        &line,
                        Stroke::default()
                            .with_color(color)
                            .with_width(2.0)
                            .with_line_join(LineJoin::Round),
                    );
                }
            }
        }
    }

    /// Marks item `i` and shows its values in a box by the pointer.
    fn draw_hover(&self, frame: &mut Frame, l: &Layout, i: usize, cursor: Point, size: Size) {
        let p = self.p;
        let item = &self.model.items[i];
        let mark = l.marks[i];
        let color = self.color(item.series);
        if self.model.mark == Mark::Bar {
            frame.stroke(
                &Path::rectangle(mark.position(), mark.size()),
                Stroke::default().with_color(rgb(p.text)).with_width(1.5),
            );
        } else {
            let c = mark.center();
            if matches!(self.model.mark, Mark::Line | Mark::Area) {
                // A guide across the plot through the point. It's a filled
                // rectangle rather than a stroked line: the software
                // renderer finds what to redraw from a shape's bounds, and
                // a straight line's are zero wide.
                let (at, size) = match self.model.value {
                    Dir::Y => (
                        Point::new(c.x - 0.5, l.plot.y),
                        Size::new(1.0, l.plot.height),
                    ),
                    Dir::X => (
                        Point::new(l.plot.x, c.y - 0.5),
                        Size::new(l.plot.width, 1.0),
                    ),
                };
                frame.fill_rectangle(at, size, rgb(p.muted));
            }
            frame.fill(&Path::circle(c, 6.0), rgb(p.bg));
            frame.fill(&Path::circle(c, 4.5), color);
        }

        let lines: Vec<(String, String)> = item
            .tip
            .iter()
            .map(|(k, v)| (format!("{k}: "), v.clone()))
            .collect();
        let key_w = lines
            .iter()
            .map(|(k, _)| text_width(k, SMALL, REGULAR))
            .fold(0.0, f32::max);
        let val_w = lines
            .iter()
            .map(|(_, v)| text_width(v, SMALL, BOLD))
            .fold(0.0, f32::max);
        let pad = 8.0;
        let line_h = 17.0;
        // A little slack: measured widths can come up a pixel or two short.
        let w = key_w + val_w + 2.0 * pad + 4.0;
        let h = lines.len() as f32 * line_h + 2.0 * pad - 2.0;
        // Below right of the pointer, or flipped to stay in the chart.
        let mut x = cursor.x + 14.0;
        if x + w > size.width {
            x = cursor.x - 14.0 - w;
        }
        let mut y = cursor.y + 14.0;
        if y + h > size.height {
            y = cursor.y - 14.0 - h;
        }
        let x = x.max(0.0);
        let y = y.max(0.0);
        let rect = Path::rounded_rectangle(Point::new(x, y), Size::new(w, h), 6.0.into());
        frame.fill(&rect, rgb(p.card));
        frame.stroke(
            &rect,
            Stroke::default().with_color(rgb(p.border)).with_width(1.0),
        );
        for (n, (k, v)) in lines.iter().enumerate() {
            let ty = y + pad + n as f32 * line_h;
            frame.fill_text(label(
                k,
                Point::new(x + pad, ty),
                SMALL,
                REGULAR,
                rgb(p.muted),
            ));
            frame.fill_text(label(
                v,
                Point::new(x + pad + key_w, ty),
                SMALL,
                BOLD,
                rgb(p.text),
            ));
        }
    }
}

/// `t` moved so `at` is `fx` of the way across it and `fy` of the way down.
///
/// Text is always drawn from its top left, rather than with iced's
/// alignment: the software renderer redraws a text's area from its position
/// rightwards and down, so text aligned to its right or bottom would leave
/// stale pixels when the chart scrolls.
fn place(mut t: canvas::Text, fx: f32, fy: f32) -> canvas::Text {
    t.position.x -= text_width(&t.content, t.size.0, t.font) * fx;
    t.position.y -= t.size.0 * LINE_HEIGHT * fy;
    t
}

/// Text with its top left at `at`.
fn label(content: &str, at: Point, size: f32, font: Font, color: Color) -> canvas::Text {
    canvas::Text {
        content: content.to_string(),
        position: at,
        color,
        size: Pixels(size),
        font,
        align_y: Vertical::Top,
        ..canvas::Text::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str) -> Model {
        Model::new(&chart_spec::parse(src).unwrap())
    }

    #[test]
    fn nice_ticks_are_round() {
        assert_eq!(nice_ticks(0.0, 15.0, 5), (vec![0.0, 5.0, 10.0, 15.0], 5.0));
        assert_eq!(
            nice_ticks(0.0, 1.0, 5).0,
            vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
        );
        assert_eq!(nice_ticks(-3.0, 7.5, 6).0, vec![-2.0, 0.0, 2.0, 4.0, 6.0]);
        assert_eq!(nice_domain(9.5, 15.0, 2.0), (8.0, 16.0));
        assert_eq!(nice_step(0.3), 0.5);
        assert_eq!(nice_step(0.0), 1.0);
    }

    #[test]
    fn number_labels_fit_their_step() {
        assert_eq!(number_labels(&[0.0, 0.5, 1.0], 0.5), ["0.0", "0.5", "1.0"]);
        assert_eq!(number_labels(&[-5.0, 0.0, 5.0], 5.0), ["-5", "0", "5"]);
        assert_eq!(
            number_labels(&[0.0, 20_000.0, 40_000.0], 20_000.0),
            ["0k", "20k", "40k"]
        );
        assert_eq!(
            number_labels(&[0.0, 2.5e6, 5e6], 2.5e6),
            ["0.0M", "2.5M", "5.0M"]
        );
        assert_eq!(number_labels(&[0.25, 0.5], 0.25), ["0.25", "0.50"]);
    }

    #[test]
    fn time_ticks_land_on_calendar_boundaries() {
        let secs = |s: &str| to_secs(Datum::Text(s.into()).as_time().unwrap());
        let days = time_ticks(secs("2024-03-01"), secs("2024-03-03"), 2);
        assert_eq!(
            days,
            [
                (secs("2024-03-01"), "Mar 1".to_string()),
                (secs("2024-03-02"), "Mar 2".into()),
                (secs("2024-03-03"), "Mar 3".into()),
            ]
        );
        let months = time_ticks(secs("2023-11-15"), secs("2024-08-01"), 4);
        let labels: Vec<_> = months.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["Jan 2024", "Apr 2024", "Jul 2024"]);
        let years = time_ticks(secs("2019-06"), secs("2024"), 5);
        let labels: Vec<_> = years.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["2020", "2021", "2022", "2023", "2024"]);
    }

    #[test]
    fn scales_map_domains_to_pixels() {
        let linear = Scale {
            domain: Domain::Linear {
                lo: 0.0,
                hi: 10.0,
                time: false,
            },
            start: 200.0,
            end: 0.0,
        };
        assert_eq!(linear.at(Pos::Num(2.5)), 150.0);
        assert_eq!(linear.zero(), 200.0);
        assert_eq!(linear.per_unit(), 20.0);
        let band = Scale {
            domain: Domain::Band(vec!["a".into(), "b".into()]),
            start: 0.0,
            end: 100.0,
        };
        assert_eq!(band.at(Pos::Cat(0)), 25.0);
        assert_eq!(band.at(Pos::Cat(1)), 75.0);
        assert_eq!(band.band(), 50.0);
    }

    #[test]
    fn model_keeps_data_order_and_zero() {
        let m = model(
            "mark: bar\nencoding:\n  x: {field: q, type: ordinal}\n  y: {field: v, type: quantitative}\n  color: {field: r, type: nominal}\ndata:\n  values:\n    - {q: Q2, r: north, v: 12}\n    - {q: Q1, r: south, v: 9.5}\n    - {q: Q1, v: 3}\n    - {q: Q3, r: north}\n",
        );
        assert_eq!(m.x.domain, Domain::Band(vec!["Q2".into(), "Q1".into()]));
        // Bars start at zero; the row without a y is left out.
        assert_eq!(
            m.y.domain,
            Domain::Linear {
                lo: 0.0,
                hi: 12.0,
                time: false
            }
        );
        assert_eq!(m.items.len(), 3);
        assert_eq!(m.value, Dir::Y);
        let (_, series) = m.legend.unwrap();
        assert_eq!(series, ["north", "south", "(none)"]);
        assert_eq!(m.items[1].tip[1], ("v".to_string(), "9.5".to_string()));
    }

    #[test]
    fn horizontal_bars_grow_along_x() {
        let m = model(
            "mark: bar\nencoding:\n  x: {field: n, type: quantitative}\n  y: {field: s, type: nominal}\ndata:\n  values:\n    - {s: a, n: 4}\n    - {s: b, n: 2}\n",
        );
        assert_eq!(m.value, Dir::X);
        let l = Layout::new(&m, Size::new(400.0, 200.0));
        // Bar "a" is on top and twice as long as "b".
        assert!(l.marks[0].y < l.marks[1].y);
        assert!((l.marks[0].width - 2.0 * l.marks[1].width).abs() < 0.5);
        assert_eq!(l.marks[0].x, l.plot.x);
    }

    /// A spec with `y` values `values`, on a line.
    fn line(values: &[&str]) -> String {
        let rows: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(i, v)| format!("    - {{x: {i}, y: {v}}}\n"))
            .collect();
        format!(
            "mark: line\nencoding:\n  x: {{field: x, type: quantitative}}\n  y: {{field: y, type: quantitative}}\ndata:\n  values:\n{}",
            rows.concat()
        )
    }

    #[test]
    fn extreme_ranges_lay_out() {
        // A span past f64::MAX gets its two ends, rather than panicking.
        let (ticks, _) = nice_ticks(-1.5e308, 1.5e308, 5);
        assert_eq!(ticks, [-1.5e308, 1.5e308]);
        for values in [
            &["-1.5e308", "1.5e308"][..],
            &["1e-10", "9e-10"],
            &["0", "3e-9"],
            &["1e300", "2e300"],
        ] {
            let m = model(&line(values));
            let l = Layout::new(&m, Size::new(400.0, 300.0));
            let Domain::Linear { lo, hi, .. } = l.y.domain else {
                panic!("y should be numeric");
            };
            assert!(
                lo.is_finite() && hi.is_finite() && hi > lo,
                "{values:?}: {lo}..{hi}"
            );
            assert!(l.marks.iter().all(|r| r.y.is_finite()), "{values:?}");
            // Every tick has its own label.
            let mut labels: Vec<_> = l.y_ticks.iter().map(|(_, t)| t.clone()).collect();
            assert!(labels.len() >= 2, "{values:?}: {labels:?}");
            labels.dedup();
            assert_eq!(labels.len(), l.y_ticks.len(), "{values:?}: {labels:?}");
        }
    }

    #[test]
    fn tiny_and_huge_labels() {
        let (ticks, step) = nice_ticks(0.0, 3e-9, 3);
        assert_eq!(number_labels(&ticks, step), ["0", "1e-9", "2e-9", "3e-9"]);
        let (ticks, step) = nice_ticks(1e300, 2e300, 2);
        assert_eq!(
            number_labels(&ticks, step),
            ["1.0e300", "1.5e300", "2.0e300"]
        );
    }

    #[test]
    fn narrow_charts_stay_in_bounds() {
        let m = model(
            "mark: bar\nencoding:\n  x: {field: n, type: quantitative}\n  y: {field: s, type: nominal}\n  color: {field: s, type: nominal}\ndata:\n  values:\n    - {s: a very long category name indeed, n: 4}\n    - {s: b, n: 2}\n",
        );
        let l = Layout::new(&m, Size::new(200.0, 200.0));
        assert!(l.plot.x + l.plot.width + RIGHT_MARGIN <= 200.0);
        assert!(l.plot.width > 100.0);
    }

    #[test]
    fn fit_cuts_to_width() {
        let full = "a very long category name";
        let width = text_width("a very…", SMALL, REGULAR);
        assert_eq!(fit(full, width, SMALL), "a very…");
        assert_eq!(fit(full, 1000.0, SMALL), full);
        assert_eq!(fit(full, 1.0, SMALL), "");
    }

    #[test]
    fn charts_are_parsed_once() {
        let src = line(&["1", "2"]);
        let (a, b) = (prepare(&src), prepare(&src));
        assert!(Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn nearest_mark() {
        let pt = |x, y| Rectangle::new(Point::new(x, y), Size::ZERO);
        let marks = [
            pt(10.0, 10.0),
            pt(50.0, 90.0),
            Rectangle::new(Point::new(100.0, 0.0), Size::new(20.0, 50.0)),
        ];
        // Inside a bar.
        assert_eq!(
            nearest(&marks, Point::new(110.0, 20.0), Snap::Within(5.0)),
            Some(2)
        );
        assert_eq!(
            nearest(&marks, Point::new(14.0, 13.0), Snap::Within(10.0)),
            Some(0)
        );
        assert_eq!(
            nearest(&marks, Point::new(30.0, 50.0), Snap::Within(10.0)),
            None
        );
        // Along x, the nearest x wins however far off in y.
        assert_eq!(
            nearest(&marks, Point::new(45.0, 0.0), Snap::Along(Dir::X)),
            Some(1)
        );
        assert_eq!(nearest(&[], Point::ORIGIN, Snap::Along(Dir::X)), None);
    }
}
