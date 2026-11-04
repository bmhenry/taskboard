//! Parses Markdown into a tree of simple blocks the UI can draw with plain
//! text elements. Inline formatting is reduced to its text.
//!
//! A block's `children` are the blocks nested inside it: for a list item,
//! its sub-lists and any paragraphs or code after its first line; for a
//! heading, its section: everything up to the next heading of the same or a
//! higher level, or the end of the list item or quote the heading is in.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

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
    Quote,
    Rule,
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
            if !b.text.is_empty() {
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

    /// Ends the open sections of `level` or deeper, back to the innermost
    /// list item or quote.
    fn close_sections(&mut self, level: u8) {
        self.flush();
        let floor = self.quotes.last().copied().unwrap_or(0);
        while self.open.len() > floor
            && matches!(self.open.last(), Some(Block { kind: BlockKind::Heading(l), .. }) if *l >= level)
        {
            self.close();
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

    /// Where inline text goes: the current leaf, else the open item's
    /// first line, else a new paragraph.
    fn text(&mut self) -> &mut String {
        if self.current.is_none() && self.item_line().is_some() {
            return &mut self.item_line().unwrap().text;
        }
        &mut self
            .current
            .get_or_insert_with(|| Block::new(BlockKind::Paragraph))
            .text
    }

    fn finish(mut self) -> Vec<Block> {
        while !self.open.is_empty() {
            self.close();
        }
        self.flush();
        number(&mut self.roots, &[]);
        self.roots
    }
}

fn number(blocks: &mut [Block], parent: &[usize]) {
    for (i, b) in blocks.iter_mut().enumerate() {
        b.id = [parent, &[i]].concat();
        number(&mut b.children, &b.id.clone());
    }
}

pub fn parse(src: &str) -> Vec<Block> {
    let mut tree = Builder::default();
    // Each open list's next number, or None for bullet lists.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0;

    let options = Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
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
            Event::Start(Tag::CodeBlock(_)) => tree.start(BlockKind::Code),
            Event::Start(Tag::BlockQuote(_)) => {
                tree.flush();
                tree.quotes.push(tree.open.len());
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                tree.close_sections(1);
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
                tree.close_sections(1);
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
}
