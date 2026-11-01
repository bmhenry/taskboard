//! Flattens Markdown into a list of simple blocks the UI can draw with plain
//! text elements. Inline formatting is reduced to its text.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Heading(u8),
    Paragraph,
    /// A list item at `depth` (0 = top level) with its bullet or number.
    ListItem {
        depth: usize,
        marker: String,
    },
    Code,
    Quote,
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

pub fn parse(src: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Each open list's next number, or None for bullet lists.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0;
    let mut current: Option<Block> = None;

    let flush = |current: &mut Option<Block>, blocks: &mut Vec<Block>| {
        if let Some(mut b) = current.take() {
            let trimmed = b.text.trim_end().to_string();
            if !trimmed.is_empty() {
                b.text = trimmed;
                blocks.push(b);
            }
        }
    };
    let start = |kind: BlockKind, current: &mut Option<Block>, blocks: &mut Vec<Block>| {
        flush(current, blocks);
        *current = Some(Block {
            kind,
            text: String::new(),
        });
    };

    let options = Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(src, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                start(BlockKind::Heading(level as u8), &mut current, &mut blocks)
            }
            Event::Start(Tag::Paragraph) => {
                // Paragraphs inside a list item continue that item.
                if !matches!(
                    current,
                    Some(Block {
                        kind: BlockKind::ListItem { .. },
                        ..
                    })
                ) {
                    let kind = if quote_depth > 0 {
                        BlockKind::Quote
                    } else {
                        BlockKind::Paragraph
                    };
                    start(kind, &mut current, &mut blocks);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => start(BlockKind::Code, &mut current, &mut blocks),
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut current, &mut blocks);
                quote_depth -= 1;
            }
            Event::Start(Tag::List(first)) => {
                flush(&mut current, &mut blocks);
                lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut current, &mut blocks);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let depth = lists.len().saturating_sub(1);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => "•".to_string(),
                };
                start(
                    BlockKind::ListItem { depth, marker },
                    &mut current,
                    &mut blocks,
                );
            }
            Event::End(TagEnd::Paragraph) => {
                // A loose list item's paragraph ends before the item does.
                if !matches!(
                    current,
                    Some(Block {
                        kind: BlockKind::ListItem { .. },
                        ..
                    })
                ) {
                    flush(&mut current, &mut blocks);
                }
            }
            Event::End(TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item) => {
                flush(&mut current, &mut blocks)
            }
            Event::Rule => {
                flush(&mut current, &mut blocks);
                blocks.push(Block {
                    kind: BlockKind::Rule,
                    text: String::new(),
                });
            }
            Event::TaskListMarker(done) => {
                // The checkbox replaces the item's bullet.
                if let Some(Block {
                    kind: BlockKind::ListItem { marker, .. },
                    ..
                }) = current.as_mut()
                {
                    *marker = if done { "☑" } else { "☐" }.to_string();
                }
            }
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) | Event::Html(t) => {
                let b = current.get_or_insert_with(|| Block {
                    kind: BlockKind::Paragraph,
                    text: String::new(),
                });
                b.text.push_str(&t);
            }
            Event::SoftBreak => {
                if let Some(b) = current.as_mut() {
                    b.text.push(' ');
                }
            }
            Event::HardBreak => {
                if let Some(b) = current.as_mut() {
                    b.text.push('\n');
                }
            }
            _ => {}
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_blocks() {
        let blocks = parse(
            "# Plan\n\nDo **this** first.\n\n1. one\n2. two\n   - nested\n\n```\nlet x = 1;\n```\n",
        );
        let kinds: Vec<_> = blocks.iter().map(|b| (&b.kind, b.text.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (&BlockKind::Heading(1), "Plan"),
                (&BlockKind::Paragraph, "Do this first."),
                (
                    &BlockKind::ListItem {
                        depth: 0,
                        marker: "1.".into()
                    },
                    "one"
                ),
                (
                    &BlockKind::ListItem {
                        depth: 0,
                        marker: "2.".into()
                    },
                    "two"
                ),
                (
                    &BlockKind::ListItem {
                        depth: 1,
                        marker: "•".into()
                    },
                    "nested"
                ),
                (&BlockKind::Code, "let x = 1;"),
            ]
        );
    }
}
