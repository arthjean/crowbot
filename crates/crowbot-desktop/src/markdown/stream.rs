use std::borrow::Cow;
use std::rc::Rc;

use super::parse::{Block, Inline, parse};

#[derive(Default)]
pub struct Document {
    source: String,
    closed: Vec<Rc<Block>>,
    closed_end: usize,
    tail: Vec<Rc<Block>>,
    streaming: bool,
    parsed_bytes: usize,
    parse_starts: Vec<usize>,
}

impl Document {
    pub fn plain(text: &str) -> Self {
        Self {
            source: text.to_owned(),
            tail: vec![Rc::new(Block::Paragraph(Inline::plain(text)))],
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn markdown(text: &str) -> Self {
        let mut document = Self::default();
        document.set(text, false);
        document
    }

    #[cfg(test)]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[cfg(test)]
    pub fn parse_starts(&self) -> &[usize] {
        &self.parse_starts
    }

    pub fn blocks(&self) -> Vec<Rc<Block>> {
        self.closed.iter().chain(&self.tail).cloned().collect()
    }

    pub fn text(&self) -> String {
        rendered_text(&self.blocks())
    }

    pub fn set(&mut self, text: &str, streaming: bool) {
        if text == self.source && streaming == self.streaming && !self.is_empty() {
            return;
        }
        if !text.starts_with(self.source.as_str()) {
            *self = Self::default();
        }
        self.source.push_str(&text[self.source.len()..]);
        self.streaming = streaming;
        let tail = &self.source[self.closed_end..];
        let guarded = if streaming {
            guard(tail)
        } else {
            Cow::Borrowed(tail)
        };
        self.parsed_bytes += guarded.len();
        if cfg!(test) {
            self.parse_starts.push(self.closed_end);
        }
        let mut blocks = parse(&guarded);
        // The open block starts at its line: an indented code block is
        // reported after its indent, and the next parse needs that indent. A
        // block that starts in text the guard added closes nothing.
        let open_from = blocks
            .last()
            .filter(|_| streaming && blocks.len() > 1)
            .and_then(|(range, _)| tail.get(..range.start))
            .filter(|before| before.len() < tail.len())
            .map(|before| before.rfind('\n').map_or(0, |at| at + 1))
            .filter(|start| *start > 0);
        if let Some(start) = open_from
            && let Some((_, last)) = blocks.pop()
        {
            self.closed
                .extend(blocks.into_iter().map(|(_, block)| Rc::new(block)));
            self.closed_end += start;
            self.tail = vec![Rc::new(last)];
        } else {
            self.tail = blocks
                .into_iter()
                .map(|(_, block)| Rc::new(block))
                .collect();
        }
    }

    fn is_empty(&self) -> bool {
        self.closed.is_empty() && self.tail.is_empty()
    }
}

pub struct Piece<'a> {
    pub text: &'a str,
    pub after: char,
}

pub fn pieces(blocks: &[Rc<Block>]) -> Vec<Piece<'_>> {
    let mut found = Vec::new();
    for block in blocks {
        block_pieces(block, &mut found);
    }
    found
}

fn block_pieces<'a>(block: &'a Block, found: &mut Vec<Piece<'a>>) {
    match block {
        Block::Paragraph(inline) | Block::Heading { text: inline, .. } => found.push(Piece {
            text: &inline.text,
            after: '\n',
        }),
        Block::Code { text, .. } => found.push(Piece { text, after: '\n' }),
        Block::Quote(blocks) => {
            for block in blocks {
                block_pieces(block, found);
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for block in &item.blocks {
                    block_pieces(block, found);
                }
            }
        }
        Block::Table { head, rows, .. } => {
            for row in std::iter::once(head).chain(rows) {
                for (index, cell) in row.iter().enumerate() {
                    found.push(Piece {
                        text: &cell.text,
                        after: if index + 1 == row.len() { '\n' } else { '\t' },
                    });
                }
            }
        }
        Block::Rule => {}
    }
}

pub fn rendered_text(blocks: &[Rc<Block>]) -> String {
    let mut text = String::new();
    for piece in pieces(blocks) {
        text.push_str(piece.text);
        text.push(piece.after);
    }
    text.pop();
    text
}

fn open_fence(text: &str) -> Option<(char, usize)> {
    let mut open: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(marker) = trimmed.chars().next().filter(|first| "`~".contains(*first)) else {
            continue;
        };
        let length = trimmed.chars().take_while(|next| *next == marker).count();
        if length < 3 {
            continue;
        }
        match open {
            None => open = Some((marker, length)),
            Some((opened, opened_length)) => {
                if opened == marker
                    && length >= opened_length
                    && trimmed[length..].trim().is_empty()
                {
                    open = None;
                }
            }
        }
    }
    open
}

fn is_partial_marker(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let all = |allowed: &str| trimmed.chars().all(|next| allowed.contains(next));
    if (all("`") || all("~")) && trimmed.len() < 3 {
        return true;
    }
    if all("-=_*+ ") || all("#") && trimmed.len() <= 6 || all("> ") || all("| ") {
        return true;
    }
    let digits = trimmed.trim_end_matches([' ', '.', ')']);
    if digits.len() <= 9 && digits.chars().all(|next| next.is_ascii_digit()) {
        return true;
    }
    let task = trimmed
        .strip_prefix(['-', '*', '+'])
        .map(str::trim_start)
        .filter(|rest| rest.starts_with('['));
    task.is_some_and(|rest| rest.len() <= 3 && rest[1..].chars().all(|next| " xX]".contains(next)))
}

fn starts_row(line: &str) -> bool {
    // Four spaces or a tab make the line code, not a table row.
    let indent = line.len() - line.trim_start_matches(' ').len();
    indent < 4 && line[indent..].starts_with('|')
}

fn columns(row: &str) -> usize {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = match row.strip_suffix('|') {
        Some(inner) if !inner.ends_with('\\') => inner,
        _ => row,
    };
    let mut count = 1;
    let mut escaped = false;
    for next in row.chars() {
        match next {
            '\\' => escaped = !escaped,
            '|' if !escaped => count += 1,
            _ => escaped = false,
        }
    }
    count
}

fn delimiter_row(columns: usize) -> String {
    let mut row = "|".to_owned();
    for _ in 0..columns {
        row.push_str("---|");
    }
    row.push('\n');
    row
}

fn becoming_table(text: &mut String) {
    let complete = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let Some(last) = lines.last().copied() else {
        return;
    };
    let before = |back: usize| {
        lines
            .len()
            .checked_sub(back + 1)
            .and_then(|index| lines.get(index))
            .copied()
    };
    if starts_row(last) && !before(1).is_some_and(starts_row) {
        let row = delimiter_row(columns(last));
        if !complete {
            text.push('\n');
        }
        text.push_str(&row);
        return;
    }
    let delimiter_prefix =
        !last.trim().is_empty() && last.chars().all(|next| "|:- ".contains(next));
    if !complete
        && delimiter_prefix
        && let Some(header) = before(1).filter(|header| starts_row(header))
        && !before(2).is_some_and(starts_row)
    {
        let row = delimiter_row(columns(header));
        let kept = text.len() - last.len();
        text.truncate(kept);
        text.push_str(&row);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delimiter {
    Strong,
    Emphasis,
    Strike,
}

impl Delimiter {
    fn marker(self) -> &'static str {
        match self {
            Self::Strong => "**",
            Self::Emphasis => "*",
            Self::Strike => "~~",
        }
    }
}

fn close_inline(text: &mut String) {
    let region_start = text.rfind("\n\n").map_or(0, |at| at + 2);
    let region = &text[region_start..];
    let characters: Vec<(usize, char)> = region.char_indices().collect();
    let mut code: Option<(usize, usize)> = None;
    let mut open: Vec<(Delimiter, usize)> = Vec::new();
    let mut index = 0;
    let run_of = |from: usize, marker: char| {
        characters[from..]
            .iter()
            .take_while(|(_, next)| *next == marker)
            .count()
    };
    while index < characters.len() {
        let (at, character) = characters[index];
        if character == '`' {
            let run = run_of(index, '`');
            match code {
                None => code = Some((run, at)),
                Some((opened, _)) if opened == run => code = None,
                Some(_) => {}
            }
            index += run;
            continue;
        }
        if code.is_some() {
            index += 1;
            continue;
        }
        if character == '\\' {
            index += 2;
            continue;
        }
        if character == '*' || character == '~' {
            let run = run_of(index, character);
            let before = index.checked_sub(1).map(|previous| characters[previous].1);
            let after = characters.get(index + run).map(|next| next.1);
            let can_close = before.is_some_and(|previous| !previous.is_whitespace());
            let can_open = after.is_some_and(|next| !next.is_whitespace());
            let at_end = after.is_none();
            let mut left = run;
            if character == '~' {
                if run >= 2 {
                    match open
                        .iter()
                        .rposition(|(kind, _)| *kind == Delimiter::Strike)
                    {
                        Some(position) if can_close => {
                            open.truncate(position);
                        }
                        _ if can_open || at_end => open.push((Delimiter::Strike, at)),
                        _ => {}
                    }
                }
                index += run;
                continue;
            }
            while left > 0 && can_close {
                match open.last() {
                    Some((Delimiter::Strong, _)) if left >= 2 => {
                        open.pop();
                        left -= 2;
                    }
                    Some((Delimiter::Emphasis, _)) => {
                        open.pop();
                        left -= 1;
                    }
                    _ => break,
                }
            }
            if can_open || at_end {
                let mut offset = at + (run - left);
                while left >= 2 {
                    open.push((Delimiter::Strong, offset));
                    offset += 2;
                    left -= 2;
                }
                if left == 1 {
                    open.push((Delimiter::Emphasis, offset));
                }
            }
            index += run;
            continue;
        }
        index += 1;
    }

    let region_length = region.len();
    let mut cut = region_length;
    let mut closers = String::new();
    if let Some((run, at)) = code {
        if at + run == region_length {
            cut = at;
        } else {
            closers.push_str(&"`".repeat(run));
        }
    }
    for (kind, at) in open.iter().rev() {
        if *at >= cut {
            continue;
        }
        if region[..cut].trim_end().len() <= at + kind.marker().len() {
            cut = *at;
        } else {
            closers.push_str(kind.marker());
        }
    }
    let kept = region_start + cut;
    text.truncate(kept);
    if !closers.is_empty() {
        let trimmed = text.trim_end().len();
        text.truncate(trimmed);
        text.push_str(&closers);
    }
    let region = &text[region_start..];
    if let Some(at) = region.rfind("](")
        && !region[at..].contains(')')
        && region[..at].contains('[')
        && !region[at..].contains(char::is_whitespace)
    {
        text.push(')');
    }
}

fn guard(tail: &str) -> Cow<'_, str> {
    let (body, last) = match tail.rfind('\n') {
        Some(at) => tail.split_at(at + 1),
        None => ("", tail),
    };
    if let Some((marker, length)) = open_fence(tail) {
        let partial_close = {
            let trimmed = last.trim();
            !trimmed.is_empty()
                && trimmed.len() < length
                && trimmed.chars().all(|next| next == marker)
                && open_fence(body).is_some()
        };
        return Cow::Borrowed(if partial_close { body } else { tail });
    }
    let mut text = if is_partial_marker(last) { body } else { tail }.to_owned();
    becoming_table(&mut text);
    close_inline(&mut text);
    if text == tail {
        Cow::Borrowed(tail)
    } else {
        Cow::Owned(text)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn streamed(text: &str) -> Vec<Rc<Block>> {
        let mut document = Document::default();
        document.set(text, true);
        document.blocks()
    }

    fn shown(text: &str) -> String {
        rendered_text(&streamed(text))
    }

    fn first(text: &str) -> Block {
        (*streamed(text)[0]).clone()
    }

    const SAMPLE: &str = "## Plan\n\nA **bold** start with `code` and a [link](https://example.com).\n\n- first item\n- second item\n\n1. one\n2. two\n\n> quoted line\n\n| Name | Count |\n|------|------:|\n| a | 1 |\n| b | 2 |\n\n---\n\n```rust\nfn main() {\n    println!(\"caw\");\n}\n```\n\nThe end.";

    #[test]
    fn blocks_already_closed_are_not_parsed_again() {
        let paragraph = "word ".repeat(40);
        let message: String = (0..100).map(|_| format!("{paragraph}\n\n")).collect();
        assert!(message.len() >= 20_000);
        let mut document = Document::default();
        let mut slowest = Duration::ZERO;
        let mut end = 0;
        while end < message.len() {
            end = (end + 20).min(message.len());
            let started = Instant::now();
            document.set(&message[..end], true);
            slowest = slowest.max(started.elapsed());
        }
        let steps = message.len() / 20;
        assert!(
            document.parsed_bytes < steps * 2 * (paragraph.len() + 22),
            "{} bytes parsed for {} bytes of text",
            document.parsed_bytes,
            message.len()
        );
        let mut furthest = 0;
        for start in &document.parse_starts {
            assert!(*start >= furthest);
            furthest = *start;
        }
        assert!(furthest > message.len() - 2 * (paragraph.len() + 2));
        assert_eq!(document.blocks().len(), 100);
        assert!(slowest < Duration::from_millis(16), "{slowest:?}");
    }

    #[test]
    fn a_closed_block_keeps_its_place_while_the_tail_grows() {
        let mut document = Document::default();
        document.set("First paragraph.\n\nSecond", true);
        let before = document.blocks();
        document.set("First paragraph.\n\nSecond one grows.\n\nThird", true);
        let after = document.blocks();
        assert!(Rc::ptr_eq(&before[0], &after[0]));
        assert_eq!(after.len(), 3);
        assert_eq!(
            rendered_text(&after),
            "First paragraph.\nSecond one grows.\nThird"
        );
    }

    #[test]
    fn the_stream_ends_with_the_same_blocks_as_the_whole_text() {
        let mut document = Document::default();
        let mut end = 0;
        while end < SAMPLE.len() {
            end += 1;
            while !SAMPLE.is_char_boundary(end) {
                end += 1;
            }
            document.set(&SAMPLE[..end], true);
            let text = document.text();
            for raw in ["**", "```", "|--", "\n- ", "## "] {
                assert!(!text.contains(raw), "{raw:?} shows in {text:?}");
            }
        }
        document.set(SAMPLE, false);
        let whole: Vec<Block> = parse(SAMPLE).into_iter().map(|(_, block)| block).collect();
        let streamed: Vec<Block> = document
            .blocks()
            .iter()
            .map(|block| (**block).clone())
            .collect();
        assert_eq!(streamed, whole);
    }

    fn streamed_by_character(text: &str) -> Vec<Block> {
        let mut document = Document::default();
        for end in 1..=text.len() {
            if text.is_char_boundary(end) {
                document.set(&text[..end], true);
                let shown = document.text();
                assert!(!shown.contains("|--"), "{shown:?} at {end} of {text:?}");
            }
        }
        document.set(text, false);
        let blocks = document.blocks();
        blocks.iter().map(|block| (**block).clone()).collect()
    }

    fn whole(text: &str) -> Vec<Block> {
        parse(text).into_iter().map(|(_, block)| block).collect()
    }

    #[test]
    fn indented_code_ends_a_stream_as_the_blocks_of_the_whole_text() {
        for text in [
            "Run:\n\n    npm install\n    npm test",
            "Run:\n\n    npm install\n    npm test\n\nThen wait.",
            "Run:\n\n    cat notes\n    | grep é",
            "Run:\n\n    cat notes\n    | grep é\n\nThen wait.",
            "Run:\n\n\tcat notes\n\t| sort",
        ] {
            assert_eq!(streamed_by_character(text), whole(text), "{text:?}");
        }
    }

    #[test]
    fn an_unclosed_code_fence_shows_as_the_code_block_it_becomes() {
        assert_eq!(
            first("```rust\nfn main() {"),
            Block::Code {
                language: "rust".to_owned(),
                text: "fn main() {".to_owned(),
            }
        );
        assert_eq!(
            first("```ru"),
            Block::Code {
                language: "ru".to_owned(),
                text: String::new(),
            }
        );
        assert_eq!(shown("Text\n\n``"), "Text");
        assert_eq!(shown("```\ncode\n``"), "code");
        assert_eq!(shown("```\na * b\n**"), "a * b\n**");
    }

    #[test]
    fn an_unclosed_table_shows_as_the_table_it_becomes() {
        for partial in [
            "| Name | Count",
            "| Name | Count |",
            "| Name | Count |\n",
            "| Name | Count |\n|",
            "| Name | Count |\n|---",
            "| Name | Count |\n|---|--",
        ] {
            let Block::Table { head, rows, .. } = first(partial) else {
                panic!("{partial:?} is not a table");
            };
            assert_eq!(head.len(), 2, "{partial:?}");
            assert_eq!(head[0].text, "Name");
            assert!(rows.is_empty());
        }
        let Block::Table { head, rows, .. } = first("| Name | Count |\n|---|---|\n| a | 1") else {
            panic!("not a table");
        };
        assert_eq!(head.len(), 2);
        assert_eq!(rows[0][1].text, "1");
        assert_eq!(shown("Intro\n\n| A"), "Intro\nA");
    }

    #[test]
    fn an_unclosed_list_shows_as_the_list_it_becomes() {
        assert_eq!(shown("Intro\n-"), "Intro");
        assert_eq!(shown("Intro\n- "), "Intro");
        assert!(matches!(first("- one"), Block::List { .. }));
        assert!(matches!(first("Intro\n- one"), Block::Paragraph(_)));
        assert!(matches!(&*streamed("Intro\n- one")[1], Block::List { .. }));
        assert_eq!(shown("- one\n1"), "one");
        assert_eq!(shown("- one\n2."), "one");
        assert!(
            matches!(first("3. three"), Block::List { start: Some(3), .. }),
            "an ordered list keeps its number"
        );
        assert_eq!(shown("- [ ]"), "");
        let Block::List { items, .. } = first("- [x] done") else {
            panic!("not a list");
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(shown("Title\n=="), "Title");
        assert_eq!(shown("#"), "");
        assert!(matches!(first("# T"), Block::Heading { level: 1, .. }));
        assert_eq!(shown(">"), "");
    }

    #[test]
    fn unclosed_inline_marks_show_as_the_style_they_become() {
        let bold = |text: &str| match first(text) {
            Block::Paragraph(inline) => (
                inline.text.clone(),
                inline.spans.iter().any(|span| span.style.bold),
            ),
            other => panic!("{other:?}"),
        };
        assert_eq!(bold("A **bo"), ("A bo".to_owned(), true));
        assert_eq!(bold("A **bold "), ("A bold".to_owned(), true));
        assert_eq!(bold("A **bold** b"), ("A bold b".to_owned(), true));
        assert_eq!(shown("A **"), "A");
        assert_eq!(shown("A *"), "A");
        assert_eq!(shown("A *slanted"), "A slanted");
        assert_eq!(shown("A ~~gone"), "A gone");
        assert_eq!(shown("A `co"), "A co");
        assert_eq!(shown("A `"), "A");
        assert_eq!(shown("2 * 3 = 6"), "2 * 3 = 6");
        assert_eq!(shown("See [docs](https://exa"), "See docs");
        assert_eq!(shown("**Title:** then *more"), "Title: then more");
    }

    #[test]
    fn text_never_disappears_while_it_streams() {
        let mut document = Document::default();
        let mut longest = 0;
        for end in 1..=SAMPLE.len() {
            if !SAMPLE.is_char_boundary(end) {
                continue;
            }
            document.set(&SAMPLE[..end], true);
            let length = document.text().len();
            assert!(
                length + 12 >= longest,
                "text shrank from {longest} to {length} at {end}"
            );
            longest = longest.max(length);
        }
        assert!(document.text().starts_with("Plan\nA bold start"));
    }

    #[test]
    fn a_text_that_is_replaced_starts_over() {
        let mut document = Document::default();
        document.set("One.\n\nTwo.\n\nThree", true);
        document.set("Other", true);
        assert_eq!(document.text(), "Other");
        assert_eq!(document.source(), "Other");
    }

    #[test]
    fn plain_text_is_one_block_shown_as_written() {
        let document = Document::plain("**not bold**\n- not a list");
        assert_eq!(document.text(), "**not bold**\n- not a list");
        assert_eq!(document.blocks().len(), 1);
        assert_eq!(Document::markdown("**bold**").text(), "bold");
    }

    #[test]
    fn the_shown_text_joins_blocks_with_line_breaks_and_cells_with_tabs() {
        let document = Document::markdown(
            "Intro\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n```\ncode\nmore\n```",
        );
        assert_eq!(document.text(), "Intro\na\nb\nx\ty\n1\t2\ncode\nmore");
    }
}
