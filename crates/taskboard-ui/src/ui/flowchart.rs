//! Mermaid diagrams: a ` ```mermaid ` block parsed by `crate::mermaid` (see
//! `docs/mermaid.md`), laid out, and drawn on an iced canvas in the
//! theme's colours. A flowchart that doesn't parse is shown as its error
//! over its source, and another kind of diagram as its source with a note;
//! never as nothing.
//!
//! [`diagram`] is the whole widget. It's generic over the message type and
//! keeps no state outside the canvas. Parsing, measuring and layout happen
//! once per source ([`prepare`]); the canvas caches its drawing until the
//! theme or the scale changes. A diagram wider than the space it's in is
//! scaled down, to [`MIN_SCALE`] at the least, and scrolls sideways past
//! that.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use iced::alignment::Vertical;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke};
use iced::widget::scrollable::{Direction as ScrollDirection, Scrollbar};
use iced::widget::{responsive, scrollable, text};
use iced::{Color, Element, Length, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::theme::{self, Pal, REGULAR, rgb};
use super::widgets::{LINE_HEIGHT, Note, source_note, text_width};
use crate::mermaid::layout::{self, EdgeIn, LOOP, Layout};
use crate::mermaid::{self, Diagram, Direction, Flowchart, MermaidError, Shape, Stroke as Line};

/// Node text.
const NODE_TEXT: f32 = 14.0;
/// Edge label text.
const EDGE_TEXT: f32 = 12.0;
/// Room around a node's text inside its shape.
const PAD_X: f32 = 14.0;
const PAD_Y: f32 = 9.0;
/// Room around an edge label's text, on its background.
const LABEL_PAD: f32 = 3.0;
/// Text wider than this wraps at its spaces.
const MAX_TEXT_WIDTH: f32 = 200.0;
/// Room around the whole diagram, for strokes and arrowheads.
const MARGIN: f32 = 6.0;
/// Arrowheads, whatever the stroke.
const ARROW_LEN: f32 = 9.0;
const ARROW_WIDTH: f32 = 8.0;
/// The inner lines of a subroutine, in from its sides.
const SUBROUTINE_INSET: f32 = 7.0;
/// Half the height of a cylinder's ends.
const CYLINDER_RY: f32 = 6.0;
/// The least a diagram is scaled by to fit before it scrolls instead.
pub const MIN_SCALE: f32 = 0.6;

/// The diagram `src` describes, or its source with its error or a note.
pub fn diagram<'a, M: 'a>(p: Pal, src: &str) -> Element<'a, M> {
    let prepared = prepare(src);
    let model = match &*prepared {
        Err(e) => return source_note(p, Note::Error, "Diagram error", &e.message, e.line, src),
        Ok(Prepared::Unsupported(kind)) => {
            let message =
                format!("Mermaid `{kind}` diagrams aren't supported; only flowcharts are drawn.");
            return source_note(p, Note::Info, "Diagram not drawn", &message, None, src);
        }
        Ok(Prepared::Flowchart(model)) => model.clone(),
    };
    if model.chart.nodes.is_empty() {
        return text("Empty flowchart").size(13).color(rgb(p.muted)).into();
    }
    responsive(move |space| fitted(p, model.clone(), space.width))
        .height(Length::Shrink)
        .into()
}

/// The diagram scaled to fit `width` as far as it can, scrolling past that.
fn fitted<'a, M: 'a>(p: Pal, model: Rc<Model>, width: f32) -> Element<'a, M> {
    let scale = fit_scale(model.width, width);
    let (w, h) = (model.width * scale, model.height * scale);
    let canvas = canvas::Canvas::new(Flow { model, p, scale })
        .width(w)
        .height(h);
    if w <= width + 0.5 {
        return canvas.into();
    }
    let bar = Scrollbar::new().width(6).scroller_width(6).spacing(4);
    scrollable(canvas)
        .direction(ScrollDirection::Horizontal(bar))
        .width(Length::Fill)
        .style(move |_, s| theme::scroll_style(p, s))
        .into()
}

/// What a diagram `natural` wide is scaled by in `space`.
fn fit_scale(natural: f32, space: f32) -> f32 {
    if natural <= 0.0 || !space.is_finite() {
        return 1.0;
    }
    (space / natural).clamp(MIN_SCALE, 1.0)
}

/// A parsed block, as [`diagram`] needs it.
enum Prepared {
    Flowchart(Rc<Model>),
    Unsupported(String),
}

/// How many parsed diagrams [`prepare`] keeps.
const PREPARED_DIAGRAMS: usize = 64;

thread_local! {
    /// Parsed and laid out diagrams by source, so redrawing a view doesn't
    /// lay them out again. Markdown is itself parsed on every view, so the
    /// work can't be kept on its blocks.
    static PREPARED: RefCell<HashMap<String, Rc<Result<Prepared, MermaidError>>>> =
        RefCell::new(HashMap::new());
}

/// `src` parsed and laid out, from the cache when it's been seen.
fn prepare(src: &str) -> Rc<Result<Prepared, MermaidError>> {
    PREPARED.with(|cache| {
        if let Some(hit) = cache.borrow().get(src) {
            return hit.clone();
        }
        let prepared = Rc::new(mermaid::parse(src).map(|d| match d {
            Diagram::Flowchart(f) => Prepared::Flowchart(Rc::new(Model::new(f))),
            Diagram::Unsupported(kind) => Prepared::Unsupported(kind),
        }));
        let mut cache = cache.borrow_mut();
        if cache.len() >= PREPARED_DIAGRAMS {
            cache.clear();
        }
        cache.insert(src.to_string(), prepared.clone());
        prepared
    })
}

/// Text broken into lines, measured.
#[derive(Debug, Clone, PartialEq)]
struct TextBlock {
    /// Each line and its width.
    lines: Vec<(String, f32)>,
    size: layout::Size,
    font_size: f32,
}

impl TextBlock {
    /// `s` at `font_size`, its lines wrapped at spaces to
    /// [`MAX_TEXT_WIDTH`].
    fn new(s: &str, font_size: f32) -> TextBlock {
        let measure = |t: &str| text_width(t, font_size, REGULAR);
        let mut lines = Vec::new();
        for line in s.lines() {
            let mut current = String::new();
            for word in line.split_whitespace() {
                let joined = if current.is_empty() {
                    word.to_string()
                } else {
                    format!("{current} {word}")
                };
                if !current.is_empty() && measure(&joined) > MAX_TEXT_WIDTH {
                    lines.push(std::mem::replace(&mut current, word.to_string()));
                } else {
                    current = joined;
                }
            }
            lines.push(current);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        let lines: Vec<(String, f32)> = lines
            .into_iter()
            .map(|l| {
                let w = measure(&l);
                (l, w)
            })
            .collect();
        let size = layout::Size {
            w: lines.iter().map(|l| l.1).fold(0.0, f32::max),
            h: lines.len() as f32 * font_size * LINE_HEIGHT,
        };
        TextBlock {
            lines,
            size,
            font_size,
        }
    }
}

/// A flowchart worked out into what's drawn: its text measured and its
/// layout made, in unscaled pixels.
#[derive(Debug)]
struct Model {
    chart: Flowchart,
    labels: Vec<TextBlock>,
    /// Each node's outline's size.
    sizes: Vec<layout::Size>,
    edge_labels: Vec<Option<TextBlock>>,
    layout: Layout,
    /// The canvas's size, margins included.
    width: f32,
    height: f32,
}

impl Model {
    fn new(chart: Flowchart) -> Model {
        let labels: Vec<TextBlock> = chart
            .nodes
            .iter()
            .map(|n| TextBlock::new(&n.label, NODE_TEXT))
            .collect();
        let sizes: Vec<layout::Size> = chart
            .nodes
            .iter()
            .zip(&labels)
            .map(|(n, t)| node_size(n.shape, t.size))
            .collect();
        let edge_labels: Vec<Option<TextBlock>> = chart
            .edges
            .iter()
            .map(|e| e.label.as_deref().map(|l| TextBlock::new(l, EDGE_TEXT)))
            .collect();
        let edges: Vec<EdgeIn> = chart
            .edges
            .iter()
            .zip(&edge_labels)
            .map(|(e, l)| EdgeIn {
                from: e.from,
                to: e.to,
                min_len: e.min_len,
                label: l.as_ref().map(|l| layout::Size {
                    w: l.size.w + 2.0 * LABEL_PAD,
                    h: l.size.h + 2.0 * LABEL_PAD,
                }),
            })
            .collect();
        let layout = layout::layout(chart.direction, &sizes, &edges);
        Model {
            width: layout.width + 2.0 * MARGIN,
            height: layout.height + 2.0 * MARGIN,
            chart,
            labels,
            sizes,
            edge_labels,
            layout,
        }
    }
}

/// The outline of a `shape` holding `text` sized text.
fn node_size(shape: Shape, text: layout::Size) -> layout::Size {
    let (tw, th) = (text.w, text.h);
    let (w, h) = (tw + 2.0 * PAD_X, th + 2.0 * PAD_Y);
    let (w, h) = match shape {
        Shape::Rect | Shape::Round => (w, h),
        Shape::Stadium => (tw + h + 4.0, h),
        Shape::Subroutine => (w + 2.0 * SUBROUTINE_INSET, h),
        Shape::Cylinder => (w, h + 2.0 * CYLINDER_RY),
        Shape::Circle => {
            let d = (tw * tw + th * th).sqrt() + 2.0 * PAD_Y;
            (d, d)
        }
        // Twice the text each way, so it fits inside the diagonals.
        Shape::Diamond => (2.0 * tw + 2.0 * PAD_Y, 2.0 * th + PAD_Y),
        Shape::Hexagon => (w + h / 2.0, h),
    };
    layout::Size {
        w: w.max(h.min(40.0)),
        h,
    }
}

/// Where the line from the centre `c` of a `shape` of `size` towards `to`
/// leaves its outline.
fn outline_point(shape: Shape, c: Point, size: layout::Size, to: Point) -> Point {
    let (dx, dy) = (to.x - c.x, to.y - c.y);
    if dx == 0.0 && dy == 0.0 {
        return c;
    }
    let (hw, hh) = (size.w / 2.0, size.h / 2.0);
    let t = match shape {
        Shape::Circle => hw / dx.hypot(dy),
        Shape::Diamond => 1.0 / (dx.abs() / hw + dy.abs() / hh),
        Shape::Hexagon => {
            let corners = polygon(shape, c, size);
            ray_polygon(c, dx, dy, &corners).unwrap_or(1.0)
        }
        _ => {
            let tx = if dx == 0.0 {
                f32::INFINITY
            } else {
                hw / dx.abs()
            };
            let ty = if dy == 0.0 {
                f32::INFINITY
            } else {
                hh / dy.abs()
            };
            tx.min(ty)
        }
    };
    Point::new(c.x + dx * t, c.y + dy * t)
}

/// The corners of a diamond or hexagon.
fn polygon(shape: Shape, c: Point, size: layout::Size) -> Vec<Point> {
    let (hw, hh) = (size.w / 2.0, size.h / 2.0);
    match shape {
        Shape::Diamond => vec![
            Point::new(c.x, c.y - hh),
            Point::new(c.x + hw, c.y),
            Point::new(c.x, c.y + hh),
            Point::new(c.x - hw, c.y),
        ],
        _ => {
            let inset = size.h / 4.0;
            vec![
                Point::new(c.x - hw + inset, c.y - hh),
                Point::new(c.x + hw - inset, c.y - hh),
                Point::new(c.x + hw, c.y),
                Point::new(c.x + hw - inset, c.y + hh),
                Point::new(c.x - hw + inset, c.y + hh),
                Point::new(c.x - hw, c.y),
            ]
        }
    }
}

/// The least `t` where `c + t (dx, dy)` crosses the edge of `corners`.
fn ray_polygon(c: Point, dx: f32, dy: f32, corners: &[Point]) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (i, a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let (ex, ey) = (b.x - a.x, b.y - a.y);
        let det = dx * ey - dy * ex;
        if det.abs() < 1e-6 {
            continue;
        }
        let (ax, ay) = (a.x - c.x, a.y - c.y);
        let t = (ax * ey - ay * ex) / det;
        let u = (ax * dy - ay * dx) / det;
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&u) {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    best
}

/// The canvas program: a laid out flowchart drawn at `scale` in a theme.
struct Flow {
    model: Rc<Model>,
    p: Pal,
    scale: f32,
}

/// A canvas's state: its drawing, and what it was drawn for.
#[derive(Default)]
struct State {
    cache: canvas::Cache,
    drawn: RefCell<Option<(Rc<Model>, Pal, f32)>>,
}

impl<M> canvas::Program<M> for Flow {
    type State = State;

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut drawn = state.drawn.borrow_mut();
        let same = drawn.as_ref().is_some_and(|(m, p, s)| {
            Rc::ptr_eq(m, &self.model) && *p == self.p && *s == self.scale
        });
        if !same {
            state.cache.clear();
            *drawn = Some((self.model.clone(), self.p, self.scale));
        }
        vec![state.cache.draw(renderer, bounds.size(), |frame| {
            self.paint(frame);
        })]
    }
}

/// An edge's line and its arrowheads, scaled, ready to draw.
struct Drawn {
    path: Path,
    stroke: Line,
    arrows: Vec<[Point; 3]>,
}

impl Flow {
    /// A point of the layout on the canvas.
    fn at(&self, p: layout::Point) -> Point {
        Point::new((p.x + MARGIN) * self.scale, (p.y + MARGIN) * self.scale)
    }

    fn paint(&self, frame: &mut Frame) {
        let m = &*self.model;
        let p = self.p;
        let s = self.scale;
        let muted = rgb(p.muted);

        // Edges, under the nodes they meet.
        for (i, edge) in m.chart.edges.iter().enumerate() {
            let Some(d) = self.edge(i) else { continue };
            let width = match edge.stroke {
                Line::Thick => 3.0,
                _ => 1.5,
            } * s;
            let mut stroke = Stroke::default()
                .with_color(muted)
                .with_width(width.max(1.0))
                .with_line_join(LineJoin::Round)
                .with_line_cap(LineCap::Round);
            if d.stroke == Line::Dotted {
                stroke.line_cap = LineCap::Butt;
                stroke.line_dash = LineDash {
                    segments: &[3.0, 3.0],
                    offset: 0,
                };
            }
            frame.stroke(&d.path, stroke);
            for tri in d.arrows {
                frame.fill(
                    &Path::new(|b| {
                        b.move_to(tri[0]);
                        b.line_to(tri[1]);
                        b.line_to(tri[2]);
                        b.close();
                    }),
                    muted,
                );
            }
        }

        // Nodes.
        let fill = rgb(p.card);
        let outline = Stroke::default()
            .with_color(rgb(p.border))
            .with_width((1.5 * s).max(1.0))
            .with_line_join(LineJoin::Round);
        for (i, node) in m.chart.nodes.iter().enumerate() {
            let c = self.at(m.layout.nodes[i]);
            let size = m.sizes[i];
            let size = Size::new(size.w * s, size.h * s);
            let (body, extra) = shape_paths(node.shape, c, size, s);
            frame.fill(&body, fill);
            frame.stroke(&body, outline);
            if let Some(extra) = extra {
                frame.stroke(&extra, outline);
            }
        }

        // Edge labels' backgrounds, over the edges.
        let label_bg = rgb(p.panel);
        for (route, label) in m.layout.edges.iter().zip(&m.edge_labels) {
            if let (Some(at), Some(label)) = (route.label, label) {
                let c = self.at(at);
                let (w, h) = (
                    (label.size.w + 2.0 * LABEL_PAD) * s,
                    (label.size.h + 2.0 * LABEL_PAD) * s,
                );
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(c.x - w / 2.0, c.y - h / 2.0),
                        Size::new(w, h),
                        (3.0 * s).into(),
                    ),
                    label_bg,
                );
            }
        }

        // Text last: a canvas draws all its text over its shapes anyway.
        let color = rgb(p.text);
        for (i, label) in m.labels.iter().enumerate() {
            self.text(frame, label, self.at(m.layout.nodes[i]), color);
        }
        for (route, label) in m.layout.edges.iter().zip(&m.edge_labels) {
            if let (Some(at), Some(label)) = (route.label, label) {
                self.text(frame, label, self.at(at), color);
            }
        }
    }

    /// `block` centred on `c`, each line placed from its top left: the
    /// software renderer leaves stale pixels behind centred text when the
    /// canvas scrolls.
    fn text(&self, frame: &mut Frame, block: &TextBlock, c: Point, color: Color) {
        let s = self.scale;
        let line_h = block.font_size * LINE_HEIGHT * s;
        let mut y = c.y - block.size.h * s / 2.0;
        for (line, w) in &block.lines {
            frame.fill_text(canvas::Text {
                content: line.clone(),
                position: Point::new(c.x - w * s / 2.0, y),
                color,
                size: Pixels(block.font_size * s),
                font: REGULAR,
                align_y: Vertical::Top,
                ..canvas::Text::default()
            });
            y += line_h;
        }
    }

    /// Edge `i`'s line from outline to outline, smoothed through its
    /// bends, with its arrowheads; `None` if its ends meet.
    fn edge(&self, i: usize) -> Option<Drawn> {
        let m = &*self.model;
        let edge = &m.chart.edges[i];
        let route = &m.layout.edges[i];
        let (from, to) = (edge.from, edge.to);
        let node = |v: usize| {
            let c = m.layout.nodes[v];
            (Point::new(c.x, c.y), m.chart.nodes[v].shape, m.sizes[v])
        };
        let mut pts: Vec<Point> = if from == to {
            self_loop(node(from), m.chart.direction)
        } else {
            let mut pts: Vec<Point> = route.points.iter().map(|p| Point::new(p.x, p.y)).collect();
            if pts.len() < 2 {
                return None;
            }
            let (c, shape, size) = node(from);
            pts[0] = outline_point(shape, c, size, pts[1]);
            let n = pts.len();
            let (c, shape, size) = node(to);
            pts[n - 1] = outline_point(shape, c, size, pts[n - 2]);
            pts
        };
        // Arrowheads, with the line stopping short under them.
        let mut arrows = Vec::new();
        let mut head = |pts: &mut Vec<Point>, end: bool| {
            let n = pts.len();
            let (tip, prev) = if end {
                (pts[n - 1], pts[n - 2])
            } else {
                (pts[0], pts[1])
            };
            let (dx, dy) = (tip.x - prev.x, tip.y - prev.y);
            let len = dx.hypot(dy);
            if len < 1e-3 {
                return;
            }
            let (ux, uy) = (dx / len, dy / len);
            let base = Point::new(tip.x - ux * ARROW_LEN, tip.y - uy * ARROW_LEN);
            let (nx, ny) = (-uy * ARROW_WIDTH / 2.0, ux * ARROW_WIDTH / 2.0);
            arrows.push([
                self.at_pt(tip),
                self.at_pt(Point::new(base.x + nx, base.y + ny)),
                self.at_pt(Point::new(base.x - nx, base.y - ny)),
            ]);
            let short = Point::new(tip.x - ux * ARROW_LEN * 0.8, tip.y - uy * ARROW_LEN * 0.8);
            if end {
                pts[n - 1] = short;
            } else {
                pts[0] = short;
            }
        };
        if edge.arrow_end {
            head(&mut pts, true);
        }
        if edge.arrow_start {
            head(&mut pts, false);
        }
        let pts: Vec<Point> = pts.into_iter().map(|p| self.at_pt(p)).collect();
        let path = if from == to {
            // A loop's four points are a cubic curve's.
            Path::new(|b| {
                b.move_to(pts[0]);
                b.bezier_curve_to(pts[1], pts[2], pts[3]);
            })
        } else {
            smooth(&pts)
        };
        Some(Drawn {
            path,
            stroke: edge.stroke,
            arrows,
        })
    }

    fn at_pt(&self, p: Point) -> Point {
        self.at(layout::Point { x: p.x, y: p.y })
    }
}

/// A line through `pts`, rounding each bend: straight to the middle of the
/// first leg, a curve round each bend to the middle of the next leg, and
/// straight to the end.
fn smooth(pts: &[Point]) -> Path {
    let mid = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    Path::new(|b| {
        b.move_to(pts[0]);
        let n = pts.len();
        if n > 2 {
            b.line_to(mid(pts[0], pts[1]));
            for i in 1..n - 1 {
                b.quadratic_curve_to(pts[i], mid(pts[i], pts[i + 1]));
            }
        }
        b.line_to(pts[n - 1]);
    })
}

/// A self-loop on a node, as a cubic curve's four points: out of its side
/// (the right going down the page, the bottom going across) and back.
fn self_loop((c, shape, size): (Point, Shape, layout::Size), dir: Direction) -> Vec<Point> {
    // `out` is away from the node; `along` runs down its side.
    let (out, along, half_out, half_along) = if dir.horizontal() {
        ((0.0, 1.0), (1.0, 0.0), size.h / 2.0, size.w / 2.0)
    } else {
        ((1.0, 0.0), (0.0, 1.0), size.w / 2.0, size.h / 2.0)
    };
    let spread = (half_along / 2.0).min(10.0);
    let side = |k: f32| {
        Point::new(
            c.x + out.0 * half_out + along.0 * k,
            c.y + out.1 * half_out + along.1 * k,
        )
    };
    let start = outline_point(shape, c, size, side(-spread));
    let end = outline_point(shape, c, size, side(spread));
    // A cubic with both controls `reach` out bulges three quarters of it.
    let reach = LOOP / 0.75;
    let control = |k: f32| {
        Point::new(
            c.x + out.0 * (half_out + reach) + along.0 * k,
            c.y + out.1 * (half_out + reach) + along.1 * k,
        )
    };
    vec![start, control(-spread * 1.6), control(spread * 1.6), end]
}

/// The outline of a node, and for a subroutine or cylinder the extra
/// lines drawn on it.
fn shape_paths(shape: Shape, c: Point, size: Size, scale: f32) -> (Path, Option<Path>) {
    let (w, h) = (size.width, size.height);
    let top_left = Point::new(c.x - w / 2.0, c.y - h / 2.0);
    match shape {
        Shape::Rect => (
            Path::rounded_rectangle(top_left, size, (2.0 * scale).into()),
            None,
        ),
        Shape::Round => (
            Path::rounded_rectangle(top_left, size, (8.0 * scale).min(h / 2.0).into()),
            None,
        ),
        Shape::Stadium => (
            Path::rounded_rectangle(top_left, size, (h / 2.0).into()),
            None,
        ),
        Shape::Subroutine => {
            let inset = SUBROUTINE_INSET * scale;
            let lines = Path::new(|b| {
                for x in [top_left.x + inset, top_left.x + w - inset] {
                    b.move_to(Point::new(x, top_left.y));
                    b.line_to(Point::new(x, top_left.y + h));
                }
            });
            (Path::rectangle(top_left, size), Some(lines))
        }
        Shape::Cylinder => {
            let ry = CYLINDER_RY * scale;
            let (l, r) = (top_left.x, top_left.x + w);
            let (top, bottom) = (top_left.y + ry, top_left.y + h - ry);
            // Half an ellipse of the cylinder's ends at `y`, from left to
            // right, bulging by `bulge`.
            let half = |b: &mut canvas::path::Builder, y: f32, bulge: f32, from_left: bool| {
                const STEPS: usize = 16;
                for k in 1..=STEPS {
                    let t = k as f32 / STEPS as f32 * std::f32::consts::PI;
                    let t = if from_left {
                        t
                    } else {
                        std::f32::consts::PI - t
                    };
                    let x = c.x - (w / 2.0) * t.cos();
                    b.line_to(Point::new(x, y + bulge * t.sin()));
                }
            };
            let body = Path::new(|b| {
                b.move_to(Point::new(l, top));
                b.line_to(Point::new(l, bottom));
                half(b, bottom, ry, true);
                b.line_to(Point::new(r, top));
                half(b, top, -ry, false);
                b.close();
            });
            let rim = Path::new(|b| {
                b.move_to(Point::new(l, top));
                half(b, top, ry, true);
            });
            (body, Some(rim))
        }
        Shape::Circle => (Path::circle(c, w / 2.0), None),
        Shape::Diamond | Shape::Hexagon => {
            let corners = polygon(
                shape,
                c,
                layout::Size {
                    w: size.width,
                    h: size.height,
                },
            );
            let path = Path::new(|b| {
                b.move_to(corners[0]);
                for &p in &corners[1..] {
                    b.line_to(p);
                }
                b.close();
            });
            (path, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str) -> Rc<Model> {
        match &*prepare(src) {
            Ok(Prepared::Flowchart(m)) => m.clone(),
            Ok(Prepared::Unsupported(k)) => panic!("unsupported {k}"),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn diagrams_are_prepared_once() {
        let src = "graph TD\nA-->B";
        let (a, b) = (prepare(src), prepare(src));
        assert!(Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn every_kind_of_block_builds() {
        let p = theme::palette(taskboard_core::settings::Theme::Light, false);
        for src in [
            "graph TD\nA[Start] --> B{Ok?}\nB -->|yes| C([Done])\nB -- no --> A\nC --> C",
            "graph LR\na((1)) <==> b{{2}} -.-> c[(3)] --- d[[4]] --> e(5)",
            "graph TD",
            "graph TD\nsubgraph x",
            "sequenceDiagram\nA->>B: hi",
            "",
        ] {
            let _: Element<'_, ()> = diagram(p, src);
        }
    }

    #[test]
    fn text_wraps_and_sizes_nodes() {
        let long = "a fairly long label that should wrap onto more than one line";
        let block = TextBlock::new(long, NODE_TEXT);
        assert!(block.lines.len() > 1, "{block:?}");
        assert!(block.lines.iter().all(|l| l.1 <= MAX_TEXT_WIDTH));
        let two = TextBlock::new("one\ntwo", NODE_TEXT);
        assert_eq!(two.lines.len(), 2);
        assert!(two.size.h > TextBlock::new("one", NODE_TEXT).size.h);
        assert_eq!(TextBlock::new("", NODE_TEXT).lines.len(), 1);

        // Every shape holds its text.
        let text = block.size;
        for shape in [
            Shape::Rect,
            Shape::Round,
            Shape::Stadium,
            Shape::Subroutine,
            Shape::Cylinder,
            Shape::Circle,
            Shape::Diamond,
            Shape::Hexagon,
        ] {
            let s = node_size(shape, text);
            assert!(s.w >= text.w && s.h >= text.h, "{shape:?}");
        }
    }

    #[test]
    fn edges_end_on_outlines() {
        let c = Point::new(100.0, 100.0);
        let size = layout::Size { w: 80.0, h: 40.0 };
        // Straight down, every shape stops at its bottom.
        for shape in [Shape::Rect, Shape::Diamond, Shape::Hexagon] {
            let p = outline_point(shape, c, size, Point::new(100.0, 300.0));
            assert!(
                (p.y - 120.0).abs() < 1e-3 && (p.x - 100.0).abs() < 1e-3,
                "{shape:?} {p:?}"
            );
        }
        // Diagonally, a diamond's edge is inside its box.
        let p = outline_point(Shape::Diamond, c, size, Point::new(200.0, 150.0));
        let on_edge = (p.x - c.x).abs() / 40.0 + (p.y - c.y).abs() / 20.0;
        assert!((on_edge - 1.0).abs() < 1e-3, "{p:?}");
        // A circle's at its radius.
        let size = layout::Size { w: 60.0, h: 60.0 };
        let p = outline_point(Shape::Circle, c, size, Point::new(200.0, 200.0));
        assert!(((p.x - c.x).hypot(p.y - c.y) - 30.0).abs() < 1e-3);
        // Sideways out of a hexagon, its point.
        let size = layout::Size { w: 80.0, h: 40.0 };
        let p = outline_point(Shape::Hexagon, c, size, Point::new(0.0, 100.0));
        assert!((p.x - 60.0).abs() < 1e-3, "{p:?}");
        // Towards its own centre, it stays put.
        assert_eq!(outline_point(Shape::Rect, c, size, c), c);
    }

    #[test]
    fn wide_diagrams_scale_then_scroll() {
        assert_eq!(fit_scale(300.0, 600.0), 1.0);
        assert_eq!(fit_scale(600.0, 480.0), 0.8);
        assert_eq!(fit_scale(1000.0, 300.0), MIN_SCALE);
        assert_eq!(fit_scale(0.0, 300.0), 1.0);
        assert_eq!(fit_scale(500.0, f32::INFINITY), 1.0);
    }

    #[test]
    fn model_fits_its_layout() {
        let m = model("graph LR\nA[One] -->|label| B((Two))\nB --> B");
        assert_eq!(m.labels.len(), 2);
        assert!(m.edge_labels[0].is_some() && m.edge_labels[1].is_none());
        assert_eq!(m.width, m.layout.width + 2.0 * MARGIN);
        // The circle is round.
        assert_eq!(m.sizes[1].w, m.sizes[1].h);
    }
}
