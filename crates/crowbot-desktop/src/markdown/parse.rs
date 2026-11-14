use std::ops::Range;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: InlineStyle,
    pub link: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<Span>,
    pub links: Vec<String>,
}

impl Inline {
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            spans: Vec::new(),
            links: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn link_at(&self, offset: usize) -> Option<&str> {
        let span = self
            .spans
            .iter()
            .find(|span| span.range.start <= offset && offset < span.range.end)?;
        self.links.get(span.link?).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Inline),
    Heading {
        level: u8,
        text: Inline,
    },
    Code {
        language: String,
        text: String,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Rule,
}

#[derive(Default)]
struct InlineBuilder {
    text: String,
    segments: Vec<Span>,
    links: Vec<String>,
    bold: usize,
    italic: usize,
    strike: usize,
    open_links: Vec<usize>,
}

impl InlineBuilder {
    fn style(&self, code: bool) -> InlineStyle {
        InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code,
        }
    }

    fn push(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let style = self.style(code);
        let link = self.open_links.last().copied();
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        match self.segments.last_mut() {
            Some(last) if last.style == style && last.link == link && last.range.end == start => {
                last.range.end = end;
            }
            _ => self.segments.push(Span {
                range: start..end,
                style,
                link,
            }),
        }
    }

    fn open_link(&mut self, target: &str) {
        self.links.push(target.to_owned());
        self.open_links.push(self.links.len() - 1);
    }

    fn finish(mut self) -> Inline {
        let trimmed = self.text.trim_end_matches('\n').len();
        self.text.truncate(trimmed);
        let mut spans = Vec::new();
        for mut segment in self.segments {
            segment.range.end = segment.range.end.min(trimmed);
            if segment.range.is_empty() {
                continue;
            }
            if segment.link.is_some() || segment.style.code {
                spans.push(segment);
                continue;
            }
            let mut cursor = segment.range.start;
            for found in bare_urls(&self.text[segment.range.clone()]) {
                let url = segment.range.start + found.start..segment.range.start + found.end;
                if cursor < url.start {
                    spans.push(Span {
                        range: cursor..url.start,
                        style: segment.style,
                        link: None,
                    });
                }
                self.links.push(self.text[url.clone()].to_owned());
                spans.push(Span {
                    range: url.clone(),
                    style: segment.style,
                    link: Some(self.links.len() - 1),
                });
                cursor = url.end;
            }
            if cursor < segment.range.end {
                spans.push(Span {
                    range: cursor..segment.range.end,
                    style: segment.style,
                    link: None,
                });
            }
        }
        spans.retain(|span| span.link.is_some() || span.style != InlineStyle::default());
        Inline {
            text: self.text,
            spans,
            links: self.links,
        }
    }
}

fn bare_urls(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("http") {
        let start = from + at;
        let rest = &text[start..];
        let scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let boundary = text[..start]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric());
        let length = rest
            .find(|character: char| character.is_whitespace() || "<>\"`".contains(character))
            .unwrap_or(rest.len());
        let url =
            rest[..length].trim_end_matches(|character: char| ".,;:!?)]}'*_".contains(character));
        if boundary && url.len() > scheme {
            found.push(start..start + url.len());
        }
        from = start + length.max(4);
    }
    found
}

enum Container {
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
        row: Vec<Inline>,
    },
    Code {
        language: String,
        text: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Paragraph,
    Heading(u8),
    Cell,
    Implicit,
}

#[derive(Default)]
struct Builder {
    containers: Vec<Container>,
    leaf: Option<(Leaf, InlineBuilder)>,
    top: Vec<(Range<usize>, Block)>,
    top_range: Range<usize>,
}

impl Builder {
    fn at_top(&self) -> bool {
        self.containers.is_empty() && self.leaf.is_none()
    }

    fn push_block(&mut self, block: Block) {
        match self.containers.last_mut() {
            Some(Container::Quote(blocks) | Container::Item { blocks, .. }) => blocks.push(block),
            Some(Container::List { items, .. }) => items.push(Item {
                task: None,
                blocks: vec![block],
            }),
            Some(Container::Table { .. } | Container::Code { .. }) => {}
            None => self.top.push((self.top_range.clone(), block)),
        }
    }

    fn open_leaf(&mut self, leaf: Leaf) {
        self.close_implicit();
        self.leaf = Some((leaf, InlineBuilder::default()));
    }

    fn close_leaf(&mut self) -> Option<(Leaf, Inline)> {
        self.leaf
            .take()
            .map(|(leaf, inline)| (leaf, inline.finish()))
    }

    fn close_implicit(&mut self) {
        if matches!(self.leaf, Some((Leaf::Implicit, _)))
            && let Some((_, inline)) = self.close_leaf()
            && !inline.text.is_empty()
        {
            self.push_block(Block::Paragraph(inline));
        }
    }

    fn inline(&mut self) -> &mut InlineBuilder {
        &mut self
            .leaf
            .get_or_insert_with(|| (Leaf::Implicit, InlineBuilder::default()))
            .1
    }

    fn text(&mut self, text: &str, code: bool) {
        if let Some(Container::Code { text: body, .. }) = self.containers.last_mut() {
            body.push_str(text);
        } else {
            self.inline().push(text, code);
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open_leaf(Leaf::Paragraph),
            Tag::Heading { level, .. } => self.open_leaf(Leaf::Heading(heading_level(level))),
            Tag::HtmlBlock => self.open_leaf(Leaf::Paragraph),
            Tag::BlockQuote(_) => {
                self.close_implicit();
                self.containers.push(Container::Quote(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|character: char| character.is_whitespace() || character == ',')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.containers.push(Container::Code {
                    language,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                self.close_implicit();
                self.containers.push(Container::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.close_implicit();
                self.containers.push(Container::Item {
                    task: None,
                    blocks: Vec::new(),
                });
            }
            Tag::Table(aligns) => {
                self.close_implicit();
                self.containers.push(Container::Table {
                    aligns: aligns.into_iter().map(align).collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableCell => self.open_leaf(Leaf::Cell),
            Tag::Emphasis => self.inline().italic += 1,
            Tag::Strong => self.inline().bold += 1,
            Tag::Strikethrough => self.inline().strike += 1,
            Tag::Link {
                link_type: LinkType::Email,
                dest_url,
                ..
            } if !dest_url.starts_with("mailto:") => {
                self.inline().open_link(&format!("mailto:{dest_url}"));
            }
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.inline().open_link(&dest_url);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                if let Some((_, inline)) = self.close_leaf() {
                    self.push_block(Block::Paragraph(inline));
                }
            }
            TagEnd::Heading(_) => {
                if let Some((Leaf::Heading(level), text)) = self.close_leaf() {
                    self.push_block(Block::Heading { level, text });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.close_implicit();
                if let Some(Container::Quote(blocks)) = self.containers.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::CodeBlock => {
                if let Some(Container::Code { language, mut text }) = self.containers.pop() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block::Code { language, text });
                }
            }
            TagEnd::List(_) => {
                self.close_implicit();
                if let Some(Container::List { start, items }) = self.containers.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.close_implicit();
                if let Some(Container::Item { task, blocks }) = self.containers.pop()
                    && let Some(Container::List { items, .. }) = self.containers.last_mut()
                {
                    items.push(Item { task, blocks });
                }
            }
            TagEnd::TableCell => {
                if let Some((_, inline)) = self.close_leaf()
                    && let Some(Container::Table { row, .. }) = self.containers.last_mut()
                {
                    row.push(inline);
                }
            }
            TagEnd::TableHead => {
                if let Some(Container::Table { head, row, .. }) = self.containers.last_mut() {
                    *head = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Container::Table { rows, row, .. }) = self.containers.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                if let Some(Container::Table {
                    aligns, head, rows, ..
                }) = self.containers.pop()
                {
                    self.push_block(Block::Table { aligns, head, rows });
                }
            }
            TagEnd::Emphasis => {
                let inline = self.inline();
                inline.italic = inline.italic.saturating_sub(1);
            }
            TagEnd::Strong => {
                let inline = self.inline();
                inline.bold = inline.bold.saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                let inline = self.inline();
                inline.strike = inline.strike.saturating_sub(1);
            }
            TagEnd::Link | TagEnd::Image => {
                self.inline().open_links.pop();
            }
            _ => {}
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn align(alignment: Alignment) -> Align {
    match alignment {
        Alignment::None | Alignment::Left => Align::Start,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::End,
    }
}

pub fn parse(source: &str) -> Vec<(Range<usize>, Block)> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder::default();
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        if builder.at_top() {
            builder.top_range = range;
        }
        match event {
            Event::Start(tag) => builder.start(tag),
            Event::End(tag) => builder.end(tag),
            Event::Text(text) => builder.text(&text, false),
            Event::Code(text) => builder.text(&text, true),
            Event::Html(text) | Event::InlineHtml(text) => builder.text(&text, false),
            Event::InlineMath(text) | Event::DisplayMath(text) => builder.text(&text, true),
            Event::FootnoteReference(label) => builder.text(&label, false),
            Event::SoftBreak | Event::HardBreak => builder.text("\n", false),
            Event::Rule => {
                builder.close_implicit();
                builder.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item { task, .. }) = builder.containers.last_mut() {
                    *task = Some(done);
                }
            }
        }
    }
    builder.close_implicit();
    builder.top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(source: &str) -> Vec<Block> {
        parse(source).into_iter().map(|(_, block)| block).collect()
    }

    fn paragraph(source: &str) -> Inline {
        match blocks(source).remove(0) {
            Block::Paragraph(inline) => inline,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn an_address_between_angle_brackets_is_a_mailto_link() {
        let inline = paragraph("Write to <crow@example.com> today.");
        assert_eq!(inline.text, "Write to crow@example.com today.");
        assert_eq!(inline.link_at(10), Some("mailto:crow@example.com"));
        let web = paragraph("See <https://example.com> today.");
        assert_eq!(web.link_at(5), Some("https://example.com"));
    }

    fn styled(inline: &Inline) -> Vec<(&str, InlineStyle, Option<&str>)> {
        inline
            .spans
            .iter()
            .map(|span| {
                (
                    &inline.text[span.range.clone()],
                    span.style,
                    span.link.map(|link| inline.links[link].as_str()),
                )
            })
            .collect()
    }

    const PLAIN: InlineStyle = InlineStyle {
        bold: false,
        italic: false,
        strike: false,
        code: false,
    };

    #[test]
    fn inline_styles_become_spans_over_the_shown_text() {
        let inline = paragraph("A **bold** and *slanted* and ~~gone~~ and `code` word.");
        assert_eq!(inline.text, "A bold and slanted and gone and code word.");
        assert_eq!(
            styled(&inline),
            [
                (
                    "bold",
                    InlineStyle {
                        bold: true,
                        ..PLAIN
                    },
                    None
                ),
                (
                    "slanted",
                    InlineStyle {
                        italic: true,
                        ..PLAIN
                    },
                    None
                ),
                (
                    "gone",
                    InlineStyle {
                        strike: true,
                        ..PLAIN
                    },
                    None
                ),
                (
                    "code",
                    InlineStyle {
                        code: true,
                        ..PLAIN
                    },
                    None
                ),
            ]
        );
    }

    #[test]
    fn links_keep_their_target_and_bare_addresses_become_links() {
        let inline = paragraph("See [the docs](https://docs.example/a) or https://example.com/x.");
        assert_eq!(inline.text, "See the docs or https://example.com/x.");
        assert_eq!(
            styled(&inline),
            [
                ("the docs", PLAIN, Some("https://docs.example/a")),
                (
                    "https://example.com/x",
                    PLAIN,
                    Some("https://example.com/x")
                ),
            ]
        );
        assert_eq!(inline.link_at(5), Some("https://docs.example/a"));
        assert_eq!(inline.link_at(0), None);

        let code = paragraph("`https://not.a.link`");
        assert_eq!(styled(&code)[0].2, None);
    }

    #[test]
    fn headings_rules_quotes_and_code_blocks_are_blocks() {
        let parsed =
            blocks("# Title\n\ntext\n\n---\n\n> quoted\n> more\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(
            parsed,
            [
                Block::Heading {
                    level: 1,
                    text: Inline::plain("Title"),
                },
                Block::Paragraph(Inline::plain("text")),
                Block::Rule,
                Block::Quote(vec![Block::Paragraph(Inline::plain("quoted\nmore"))]),
                Block::Code {
                    language: "rust".to_owned(),
                    text: "fn main() {}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn lists_keep_their_order_their_numbers_and_their_tasks() {
        let parsed =
            blocks("- one\n- two\n  - nested\n\n3. three\n4. four\n\n- [x] done\n- [ ] open\n");
        let texts = |items: &[Item]| -> Vec<String> {
            items
                .iter()
                .map(|item| match &item.blocks[0] {
                    Block::Paragraph(inline) => inline.text.clone(),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        let Block::List { start, items } = &parsed[0] else {
            panic!("{parsed:?}");
        };
        assert_eq!(*start, None);
        assert_eq!(texts(items), ["one", "two"]);
        assert!(
            matches!(&items[1].blocks[1], Block::List { items, .. } if texts(items) == ["nested"])
        );
        let Block::List { start, items } = &parsed[1] else {
            panic!("{parsed:?}");
        };
        assert_eq!(*start, Some(3));
        assert_eq!(texts(items), ["three", "four"]);
        let Block::List { items, .. } = &parsed[2] else {
            panic!("{parsed:?}");
        };
        assert_eq!(
            items.iter().map(|item| item.task).collect::<Vec<_>>(),
            [Some(true), Some(false)]
        );
        assert_eq!(texts(items), ["done", "open"]);
    }

    #[test]
    fn a_table_has_a_head_rows_and_alignments() {
        let parsed =
            blocks("| Name | Count | Note |\n|:--|--:|:-:|\n| a | 1 | **x** |\n| b | 2 | y |\n");
        let Block::Table { aligns, head, rows } = &parsed[0] else {
            panic!("{parsed:?}");
        };
        assert_eq!(aligns, &[Align::Start, Align::End, Align::Center]);
        assert_eq!(
            head.iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["Name", "Count", "Note"]
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][2].text, "x");
        assert!(rows[0][2].spans[0].style.bold);
        assert_eq!(rows[1][0].text, "b");
    }

    #[test]
    fn every_top_block_knows_where_it_starts_in_the_source() {
        let source = "first\n\n- a\n- b\n\nlast";
        let ranges: Vec<_> = parse(source)
            .into_iter()
            .map(|(range, _)| &source[range])
            .map(str::trim_end)
            .collect();
        assert_eq!(ranges, ["first", "- a\n- b", "last"]);
    }

    #[test]
    fn text_that_is_not_markdown_shows_as_written() {
        assert_eq!(
            blocks("<div>raw</div>"),
            [Block::Paragraph(Inline::plain("<div>raw</div>"))]
        );
        assert_eq!(
            blocks("a * b _ c ` d"),
            [Block::Paragraph(Inline::plain("a * b _ c ` d"))]
        );
        assert_eq!(
            blocks("[broken](link"),
            [Block::Paragraph(Inline::plain("[broken](link"))]
        );
        assert!(blocks("").is_empty());
    }
}
