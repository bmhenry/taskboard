//! Mermaid diagrams in markdown (see `docs/mermaid.md`): a strict parser for
//! a subset of mermaid's flowchart syntax, and in [`layout`] where its nodes
//! and edges go. Pure, without iced, like `chart_spec`; `ui::flowchart`
//! draws the result.
//!
//! Other diagram types aren't errors: they parse to
//! [`Diagram::Unsupported`], which the viewer shows as source with a note.
//! Mermaid syntax this subset doesn't draw, like subgraphs or `:::class`,
//! is an error naming its line, rather than being drawn wrongly.

pub mod layout;

use std::collections::HashMap;
use std::fmt;

/// The most nodes a flowchart can have, to keep its layout quick.
pub const MAX_NODES: usize = 150;
/// The most edges a flowchart can have.
pub const MAX_EDGES: usize = 300;

/// Which way the ranks of a flowchart run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

impl Direction {
    /// Whether ranks run across the page, so the layout is transposed.
    pub fn horizontal(self) -> bool {
        matches!(self, Direction::LeftRight | Direction::RightLeft)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `A[text]`, and a bare `A`.
    Rect,
    /// `A(text)`
    Round,
    /// `A([text])`
    Stadium,
    /// `A[[text]]`
    Subroutine,
    /// `A[(text)]`
    Cylinder,
    /// `A((text))`
    Circle,
    /// `A{text}`
    Diamond,
    /// `A{{text}}`
    Hexagon,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    /// Its text, with `<br>` turned into line breaks. The id when the node
    /// is never given one.
    pub label: String,
    pub shape: Shape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// `-->`, `---`
    Solid,
    /// `==>`, `===`
    Thick,
    /// `-.->`, `-.-`
    Dotted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Indices into [`Flowchart::nodes`].
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub stroke: Stroke,
    /// An arrowhead at `from`, as in `<-->`.
    pub arrow_start: bool,
    /// An arrowhead at `to`.
    pub arrow_end: bool,
    /// The fewest ranks the edge spans: 1, plus one for each extra dash,
    /// `=` or `.` in the link.
    pub min_len: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flowchart {
    pub direction: Direction,
    /// In the order they're first mentioned.
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Why a diagram was rejected. `line` is 1-based, when the problem has one.
#[derive(Debug, Clone, PartialEq)]
pub struct MermaidError {
    pub message: String,
    pub line: Option<usize>,
}

impl fmt::Display for MermaidError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MermaidError {}

/// What a ` ```mermaid ` block holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Diagram {
    Flowchart(Flowchart),
    /// Another diagram type, named by its header (`sequenceDiagram`, ...):
    /// shown as source with a note, not as an error.
    Unsupported(String),
}

/// Parses a ` ```mermaid ` block.
pub fn parse(src: &str) -> Result<Diagram, MermaidError> {
    let mut lines = src
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        // `%%` comments and `%%{init: ...}%%` directives take whole lines.
        .filter(|(_, l)| !l.is_empty() && !l.starts_with("%%"));
    let Some((header_line, header)) = lines.next() else {
        return Err(MermaidError {
            message: "the diagram is empty: start it with `flowchart TD` or `graph LR`".into(),
            line: None,
        });
    };
    let at = |line: usize| {
        move |message: String| MermaidError {
            message,
            line: Some(line),
        }
    };
    let kind_end = header
        .find(|c: char| c.is_whitespace() || c == ';')
        .unwrap_or(header.len());
    let kind = &header[..kind_end];
    if kind != "graph" && kind != "flowchart" {
        return Ok(Diagram::Unsupported(kind.to_string()));
    }
    let mut cur = Cursor::new(&header[kind_end..]);
    cur.skip_ws();
    let word = cur.take_while(|c| !c.is_whitespace() && c != ';');
    let direction = match word {
        "" | "TD" | "TB" => Direction::TopDown,
        "BT" => Direction::BottomUp,
        "LR" => Direction::LeftRight,
        "RL" => Direction::RightLeft,
        d => {
            return Err(at(header_line)(format!(
                "unknown direction `{d}`: use TD, TB, BT, LR or RL"
            )));
        }
    };
    let mut b = Builder {
        chart: Flowchart {
            direction,
            nodes: Vec::new(),
            edges: Vec::new(),
        },
        ids: HashMap::new(),
    };
    // The header line can go on with `;` and statements.
    cur.skip_ws();
    if !cur.done() && !cur.eat(";") {
        let near = cur.near();
        return Err(at(header_line)(format!(
            "unexpected `{near}` after the direction"
        )));
    }
    b.statements(cur).map_err(at(header_line))?;
    // Inside a multi-line `accDescr { ... }`.
    let mut in_description = false;
    for (line, text) in lines {
        if in_description {
            in_description = !text.contains('}');
            continue;
        }
        if keyword(text) == Some("accDescr") && text.contains('{') && !text.contains('}') {
            in_description = true;
            continue;
        }
        b.statements(Cursor::new(text)).map_err(at(line))?;
    }
    Ok(Diagram::Flowchart(b.chart))
}

/// Statements that only style or annotate a diagram: accepted and skipped,
/// since its colours come from the theme.
const IGNORED: &[&str] = &[
    "classDef",
    "class",
    "style",
    "linkStyle",
    "click",
    "accTitle",
    "accDescr",
];

/// Mermaid statements this subset can't draw.
const UNSUPPORTED: &[&str] = &["subgraph", "end", "direction"];

/// The word a statement starts with, when it's a keyword's shape: letters
/// followed by a space, `:`, `{` or nothing.
fn keyword(s: &str) -> Option<&str> {
    let end = s
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(s.len());
    let next = s[end..].chars().next();
    (end > 0 && matches!(next, None | Some(' ' | '\t' | ':' | '{' | ';'))).then(|| &s[..end])
}

struct Builder {
    chart: Flowchart,
    ids: HashMap<String, usize>,
}

/// A link between nodes, as written.
struct Link {
    label: Option<String>,
    stroke: Stroke,
    arrow_start: bool,
    arrow_end: bool,
    min_len: usize,
}

impl Builder {
    /// The `;`-separated statements in `cur`.
    fn statements(&mut self, mut cur: Cursor) -> Result<(), String> {
        loop {
            cur.skip_ws();
            while cur.eat(";") {
                cur.skip_ws();
            }
            if cur.done() {
                return Ok(());
            }
            self.statement(&mut cur)?;
            cur.skip_ws();
            if !cur.done() && !cur.eat(";") {
                return Err(cur.expected("a link (`-->`, `---`, `-.->`, `==>`)"));
            }
        }
    }

    fn statement(&mut self, cur: &mut Cursor) -> Result<(), String> {
        if let Some(word) = keyword(cur.rest()) {
            if IGNORED.contains(&word) {
                cur.take_while(|c| c != ';');
                return Ok(());
            }
            if UNSUPPORTED.contains(&word) {
                return Err(format!(
                    "`{word}` isn't supported: only nodes and links are drawn"
                ));
            }
        }
        let mut from = self.node_group(cur)?;
        loop {
            cur.skip_ws();
            if cur.done() || cur.rest().starts_with(';') {
                return Ok(());
            }
            let link = cur.link()?;
            cur.skip_ws();
            if cur.done() || cur.rest().starts_with(';') {
                return Err("the link doesn't lead to a node".into());
            }
            let to = self.node_group(cur)?;
            for &a in &from {
                for &b in &to {
                    if self.chart.edges.len() == MAX_EDGES {
                        return Err(format!(
                            "too many links: a flowchart can have at most {MAX_EDGES}"
                        ));
                    }
                    self.chart.edges.push(Edge {
                        from: a,
                        to: b,
                        label: link.label.clone(),
                        stroke: link.stroke,
                        arrow_start: link.arrow_start,
                        arrow_end: link.arrow_end,
                        min_len: link.min_len,
                    });
                }
            }
            from = to;
        }
    }

    /// `A`, `A[text]`, `A & B[text] & C`.
    fn node_group(&mut self, cur: &mut Cursor) -> Result<Vec<usize>, String> {
        let mut out = vec![self.node(cur)?];
        loop {
            let save = cur.i;
            cur.skip_ws();
            if cur.eat("&") {
                cur.skip_ws();
                out.push(self.node(cur)?);
            } else {
                cur.i = save;
                return Ok(out);
            }
        }
    }

    fn node(&mut self, cur: &mut Cursor) -> Result<usize, String> {
        let id = cur.ident().ok_or_else(|| cur.expected("a node id"))?;
        if cur.rest().starts_with(":::") {
            return Err("`:::class` isn't supported: colours come from the theme".into());
        }
        let shape = cur.shape()?;
        let i = match self.ids.get(id) {
            Some(&i) => i,
            None => {
                if self.chart.nodes.len() == MAX_NODES {
                    return Err(format!(
                        "too many nodes: a flowchart can have at most {MAX_NODES}"
                    ));
                }
                self.chart.nodes.push(Node {
                    id: id.to_string(),
                    label: id.to_string(),
                    shape: Shape::Rect,
                });
                self.ids.insert(id.to_string(), self.chart.nodes.len() - 1);
                self.chart.nodes.len() - 1
            }
        };
        if let Some((shape, label)) = shape {
            let node = &mut self.chart.nodes[i];
            node.shape = shape;
            node.label = label;
        }
        Ok(i)
    }
}

/// Node shapes by their brackets, longest first so `((` isn't read as `(`.
const SHAPES: &[(&str, &str, Shape)] = &[
    ("([", "])", Shape::Stadium),
    ("[[", "]]", Shape::Subroutine),
    ("[(", ")]", Shape::Cylinder),
    ("((", "))", Shape::Circle),
    ("{{", "}}", Shape::Hexagon),
    ("[", "]", Shape::Rect),
    ("(", ")", Shape::Round),
    ("{", "}", Shape::Diamond),
];

/// Openers of mermaid's other shapes, which are errors.
const OTHER_SHAPES: &[&str] = &["(((", "[/", "[\\", ">", "@{"];

/// A position in a line.
struct Cursor<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Cursor { s, i: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn skip_ws(&mut self) {
        self.take_while(char::is_whitespace);
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> &'a str {
        let r = self.rest();
        let end = r.find(|c| !f(c)).unwrap_or(r.len());
        self.i += end;
        &r[..end]
    }

    fn eat(&mut self, t: &str) -> bool {
        let found = self.rest().starts_with(t);
        if found {
            self.i += t.len();
        }
        found
    }

    /// The next few characters, for messages.
    fn near(&self) -> String {
        let mut near: String = self.rest().chars().take(12).collect();
        if self.rest().chars().nth(12).is_some() {
            near.push('…');
        }
        near
    }

    fn expected(&self, what: &str) -> String {
        if self.done() {
            format!("expected {what} at the end of the line")
        } else {
            format!("expected {what} at `{}`", self.near())
        }
    }

    /// A node id: letters, digits, `_` and `-`. A `-` that starts a link
    /// (`--`, `-.`, `->`) ends it.
    fn ident(&mut self) -> Option<&'a str> {
        let r = self.rest();
        let mut end = 0;
        for (k, c) in r.char_indices() {
            let ok = c.is_alphanumeric()
                || c == '_'
                || (c == '-'
                    && k > 0
                    && r[k + 1..]
                        .chars()
                        .next()
                        .is_some_and(|n| n.is_alphanumeric() || n == '_'));
            if !ok {
                break;
            }
            end = k + c.len_utf8();
        }
        if end == 0 {
            return None;
        }
        self.i += end;
        Some(&r[..end])
    }

    /// A node's brackets and text, if it has them.
    fn shape(&mut self) -> Result<Option<(Shape, String)>, String> {
        let r = self.rest();
        if let Some(open) = OTHER_SHAPES.iter().find(|o| r.starts_with(**o)) {
            let what = match *open {
                "(((" => "double circles (`(((text)))`)",
                "@{" => "`@{ shape: ... }`",
                ">" => "asymmetric shapes (`>text]`)",
                _ => "parallelograms and trapezoids (`[/text/]`, `[\\text\\]`)",
            };
            return Err(format!(
                "{what} aren't supported: use [], (), ([]), [[]], [()], (()), {{}} or {{{{}}}}"
            ));
        }
        for &(open, close, shape) in SHAPES {
            if let Some(body) = r.strip_prefix(open) {
                let (label, used) = text_until(body, close)
                    .ok_or_else(|| format!("`{open}` isn't closed: expected `{close}`"))?;
                self.i += open.len() + used + close.len();
                return Ok(Some((shape, label)));
            }
        }
        Ok(None)
    }

    /// A link: `-->`, `---`, `-.->`, `-.-`, `==>`, `===`, `<-->` and longer
    /// forms, with text as `-->|text|` or `-- text -->`.
    fn link(&mut self) -> Result<Link, String> {
        let start = self.i;
        let arrow_start = self.eat("<");
        let r = self.rest();
        let (stroke, open_label) = if r.starts_with("==") {
            (Stroke::Thick, "==")
        } else if r.starts_with("-.") {
            (Stroke::Dotted, "-.")
        } else if r.starts_with("--") {
            (Stroke::Solid, "--")
        } else {
            self.i = start;
            if self.rest().starts_with("~~~") {
                return Err("invisible links (`~~~`) aren't supported".into());
            }
            return Err(self.expected("a link (`-->`, `---`, `-.->`, `==>`)"));
        };
        let mut label = None;
        let (arrow_end, min_len);
        // `-- text -->`: the opening on its own, then text, then a link.
        let labelled = match stroke {
            Stroke::Dotted => !r[2..].starts_with(['.', '-']),
            _ => {
                let ch = r.as_bytes()[0] as char;
                let run = r.find(|c| c != ch).unwrap_or(r.len());
                run == 2 && !r[2..].starts_with(['>', 'x', 'o'])
            }
        };
        if labelled {
            self.i += open_label.len();
            let close = match stroke {
                Stroke::Solid => "--",
                Stroke::Thick => "==",
                Stroke::Dotted => ".-",
            };
            let body = self.rest();
            let end = body.find(close).ok_or_else(|| {
                format!("the link's text isn't followed by its end, like `{close}>`")
            })?;
            label = Some(clean_label(body[..end].trim()));
            self.i += end;
            (arrow_end, min_len) = self.link_end(stroke)?;
        } else {
            if stroke == Stroke::Dotted {
                self.i += 1;
            }
            (arrow_end, min_len) = self.link_end(stroke)?;
        }
        if arrow_start && !arrow_end {
            return Err("a link that starts with `<` must end with `>`".into());
        }
        if label.is_none() {
            let save = self.i;
            self.skip_ws();
            if let Some(body) = self.rest().strip_prefix('|') {
                let (text, used) =
                    text_until(body, "|").ok_or("the link's `|text` isn't closed: expected `|`")?;
                label = Some(text);
                self.i += 1 + used + 1;
            } else {
                self.i = save;
            }
        }
        Ok(Link {
            label: label.filter(|l| !l.is_empty()),
            stroke,
            arrow_start,
            arrow_end,
            min_len,
        })
    }

    /// The end of a link, after any text: for solid and thick strokes a run
    /// of `-` or `=`, for dotted ones `.`s then `-`; then `>` or not.
    /// Returns whether there's an arrowhead and the link's length.
    fn link_end(&mut self, stroke: Stroke) -> Result<(bool, usize), String> {
        let start = self.i;
        let len = match stroke {
            Stroke::Dotted => {
                let dots = self.take_while(|c| c == '.').len();
                if dots == 0 || !self.eat("-") {
                    self.i = start;
                    return Err(self.expected("a dotted link's end (`.->` or `.-`)"));
                }
                dots
            }
            Stroke::Solid | Stroke::Thick => {
                let ch = if stroke == Stroke::Solid { '-' } else { '=' };
                self.take_while(|c| c == ch).len()
            }
        };
        let arrow = self.eat(">");
        let r = self.rest();
        if !arrow && r.starts_with(['x', 'o']) {
            return Err("cross and circle arrowheads (`--x`, `--o`) aren't supported".into());
        }
        let min_len = match stroke {
            Stroke::Dotted => len,
            // `-->` and `---` are the shortest: two dashes and a head, or three.
            _ if arrow && len >= 2 => len - 1,
            _ if len >= 3 => len - 2,
            _ => {
                self.i = start;
                return Err(self.expected("a link (`-->`, `---`, `-.->`, `==>`)"));
            }
        };
        Ok((arrow, min_len))
    }
}

/// The text before `close` and how many bytes it and the space before
/// `close` take, honouring `"quoted"` text that may hold brackets.
fn text_until(body: &str, close: &str) -> Option<(String, usize)> {
    let t = body.trim_start();
    if let Some(q) = t.strip_prefix('"') {
        let end = q.find('"')?;
        let used = body.len() - t.len() + 1 + end + 1;
        let rest = &body[used..];
        let pad = rest.len() - rest.trim_start().len();
        return rest[pad..]
            .starts_with(close)
            .then(|| (clean_label(&q[..end]), used + pad));
    }
    let end = body.find(close)?;
    Some((clean_label(&body[..end]), end))
}

/// `<br>` as a line break, and each line trimmed.
fn clean_label(s: &str) -> String {
    let mut out = s.to_string();
    for br in ["<br>", "<br/>", "<br />", "<BR>", "<BR/>"] {
        out = out.replace(br, "\n");
    }
    out.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(src: &str) -> Flowchart {
        match parse(src) {
            Ok(Diagram::Flowchart(f)) => f,
            other => panic!("{other:?}"),
        }
    }

    fn error(src: &str) -> (String, Option<usize>) {
        let e = parse(src).unwrap_err();
        (e.message, e.line)
    }

    fn edge(f: &Flowchart, i: usize) -> (&str, &str) {
        let e = &f.edges[i];
        (&f.nodes[e.from].id, &f.nodes[e.to].id)
    }

    #[test]
    fn headers_and_directions() {
        for (src, dir) in [
            ("graph", Direction::TopDown),
            ("graph TD", Direction::TopDown),
            ("flowchart TB", Direction::TopDown),
            ("flowchart BT", Direction::BottomUp),
            ("graph LR", Direction::LeftRight),
            ("graph RL", Direction::RightLeft),
            (
                "%% a comment\n%%{init: {}}%%\n\n  graph LR  ",
                Direction::LeftRight,
            ),
        ] {
            let f = fc(src);
            assert_eq!(f.direction, dir, "{src}");
            assert!(f.nodes.is_empty());
        }
    }

    #[test]
    fn every_shape() {
        let f = fc("graph TD\n\
             a\n b[rect]\n c(round)\n d([stadium])\n e[[sub]]\n f[(db)]\n g((circle))\n h{diamond}\n i{{hex}}");
        let shapes: Vec<_> = f
            .nodes
            .iter()
            .map(|n| (n.label.as_str(), n.shape))
            .collect();
        assert_eq!(
            shapes,
            [
                ("a", Shape::Rect),
                ("rect", Shape::Rect),
                ("round", Shape::Round),
                ("stadium", Shape::Stadium),
                ("sub", Shape::Subroutine),
                ("db", Shape::Cylinder),
                ("circle", Shape::Circle),
                ("diamond", Shape::Diamond),
                ("hex", Shape::Hexagon),
            ]
        );
    }

    #[test]
    fn every_link() {
        let f = fc("graph LR\n\
             a-->b\n a---b\n a-.->b\n a-.-b\n a==>b\n a===b\n a<-->b\n\
             a--->b\n a----b\n a-..->b\n a====>b\n a<==>b\n a<-.->b");
        let links: Vec<_> = f
            .edges
            .iter()
            .map(|e| (e.stroke, e.arrow_start, e.arrow_end, e.min_len))
            .collect();
        use Stroke::*;
        assert_eq!(
            links,
            [
                (Solid, false, true, 1),
                (Solid, false, false, 1),
                (Dotted, false, true, 1),
                (Dotted, false, false, 1),
                (Thick, false, true, 1),
                (Thick, false, false, 1),
                (Solid, true, true, 1),
                (Solid, false, true, 2),
                (Solid, false, false, 2),
                (Dotted, false, true, 2),
                (Thick, false, true, 3),
                (Thick, true, true, 1),
                (Dotted, true, true, 1),
            ]
        );
        assert_eq!(f.nodes.len(), 2);
    }

    #[test]
    fn link_labels() {
        let f = fc("graph TD\n\
             A -->|yes| B\n A -- no way --> C\n A -. maybe .-> D\n A == sure ==> E\n\
             A --->|\"quoted | text\"| F\n A -- open --- G\n A -->|| H\n A-- non-blocking -->I");
        let labels: Vec<_> = f.edges.iter().map(|e| e.label.as_deref()).collect();
        assert_eq!(
            labels,
            [
                Some("yes"),
                Some("no way"),
                Some("maybe"),
                Some("sure"),
                Some("quoted | text"),
                Some("open"),
                None,
                Some("non-blocking"),
            ]
        );
        assert_eq!(f.edges[1].stroke, Stroke::Solid);
        assert_eq!(f.edges[2].stroke, Stroke::Dotted);
        assert!(f.edges[2].arrow_end);
        assert_eq!(f.edges[3].stroke, Stroke::Thick);
        assert_eq!(f.edges[4].min_len, 2);
        assert!(!f.edges[5].arrow_end);
        assert_eq!(edge(&f, 7), ("A", "I"));
    }

    #[test]
    fn chains_groups_and_semicolons() {
        let f = fc("flowchart TD; a & b --> c --- d; e");
        assert_eq!(f.nodes.len(), 5);
        let edges: Vec<_> = (0..f.edges.len()).map(|i| edge(&f, i)).collect();
        assert_eq!(edges, [("a", "c"), ("b", "c"), ("c", "d")]);
        assert!(!f.edges[2].arrow_end);

        let f = fc("graph LR\na & b --> c & d");
        assert_eq!(f.edges.len(), 4);
    }

    #[test]
    fn labels_quotes_and_breaks() {
        let f = fc("graph TD\nn-1[\"a [b]; c\"] --> n-2[one<br>two <br/> three]\nx(\"(paren)\")");
        assert_eq!(f.nodes[0].id, "n-1");
        assert_eq!(f.nodes[0].label, "a [b]; c");
        assert_eq!(f.nodes[1].id, "n-2");
        assert_eq!(f.nodes[1].label, "one\ntwo\nthree");
        assert_eq!(f.nodes[2].label, "(paren)");
        assert_eq!(f.edges.len(), 1);
    }

    #[test]
    fn the_last_definition_wins() {
        let f = fc("graph TD\nA --> B\nA{Choice}\nB --> A[Box]");
        assert_eq!(f.nodes.len(), 2);
        assert_eq!(f.nodes[0].label, "Box");
        assert_eq!(f.nodes[0].shape, Shape::Rect);
        assert_eq!(f.nodes[1].label, "B");
    }

    #[test]
    fn styling_is_ignored() {
        let f = fc("graph TD\n\
             classDef red fill:#f00;\n class A red\n style A fill:#f9f,stroke:#333\n\
             linkStyle 0 stroke:#ff3\n click A \"https://example.com\"\n\
             accTitle: The title\n accDescr: one line\n accDescr {\n many\n lines\n }\n\
             A --> B");
        assert_eq!(f.nodes.len(), 2);
        assert_eq!(f.edges.len(), 1);
    }

    #[test]
    fn other_diagrams_are_unsupported() {
        for (src, kind) in [
            ("sequenceDiagram\nA->>B: hi", "sequenceDiagram"),
            ("%% c\nclassDiagram", "classDiagram"),
            ("pie title Pets", "pie"),
            ("stateDiagram-v2", "stateDiagram-v2"),
        ] {
            assert_eq!(parse(src), Ok(Diagram::Unsupported(kind.into())));
        }
    }

    #[test]
    fn errors_name_their_line() {
        let cases: &[(&str, &str, Option<usize>)] = &[
            ("", "empty", None),
            ("%% only a comment", "empty", None),
            ("graph XX", "unknown direction `XX`", Some(1)),
            ("graph TD extra", "unexpected `extra`", Some(1)),
            (
                "graph TD\nA-->B\nsubgraph one",
                "`subgraph` isn't supported",
                Some(3),
            ),
            ("graph TD\nend", "`end` isn't supported", Some(2)),
            (
                "graph TD\n  direction LR",
                "`direction` isn't supported",
                Some(2),
            ),
            ("graph TD\nA:::hot --> B", "`:::class`", Some(2)),
            ("graph TD\nA --x B", "cross and circle", Some(2)),
            ("graph TD\nA --o B", "cross and circle", Some(2)),
            ("graph TD\nA>flag]", "asymmetric", Some(2)),
            ("graph TD\nA[/para/]", "parallelograms", Some(2)),
            ("graph TD\nA[\\alt\\]", "parallelograms", Some(2)),
            ("graph TD\nA[/trap\\]", "parallelograms", Some(2)),
            ("graph TD\nA(((double)))", "double circles", Some(2)),
            ("graph TD\nA@{ shape: rect }", "`@{ shape", Some(2)),
            ("graph TD\n\nA[oops --> B", "`[` isn't closed", Some(3)),
            ("graph TD\nA(\"quote) --> B", "`(` isn't closed", Some(2)),
            (
                "graph TD\nA -->|text B",
                "isn't closed: expected `|`",
                Some(2),
            ),
            ("graph TD\nA -->", "doesn't lead to a node", Some(2)),
            ("graph TD\nA --> ;", "doesn't lead to a node", Some(2)),
            ("graph TD\nA -- text B", "text isn't followed", Some(2)),
            (
                "graph TD\nA B",
                "expected a link (`-->`, `---`, `-.->`, `==>`) at `B`",
                Some(2),
            ),
            ("graph TD\nA - B", "expected a link", Some(2)),
            ("graph TD\nA <--- B", "must end with `>`", Some(2)),
            ("graph TD\nA ~~~ B", "invisible", Some(2)),
            ("graph TD\n--> B", "expected a node id", Some(2)),
            ("graph TD; A-->B; A-->", "doesn't lead", Some(1)),
        ];
        for &(src, message, line) in cases {
            let (m, l) = error(src);
            assert!(m.contains(message), "{src:?}: {m}");
            assert_eq!(l, line, "{src:?}: {m}");
        }
    }

    #[test]
    fn size_is_capped() {
        let nodes: String = (0..=MAX_NODES).map(|i| format!("n{i}\n")).collect();
        let (m, l) = error(&format!("graph TD\n{nodes}"));
        assert!(m.contains("too many nodes"), "{m}");
        assert_eq!(l, Some(MAX_NODES + 2));
        let ok: String = (0..MAX_NODES).map(|i| format!("n{i}\n")).collect();
        assert_eq!(fc(&format!("graph TD\n{ok}")).nodes.len(), MAX_NODES);

        // 20 × 16 = 320 edges from one statement.
        let left = (0..20)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join(" & ");
        let right = (0..16)
            .map(|i| format!("b{i}"))
            .collect::<Vec<_>>()
            .join(" & ");
        let (m, l) = error(&format!("graph TD\n{left} --> {right}"));
        assert!(m.contains("too many links"), "{m}");
        assert_eq!(l, Some(2));
    }

    #[test]
    fn ids_with_hyphens_and_digits() {
        let f = fc("graph LR\nstep-1-->step-2\n2nd --- x_y");
        let ids: Vec<_> = f.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["step-1", "step-2", "2nd", "x_y"]);
    }

    #[test]
    fn doc_examples_parse() {
        let doc = include_str!("../../../../docs/mermaid.md");
        let mut blocks = 0;
        let mut lines = doc.lines();
        while let Some(line) = lines.next() {
            if line == "```mermaid" {
                let src: Vec<&str> = lines.by_ref().take_while(|l| *l != "```").collect();
                let f = fc(&src.join("\n"));
                assert!(!f.nodes.is_empty(), "example {}", blocks + 1);
                blocks += 1;
            }
        }
        assert_eq!(blocks, 3);
    }
}
