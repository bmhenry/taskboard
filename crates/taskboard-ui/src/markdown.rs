//! Parses Markdown into a tree of simple blocks the UI can draw with plain
//! text elements. Inline formatting is reduced to its text.
//!
//! A block's `children` are the blocks nested inside it: for a list item,
//! its sub-lists and any paragraphs or code after its first line; for a
//! heading, its section: everything up to the next heading of the same or a
//! higher level, or the end of the list item, quote or details the heading
//! is in; for a details block, what's between its summary and `</details>`.
//!
//! `<details>`, `<summary>…</summary>` and `</details>` are understood when
//! they're HTML blocks of their own (as GitHub writes them); any other HTML,
//! and these tags when they don't pair up, is shown as text.
//!
//! GitHub's tables become a [`Table`] block, whose cells' inline formatting
//! is reduced to its text like a paragraph's.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    /// A heading of level 1–6, whose children are its section.
    Heading(u8),
    Paragraph,
    /// A list item with its bullet, number or checkbox.
    ListItem {
        marker: String,
    },
    Code,
    /// A fenced code block whose language is `chart`: its text is the
    /// chart's spec, drawn by `ui::chart`.
    Chart,
    Quote,
    Rule,
    /// A `<details>` block, whose text is its summary. `open` is whether it
    /// starts expanded.
    Details {
        open: bool,
    },
    Table(Table),
}

/// How a table column's cells line up, from the `:` in its delimiter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A table's cells, each row as many as there are columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub align: Vec<Align>,
    pub head: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    fn new(align: &[Alignment]) -> Self {
        let align = align
            .iter()
            .map(|a| match a {
                Alignment::Center => Align::Center,
                Alignment::Right => Align::Right,
                Alignment::None | Alignment::Left => Align::Left,
            })
            .collect();
        Table {
            align,
            ..Table::default()
        }
    }

    /// Trims each cell and pads or cuts each row to the column count.
    fn tidy(&mut self) {
        let columns = self.align.len();
        for row in std::iter::once(&mut self.head).chain(&mut self.rows) {
            row.resize(columns, String::new());
            for cell in row {
                cell.truncate(cell.trim_end().len());
                cell.drain(..cell.len() - cell.trim_start().len());
            }
        }
    }
}

/// Where a block is in the tree: its index among its siblings, after those
/// of its ancestors. Stays the same while the source does, so UI state such
/// as which sections are collapsed can be keyed by it.
pub type BlockId = Vec<usize>;

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    pub text: String,
    pub children: Vec<Block>,
}

impl Block {
    fn new(kind: BlockKind) -> Self {
        Block {
            id: BlockId::new(),
            kind,
            text: String::new(),
            children: Vec::new(),
        }
    }
}

/// The tree under construction.
#[derive(Default)]
struct Builder {
    roots: Vec<Block>,
    /// Open containers, outermost first; finished blocks go into the last.
    open: Vec<Block>,
    /// The leaf block whose text is being collected.
    current: Option<Block>,
    /// For each open quote, how many containers were open when it started:
    /// sections started inside it end with it.
    quotes: Vec<usize>,
    /// For each open details block, the HTML that opened it, shown instead
    /// if it's never closed.
    details: Vec<String>,
    /// The table being collected, with the row it's on.
    table: Option<(Table, Vec<String>)>,
}

impl Builder {
    fn push(&mut self, block: Block) {
        match self.open.last_mut() {
            Some(parent) => parent.children.push(block),
            None => self.roots.push(block),
        }
    }

    /// Finishes the current leaf, dropping it if it has no text.
    fn flush(&mut self) {
        if let Some(mut b) = self.current.take() {
            b.text.truncate(b.text.trim_end().len());
            // An empty chart is still shown, as an error.
            if !b.text.is_empty() || b.kind == BlockKind::Chart {
                self.push(b);
            }
        }
    }

    fn start(&mut self, kind: BlockKind) {
        self.flush();
        self.current = Some(Block::new(kind));
    }

    fn open(&mut self, kind: BlockKind) {
        self.flush();
        self.open.push(Block::new(kind));
    }

    /// Finishes the innermost container, dropping it if it's empty.
    fn close(&mut self) {
        self.flush();
        if let Some(mut b) = self.open.pop() {
            b.text.truncate(b.text.trim_end().len());
            if !b.text.is_empty() || !b.children.is_empty() {
                self.push(b);
            }
        }
    }

    /// Makes the heading just collected the open section, unless it's empty.
    fn open_section(&mut self) {
        if let Some(mut b) = self.current.take() {
            b.text.truncate(b.text.trim_end().len());
            if !b.text.is_empty() {
                self.open.push(b);
            }
        }
    }

    /// How many open containers are outside the innermost quote.
    fn floor(&self) -> usize {
        self.quotes.last().copied().unwrap_or(0)
    }

    /// Ends the open sections of `level` or deeper, back to the innermost
    /// list item, quote or details.
    fn close_sections(&mut self, level: u8) {
        self.flush();
        while self.open.len() > self.floor()
            && matches!(self.open.last(), Some(Block { kind: BlockKind::Heading(l), .. }) if *l >= level)
        {
            self.close();
        }
    }

    /// Ends what was opened inside the innermost list item or quote, which
    /// is ending: its sections, and any details never closed.
    fn close_inner(&mut self) {
        self.flush();
        while self.open.len() > self.floor() {
            match self.open.last().map(|b| &b.kind) {
                Some(BlockKind::Heading(_)) => self.close(),
                Some(BlockKind::Details { .. }) => self.unwrap_details(),
                _ => break,
            }
        }
    }

    /// Adds `raw` as a paragraph of its own.
    fn literal(&mut self, raw: &str) {
        self.text().push_str(raw.trim());
        self.flush();
    }

    /// Handles an HTML block: details tags become blocks, anything else is
    /// shown as text.
    fn html_block(&mut self, html: &str) {
        let Some(tokens) = details_tokens(html) else {
            return self.literal(html);
        };
        for token in tokens {
            match token {
                Html::Open { open, raw } => {
                    self.open(BlockKind::Details { open });
                    self.details.push(raw.to_string());
                }
                Html::Summary { text, raw } => match self.open.last_mut() {
                    // Only a details' first block can be its summary.
                    Some(b)
                        if matches!(b.kind, BlockKind::Details { .. })
                            && b.text.is_empty()
                            && b.children.is_empty()
                            && self.current.is_none() =>
                    {
                        b.text = text;
                        if let Some(opening) = self.details.last_mut() {
                            opening.push('\n');
                            opening.push_str(raw);
                        }
                    }
                    _ => self.literal(raw),
                },
                Html::Close { raw } => self.close_details(raw),
                Html::Text(t) => self.literal(t),
            }
        }
    }

    /// Ends the innermost details, with its sections, if it was opened
    /// inside the innermost list item or quote; otherwise `</details>`
    /// (`raw`) doesn't pair up, and is shown as text.
    fn close_details(&mut self, raw: &str) {
        self.flush();
        let inner = self
            .open
            .iter()
            .rposition(|b| !matches!(b.kind, BlockKind::Heading(_)))
            .filter(|&i| i >= self.floor());
        if inner.is_some_and(|i| matches!(self.open[i].kind, BlockKind::Details { .. })) {
            self.close_sections(1);
            self.close();
            self.details.pop();
        } else {
            self.literal(raw);
        }
    }

    /// Replaces the innermost container, a details that was never closed,
    /// by the HTML that opened it, followed by what it holds.
    fn unwrap_details(&mut self) {
        self.flush();
        let Some(b) = self.open.pop() else { return };
        let opening = self.details.pop().unwrap_or_default();
        self.literal(&opening);
        for child in b.children {
            self.push(child);
        }
    }

    /// The open list item still collecting its own first line, if there is
    /// one: text goes there until the item gets children.
    fn item_line(&mut self) -> Option<&mut Block> {
        match self.open.last_mut() {
            Some(
                b @ Block {
                    kind: BlockKind::ListItem { .. },
                    ..
                },
            ) if b.children.is_empty() => Some(b),
            _ => None,
        }
    }

    /// The table cell being collected; only called while there's a table.
    fn cell(&mut self) -> &mut String {
        let (_, row) = self.table.get_or_insert_default();
        if row.is_empty() {
            row.push(String::new());
        }
        let last = row.len() - 1;
        &mut row[last]
    }

    /// Where inline text goes: the table cell being collected, else the
    /// current leaf, else the open item's first line, else a new paragraph.
    fn text(&mut self) -> &mut String {
        // Checked before borrowing: returning a borrow from inside an
        // `if let` would keep `self` borrowed for the rest of the function.
        if self.table.is_some() {
            return self.cell();
        }
        if self.current.is_none() && self.item_line().is_some() {
            return &mut self.item_line().unwrap().text;
        }
        &mut self
            .current
            .get_or_insert_with(|| Block::new(BlockKind::Paragraph))
            .text
    }

    fn finish(mut self) -> Vec<Block> {
        while let Some(b) = self.open.last() {
            if matches!(b.kind, BlockKind::Details { .. }) {
                self.unwrap_details();
            } else {
                self.close();
            }
        }
        self.flush();
        number(&mut self.roots, &[]);
        self.roots
    }
}

/// A piece of an HTML block made only of details tags and text.
#[derive(Debug, PartialEq)]
enum Html<'a> {
    /// `<details>`, or `<details open>`.
    Open {
        open: bool,
        raw: &'a str,
    },
    /// `<summary>…</summary>`, with its text.
    Summary {
        text: String,
        raw: &'a str,
    },
    Close {
        raw: &'a str,
    },
    Text(&'a str),
}

/// Splits `html` into details tags and the text between them, or None if it
/// holds any other tag.
fn details_tokens(html: &str) -> Option<Vec<Html<'_>>> {
    let mut out = Vec::new();
    let mut rest = html.trim();
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push(Html::Text(rest[..end].trim()));
            rest = rest[end..].trim_start();
            continue;
        }
        let end = rest.find('>')? + 1;
        let tag = &rest[1..end - 1];
        let (name, attrs) = tag
            .split_once(|c: char| c.is_ascii_whitespace())
            .unwrap_or((tag, ""));
        let raw;
        match name.to_ascii_lowercase().as_str() {
            "details" => {
                raw = &rest[..end];
                let open = attrs.split_ascii_whitespace().any(|a| {
                    a.split('=')
                        .next()
                        .is_some_and(|n| n.eq_ignore_ascii_case("open"))
                });
                out.push(Html::Open { open, raw });
            }
            "/details" if attrs.trim().is_empty() => {
                raw = &rest[..end];
                out.push(Html::Close { raw });
            }
            "summary" => {
                let close = rest.to_ascii_lowercase().find("</summary>")?;
                raw = &rest[..close + "</summary>".len()];
                let text = strip_tags(&rest[end..close]);
                out.push(Html::Summary { text, raw });
            }
            _ => return None,
        }
        rest = rest[raw.len()..].trim_start();
    }
    Some(out)
}

/// `html`'s text without its tags, with its whitespace collapsed.
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn number(blocks: &mut [Block], parent: &[usize]) {
    for (i, b) in blocks.iter_mut().enumerate() {
        b.id = [parent, &[i]].concat();
        number(&mut b.children, &b.id.clone());
    }
}

/// A fenced block whose info string's first word is `chart` is a chart;
/// any other code block is code.
fn code_kind(kind: &CodeBlockKind) -> BlockKind {
    match kind {
        CodeBlockKind::Fenced(info) if info.split_whitespace().next() == Some("chart") => {
            BlockKind::Chart
        }
        _ => BlockKind::Code,
    }
}

pub fn parse(src: &str) -> Vec<Block> {
    let mut tree = Builder::default();
    // Each open list's next number, or None for bullet lists.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0;
    // The HTML block being collected.
    let mut html: Option<String> = None;

    let options =
        Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for event in Parser::new_ext(src, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                tree.close_sections(level as u8);
                tree.start(BlockKind::Heading(level as u8));
            }
            Event::End(TagEnd::Heading(_)) => tree.open_section(),
            Event::Start(Tag::Paragraph) => {
                // A list item's first paragraph is its own text.
                let first_line = tree.item_line().is_some_and(|b| b.text.is_empty());
                if !first_line {
                    tree.start(if quote_depth > 0 {
                        BlockKind::Quote
                    } else {
                        BlockKind::Paragraph
                    });
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => tree.start(code_kind(&kind)),
            Event::Start(Tag::BlockQuote(_)) => {
                tree.flush();
                tree.quotes.push(tree.open.len());
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                tree.close_inner();
                tree.quotes.pop();
                quote_depth -= 1;
            }
            Event::Start(Tag::List(first)) => {
                tree.flush();
                lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                tree.flush();
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => "•".to_string(),
                };
                tree.open(BlockKind::ListItem { marker });
            }
            Event::End(TagEnd::Item) => {
                tree.close_inner();
                tree.close();
            }
            Event::End(TagEnd::Paragraph | TagEnd::CodeBlock) => tree.flush(),
            Event::Rule => {
                tree.flush();
                tree.push(Block::new(BlockKind::Rule));
            }
            Event::TaskListMarker(done) => {
                // The checkbox replaces the item's bullet.
                if let Some(Block {
                    kind: BlockKind::ListItem { marker },
                    ..
                }) = tree.item_line()
                {
                    *marker = if done { "☑" } else { "☐" }.to_string();
                }
            }
            Event::Start(Tag::Table(align)) => {
                tree.flush();
                tree.table = Some((Table::new(&align), Vec::new()));
            }
            Event::Start(Tag::TableCell) => {
                if let Some((_, row)) = &mut tree.table {
                    row.push(String::new());
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some((t, row)) = &mut tree.table {
                    t.head = std::mem::take(row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some((t, row)) = &mut tree.table {
                    t.rows.push(std::mem::take(row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((mut t, _)) = tree.table.take() {
                    t.tidy();
                    tree.push(Block::new(BlockKind::Table(t)));
                }
            }
            Event::Start(Tag::HtmlBlock) => html = Some(String::new()),
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(h) = html.take() {
                    tree.html_block(&h);
                }
            }
            Event::Html(t) if html.is_some() => html.as_mut().unwrap().push_str(&t),
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) | Event::Html(t) => {
                tree.text().push_str(&t)
            }
            Event::SoftBreak => tree.text().push(' '),
            Event::HardBreak => tree.text().push('\n'),
            _ => {}
        }
    }
    tree.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each block as (id, kind, text), depth first.
    fn flatten(blocks: &[Block]) -> Vec<(BlockId, BlockKind, String)> {
        let mut out = Vec::new();
        for b in blocks {
            out.push((b.id.clone(), b.kind.clone(), b.text.clone()));
            out.extend(flatten(&b.children));
        }
        out
    }

    fn item(marker: &str) -> BlockKind {
        BlockKind::ListItem {
            marker: marker.into(),
        }
    }

    #[test]
    fn parses_common_blocks() {
        let blocks = parse(
            "# Plan\n\nDo **this** first.\n\n1. one\n2. two\n   - nested\n\n```\nlet x = 1;\n```\n",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Heading(1), "Plan".into()),
                (vec![0, 0], BlockKind::Paragraph, "Do this first.".into()),
                (vec![0, 1], item("1."), "one".into()),
                (vec![0, 2], item("2."), "two".into()),
                (vec![0, 2, 0], item("•"), "nested".into()),
                (vec![0, 3], BlockKind::Code, "let x = 1;".into()),
            ]
        );
    }

    #[test]
    fn chart_blocks() {
        let blocks = parse("```chart\nmark: bar\n```\n\n```chart\n```\n\n```rust\nchart\n```\n");
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Chart, "mark: bar".into()),
                // Empty, but kept so it shows its error.
                (vec![1], BlockKind::Chart, String::new()),
                (vec![2], BlockKind::Code, "chart".into()),
            ]
        );
    }

    #[test]
    fn nests_list_item_contents() {
        let blocks = parse(
            "- [x] done\n\n  more about it\n\n  ```\n  code\n  ```\n- [ ] todo\n  - a\n    - b\n\n> quoted\n",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], item("☑"), "done".into()),
                (vec![0, 0], BlockKind::Paragraph, "more about it".into()),
                (vec![0, 1], BlockKind::Code, "code".into()),
                (vec![1], item("☐"), "todo".into()),
                (vec![1, 0], item("•"), "a".into()),
                (vec![1, 0, 0], item("•"), "b".into()),
                (vec![2], BlockKind::Quote, "quoted".into()),
            ]
        );
    }

    #[test]
    fn breaks_and_inline_text() {
        let blocks = parse("one\ntwo  \nthree `code` <b>x</b>\n\n- item\n  wraps\n");
        assert_eq!(
            flatten(&blocks),
            [
                (
                    vec![0],
                    BlockKind::Paragraph,
                    "one two\nthree code <b>x</b>".into()
                ),
                (vec![1], item("•"), "item wraps".into()),
            ]
        );
    }

    #[test]
    fn headings_own_their_sections() {
        let blocks = parse("intro\n\n# A\n\na\n\n### A.1\n\ndeep\n\n## A.2\n\n---\n\n# B\n\nb\n");
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Paragraph, "intro".into()),
                (vec![1], BlockKind::Heading(1), "A".into()),
                (vec![1, 0], BlockKind::Paragraph, "a".into()),
                // A skipped level still nests, and the next deeper-than-A
                // heading closes it.
                (vec![1, 1], BlockKind::Heading(3), "A.1".into()),
                (vec![1, 1, 0], BlockKind::Paragraph, "deep".into()),
                (vec![1, 2], BlockKind::Heading(2), "A.2".into()),
                (vec![1, 2, 0], BlockKind::Rule, String::new()),
                (vec![2], BlockKind::Heading(1), "B".into()),
                (vec![2, 0], BlockKind::Paragraph, "b".into()),
            ]
        );
    }

    #[test]
    fn sections_end_with_their_container() {
        let blocks = parse(
            "- item

  ## In item

  under it

after list

> ## Quoted
>
> q

after quote

#

under empty
",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], item("•"), "item".into()),
                (vec![0, 0], BlockKind::Heading(2), "In item".into()),
                (vec![0, 0, 0], BlockKind::Paragraph, "under it".into()),
                (vec![1], BlockKind::Paragraph, "after list".into()),
                (vec![2], BlockKind::Heading(2), "Quoted".into()),
                (vec![2, 0], BlockKind::Quote, "q".into()),
                (vec![3], BlockKind::Paragraph, "after quote".into()),
                // An empty heading is dropped and starts no section.
                (vec![4], BlockKind::Paragraph, "under empty".into()),
            ]
        );
    }

    fn details(open: bool) -> BlockKind {
        BlockKind::Details { open }
    }

    #[test]
    fn details_hold_their_contents() {
        let blocks = parse(
            "<details>
<summary>More <b>info</b></summary>

hidden

## Inside

under it

</details>

<details open><summary>Shown</summary>

- a

</details>
after
",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], details(false), "More info".into()),
                (vec![0, 0], BlockKind::Paragraph, "hidden".into()),
                (vec![0, 1], BlockKind::Heading(2), "Inside".into()),
                (vec![0, 1, 0], BlockKind::Paragraph, "under it".into()),
                (vec![1], details(true), "Shown".into()),
                (vec![1, 0], item("•"), "a".into()),
                (vec![2], BlockKind::Paragraph, "after".into()),
            ]
        );
    }

    #[test]
    fn details_nest_in_sections_and_each_other() {
        let blocks = parse(
            "# A

<DETAILS OPEN=\"\">

<summary>outer</summary>

<details>
<summary></summary>

inner

</details>

</details>

b
",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Heading(1), "A".into()),
                (vec![0, 0], details(true), "outer".into()),
                (vec![0, 0, 0], details(false), String::new()),
                (vec![0, 0, 0, 0], BlockKind::Paragraph, "inner".into()),
                (vec![0, 1], BlockKind::Paragraph, "b".into()),
            ]
        );
    }

    #[test]
    fn unpaired_details_tags_are_text() {
        let blocks = parse(
            "</details>

- <details>
  <summary>never closed</summary>

  in item

after

<details><summary>s</summary>with text</details>

<div>other</div>

<details>
",
        );
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Paragraph, "</details>".into()),
                // Shown as the item's own line, the HTML it began with.
                (
                    vec![1],
                    item("•"),
                    "<details>\n<summary>never closed</summary>".into()
                ),
                (vec![1, 0], BlockKind::Paragraph, "in item".into()),
                (vec![2], BlockKind::Paragraph, "after".into()),
                (vec![3], details(false), "s".into()),
                (vec![3, 0], BlockKind::Paragraph, "with text".into()),
                (vec![4], BlockKind::Paragraph, "<div>other</div>".into()),
                (vec![5], BlockKind::Paragraph, "<details>".into()),
            ]
        );
    }

    #[test]
    fn parses_tables() {
        let blocks = parse(
            r"| Name | Qty | Note |
|:-----|:---:|-----:|
| **bold** `code` | 2 | [link](https://x.y) \| pipe |
| short |
| a | b | c | extra |

after
",
        );
        let BlockKind::Table(t) = &blocks[0].kind else {
            panic!("not a table: {:?}", blocks[0]);
        };
        assert_eq!(t.align, [Align::Left, Align::Center, Align::Right]);
        assert_eq!(t.head, ["Name", "Qty", "Note"]);
        assert_eq!(
            t.rows,
            [
                // Inline formatting is reduced to its text.
                vec!["bold code", "2", "link | pipe"],
                // Short rows are padded and long ones cut to the header's
                // width.
                vec!["short", "", ""],
                vec!["a", "b", "c"],
            ]
        );
        assert_eq!(
            flatten(&blocks[1..]),
            [(vec![1], BlockKind::Paragraph, "after".into())]
        );
    }

    #[test]
    fn tables_nest_in_sections_and_items() {
        let blocks = parse("# A\n\n| x |\n|---|\n\n- item\n\n  | y |\n  |---|\n  | 1 |\n");
        let table = |head: &str, rows: &[&str]| {
            BlockKind::Table(Table {
                align: vec![Align::Left],
                head: vec![head.into()],
                rows: rows.iter().map(|r| vec![r.to_string()]).collect(),
            })
        };
        assert_eq!(
            flatten(&blocks),
            [
                (vec![0], BlockKind::Heading(1), "A".into()),
                (vec![0, 0], table("x", &[]), String::new()),
                (vec![0, 1], item("•"), "item".into()),
                (vec![0, 1, 0], table("y", &["1"]), String::new()),
            ]
        );
    }
}
