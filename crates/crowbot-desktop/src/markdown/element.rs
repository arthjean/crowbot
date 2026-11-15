use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, ElementId, Font, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StrikethroughStyle,
    StyledText, TextLayout, TextRun, UnderlineStyle, Window, div, fill, point, prelude::*,
};
use unicode_segmentation::UnicodeSegmentation;

use super::parse::{Align, Block, Inline, Item};
use crate::assets::Icon;
use crate::theme::{Theme, control, font, icon as icon_size, layout, radius, size, space, text};
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub key: i64,
    pub anchor: usize,
    pub head: usize,
    origin: (usize, usize),
    dragging: bool,
}

impl Selection {
    /// A selection that covers a range of the rendered text of a message.
    pub fn spanning(key: i64, range: Range<usize>) -> Self {
        Self {
            key,
            anchor: range.start,
            head: range.end,
            origin: (range.start, range.end),
            dragging: false,
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    fn extend(&mut self, offset: usize) {
        let (start, end) = self.origin;
        if offset < start {
            self.anchor = end;
            self.head = offset;
        } else if offset > end {
            self.anchor = start;
            self.head = offset;
        } else {
            self.anchor = start;
            self.head = end;
        }
    }
}

pub type SharedSelection = Rc<RefCell<Option<Selection>>>;

pub type LinkHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
pub type CodeHandler = Rc<dyn Fn(usize, &str, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct Actions {
    pub open_link: LinkHandler,
    pub copy_code: CodeHandler,
    pub copied_code: Option<usize>,
}

struct Geometry {
    start: usize,
    text: SharedString,
    layout: TextLayout,
    /// The part of the text that shows: a tall code block scrolls inside a
    /// box, and its hidden lines must not take the clicks of what follows.
    visible: Bounds<Pixels>,
    links: Rc<[(Range<usize>, SharedString)]>,
}

type Registry = Rc<RefCell<Vec<Geometry>>>;

struct Hit {
    chunk: usize,
    local: usize,
    on_rows: bool,
    on_glyph: bool,
}

fn outside(value: Pixels, low: Pixels, high: Pixels) -> f32 {
    f32::from(low - value).max(f32::from(value - high)).max(0.)
}

fn locate(chunks: &[Geometry], position: Point<Pixels>) -> Option<Hit> {
    let distance = |chunk: &Geometry| {
        let bounds = chunk.visible;
        (
            outside(position.y, bounds.top(), bounds.bottom()),
            outside(position.x, bounds.left(), bounds.right()),
        )
    };
    let (index, chunk, (above, _)) = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| (index, chunk, distance(chunk)))
        .min_by(|(_, _, first), (_, _, second)| {
            first
                .0
                .total_cmp(&second.0)
                .then(first.1.total_cmp(&second.1))
        })?;
    let found = chunk.layout.index_for_position(position);
    let (Ok(local) | Err(local)) = found;
    Some(Hit {
        chunk: index,
        local: local.min(chunk.text.len()),
        on_rows: above == 0.,
        on_glyph: found.is_ok(),
    })
}

fn link_at(chunks: &[Geometry], position: Point<Pixels>) -> Option<SharedString> {
    let hit = locate(chunks, position).filter(|hit| hit.on_glyph)?;
    chunks[hit.chunk]
        .links
        .iter()
        .find(|(range, _)| range.contains(&hit.local))
        .map(|(_, target)| target.clone())
}

fn word_at(text: &str, local: usize) -> Range<usize> {
    text.split_word_bound_indices()
        .map(|(start, word)| start..start + word.len())
        .find(|word| local < word.end)
        .unwrap_or(local..local)
}

fn selection_quads(layout: &TextLayout, range: Range<usize>) -> Vec<Bounds<Pixels>> {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return Vec::new();
    };
    let (bounds, row) = (layout.bounds(), layout.line_height());
    if start.y == end.y {
        return vec![Bounds::from_corners(start, point(end.x, end.y + row))];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        point(bounds.right(), start.y + row),
    )];
    if end.y > start.y + row {
        quads.push(Bounds::from_corners(
            point(bounds.left(), start.y + row),
            point(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        point(bounds.left(), end.y),
        point(end.x, end.y + row),
    ));
    quads
}

struct Chunk {
    key: i64,
    start: usize,
    shared: SharedString,
    text: StyledText,
    links: Rc<[(Range<usize>, SharedString)]>,
    registry: Registry,
    selection: SharedSelection,
    highlight: Hsla,
}

impl Element for Chunk {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, None, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(None, None, bounds, &mut (), window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.text.layout().clone();
        layout.paint_background(window, cx).ok();
        let end = self.start + self.shared.len();
        let selected = self
            .selection
            .borrow()
            .filter(|selection| selection.key == self.key)
            .map(|selection| selection.range())
            .filter(|range| {
                range.start < range.end && range.start <= end && range.end > self.start
            });
        if let Some(range) = selected {
            let local = range.start.saturating_sub(self.start)..range.end.min(end) - self.start;
            for quad in selection_quads(&layout, local) {
                window.paint_quad(fill(quad, self.highlight));
            }
        }
        layout.paint_foreground(window, cx).ok();
        let visible = layout.bounds().intersect(&window.content_mask().bounds);
        self.registry.borrow_mut().push(Geometry {
            start: self.start,
            text: self.shared.clone(),
            visible,
            layout,
            links: self.links.clone(),
        });
    }
}

pub struct MessageText {
    key: i64,
    child: AnyElement,
    registry: Registry,
    selection: SharedSelection,
    open_link: LinkHandler,
}

impl Element for MessageText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.child.prepaint(window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let chunks: Rc<Vec<Geometry>> = Rc::new(self.registry.take());
        let over_link = Rc::new(Cell::new(false));
        if hitbox.is_hovered(window) {
            let pointer = window.mouse_position();
            if link_at(&chunks, pointer).is_some() {
                over_link.set(true);
                window.set_cursor_style(CursorStyle::PointingHand, hitbox);
            } else if locate(&chunks, pointer).is_some_and(|hit| hit.on_rows) {
                window.set_cursor_style(CursorStyle::IBeam, hitbox);
            }
        }
        let key = self.key;

        window.on_mouse_event({
            let (chunks, selection, hitbox) =
                (chunks.clone(), self.selection.clone(), hitbox.clone());
            move |event: &MouseDownEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let Some(hit) = locate(&chunks, event.position).filter(|hit| hit.on_rows) else {
                    return;
                };
                let chunk = &chunks[hit.chunk];
                let local = match event.click_count {
                    1 => hit.local..hit.local,
                    2 => word_at(&chunk.text, hit.local),
                    _ => 0..chunk.text.len(),
                };
                let origin = (chunk.start + local.start, chunk.start + local.end);
                *selection.borrow_mut() = Some(Selection {
                    key,
                    anchor: origin.0,
                    head: origin.1,
                    origin,
                    dragging: true,
                });
                window.refresh();
            }
        });

        window.on_mouse_event({
            let (chunks, selection, hitbox) =
                (chunks.clone(), self.selection.clone(), hitbox.clone());
            move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let mut current = selection.borrow_mut();
                let dragged = current
                    .as_mut()
                    .filter(|selection| selection.key == key && selection.dragging);
                let Some(dragged) = dragged else {
                    let link =
                        hitbox.is_hovered(window) && link_at(&chunks, event.position).is_some();
                    if link != over_link.get() {
                        window.refresh();
                    }
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    dragged.dragging = false;
                    return;
                }
                let Some(hit) = locate(&chunks, event.position) else {
                    return;
                };
                let before = *dragged;
                dragged.extend(chunks[hit.chunk].start + hit.local);
                if *dragged != before {
                    window.refresh();
                }
            }
        });

        window.on_mouse_event({
            let (selection, open_link) = (self.selection.clone(), self.open_link.clone());
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let caret = {
                    let mut current = selection.borrow_mut();
                    let Some(released) = current
                        .as_mut()
                        .filter(|selection| selection.key == key && selection.dragging)
                    else {
                        return;
                    };
                    released.dragging = false;
                    let caret = released.anchor == released.head;
                    if caret {
                        *current = None;
                    }
                    caret
                };
                window.refresh();
                if caret
                    && event.click_count == 1
                    && let Some(target) = link_at(&chunks, event.position)
                {
                    open_link(&target, window, cx);
                }
            }
        });
    }
}

type Links = Vec<(Range<usize>, SharedString)>;

fn code_run(code: &str, theme: &Theme) -> TextRun {
    TextRun {
        len: code.len(),
        font: font::mono(),
        color: theme.text.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

fn inline_runs(inline: &Inline, tone: Tone, theme: &Theme) -> (Vec<TextRun>, Links) {
    let base = |length: usize| TextRun {
        len: length,
        font: Font {
            weight: tone.weight,
            ..font::sans()
        },
        color: tone.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut links = Vec::new();
    let mut cursor = 0;
    for span in &inline.spans {
        if span.range.start < cursor || span.range.end > inline.text.len() {
            continue;
        }
        if span.range.start > cursor {
            runs.push(base(span.range.start - cursor));
        }
        let mut run = base(span.range.len());
        if span.style.code {
            run.font = Font {
                weight: tone.weight,
                ..font::mono()
            };
            run.background_color = Some(theme.hover.into());
        }
        if span.style.bold {
            run.font.weight = FontWeight::SEMIBOLD;
        }
        if span.style.italic {
            run.font.style = FontStyle::Italic;
        }
        if span.style.strike {
            run.strikethrough = Some(StrikethroughStyle {
                thickness: size::HAIRLINE,
                color: None,
            });
        }
        if let Some(target) = span.link.and_then(|link| inline.links.get(link)) {
            run.color = theme.accent_text.into();
            run.underline = Some(UnderlineStyle {
                thickness: size::HAIRLINE,
                color: None,
                wavy: false,
            });
            links.push((span.range.clone(), SharedString::from(target.clone())));
        }
        runs.push(run);
        cursor = span.range.end;
    }
    if cursor < inline.text.len() {
        runs.push(base(inline.text.len() - cursor));
    }
    (runs, links)
}

struct Builder<'a> {
    key: i64,
    offset: usize,
    code_blocks: usize,
    registry: Registry,
    selection: &'a SharedSelection,
    actions: &'a Actions,
    theme: &'a Theme,
}

#[derive(Clone, Copy)]
struct Tone {
    color: Hsla,
    weight: FontWeight,
}

impl Builder<'_> {
    fn chunk(
        &mut self,
        content: &str,
        runs: Vec<TextRun>,
        links: Vec<(Range<usize>, SharedString)>,
    ) -> Chunk {
        let shared = SharedString::from(content.to_owned());
        let start = self.offset;
        self.offset += content.len() + 1;
        Chunk {
            key: self.key,
            start,
            text: StyledText::new(shared.clone()).with_runs(runs),
            shared,
            links: links.into(),
            registry: self.registry.clone(),
            selection: self.selection.clone(),
            highlight: self.theme.accent_strong.into(),
        }
    }

    fn inline(&mut self, inline: &Inline, tone: Tone) -> Chunk {
        let (runs, links) = inline_runs(inline, tone, self.theme);
        self.chunk(&inline.text, runs, links)
    }

    fn blocks(&mut self, blocks: &[Block], tone: Tone) -> Vec<AnyElement> {
        blocks.iter().map(|block| self.block(block, tone)).collect()
    }

    fn block(&mut self, block: &Block, tone: Tone) -> AnyElement {
        let theme = self.theme;
        match block {
            Block::Paragraph(inline) => div()
                .debug_selector(|| "md-paragraph".into())
                .child(self.inline(inline, tone))
                .into_any_element(),
            Block::Heading { level, text } => {
                let step = match level {
                    1 => text::XXL,
                    2 => text::XL,
                    _ => text::LG,
                };
                let heading = Tone {
                    weight: FontWeight::SEMIBOLD,
                    ..tone
                };
                div()
                    .debug_selector(|| "md-heading".into())
                    .text_size(step.size)
                    .line_height(step.leading)
                    .child(self.inline(text, heading))
                    .into_any_element()
            }
            Block::Code { language, text } => self.code(language, text),
            Block::Quote(blocks) => {
                let quoted = Tone {
                    color: theme.text_secondary.into(),
                    ..tone
                };
                div()
                    .debug_selector(|| "md-quote".into())
                    .border_l(size::QUOTE_BAR)
                    .border_color(theme.border_focus)
                    .pl(space::S3)
                    .flex()
                    .flex_col()
                    .gap(layout::BLOCK_GAP)
                    .children(self.blocks(blocks, quoted))
                    .into_any_element()
            }
            Block::List { start, items } => {
                let rows: Vec<AnyElement> = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| self.item(*start, index, item, tone))
                    .collect();
                div()
                    .debug_selector(|| "md-list".into())
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .children(rows)
                    .into_any_element()
            }
            Block::Table { aligns, head, rows } => self.table(aligns, head, rows, tone),
            Block::Rule => div()
                .debug_selector(|| "md-rule".into())
                .h(size::HAIRLINE)
                .my(space::S1)
                .bg(theme.border_focus)
                .into_any_element(),
        }
    }

    fn item(&mut self, start: Option<u64>, index: usize, item: &Item, tone: Tone) -> AnyElement {
        let theme = self.theme;
        let marker = match (item.task, start) {
            (Some(done), _) => div()
                .debug_selector(move || {
                    if done {
                        "md-task-done".into()
                    } else {
                        "md-task-open".into()
                    }
                })
                .mt(space::S0_5)
                .size(size::CHECKBOX)
                .rounded(radius::SM)
                .border_1()
                .border_color(theme.border_focus)
                .flex()
                .items_center()
                .justify_center()
                .when(done, |checkbox| {
                    checkbox
                        .bg(theme.accent)
                        .border_color(theme.accent)
                        .child(ui::icon(Icon::Check, size::BADGE_ICON, theme.on_accent))
                })
                .into_any_element(),
            (None, Some(first)) => {
                SharedString::from(format!("{}.", first + index as u64)).into_any_element()
            }
            (None, None) => "•".into_any_element(),
        };
        div()
            .flex()
            .items_start()
            .child(
                div()
                    .flex_none()
                    .min_w(layout::LIST_INDENT)
                    .pr(space::S1_5)
                    .flex()
                    .justify_end()
                    .text_color(theme.text_muted)
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S1)
                    .children(self.blocks(&item.blocks, tone)),
            )
            .into_any_element()
    }

    fn table(
        &mut self,
        aligns: &[Align],
        head: &[Inline],
        rows: &[Vec<Inline>],
        tone: Tone,
    ) -> AnyElement {
        let theme = self.theme;
        let columns = head.len().max(1);
        let mut cells = Vec::new();
        for (row_index, row) in std::iter::once(head)
            .chain(rows.iter().map(Vec::as_slice))
            .enumerate()
        {
            for (column, cell) in row.iter().enumerate() {
                let heading = row_index == 0;
                let cell_tone = Tone {
                    weight: if heading {
                        FontWeight::MEDIUM
                    } else {
                        tone.weight
                    },
                    ..tone
                };
                let chunk = self.inline(cell, cell_tone);
                if column >= columns {
                    continue;
                }
                let aligned = div()
                    .min_w(layout::TABLE_MIN_COLUMN)
                    .px(space::S3)
                    .py(space::S2)
                    .border_color(theme.border_strong)
                    .when(row_index > 0, |cell| cell.border_t_1())
                    .when(column > 0, |cell| cell.border_l_1())
                    .when(heading, |cell| cell.bg(theme.hover));
                let aligned = match aligns.get(column) {
                    Some(Align::Center) => aligned.text_center(),
                    Some(Align::End) => aligned.text_right(),
                    _ => aligned,
                };
                cells.push(aligned.child(chunk));
            }
        }
        let table = self.code_blocks;
        self.code_blocks += 1;
        div()
            .id(("md-table", table))
            .debug_selector(|| "md-table".into())
            .overflow_x_scroll()
            .child(
                div()
                    .grid()
                    .grid_cols(u16::try_from(columns).unwrap_or(1))
                    .rounded(radius::MD)
                    .border_1()
                    .border_color(theme.border_strong)
                    .overflow_hidden()
                    .children(cells),
            )
            .into_any_element()
    }

    fn code(&mut self, language: &str, code: &str) -> AnyElement {
        let theme = self.theme;
        let index = self.code_blocks;
        self.code_blocks += 1;
        let copied = self.actions.copied_code == Some(index);
        let copy = self.actions.copy_code.clone();
        let source = SharedString::from(code.to_owned());
        let chunk = self.chunk(code, vec![code_run(code, theme)], Vec::new());
        let label = if language.is_empty() {
            SharedString::from("text")
        } else {
            SharedString::from(language.to_owned())
        };
        let (hover, focus, muted) = (theme.hover, theme.border_focus, theme.text_muted);
        let copy_label = if copied { "Copied" } else { "Copy" };
        div()
            .debug_selector(|| "md-code".into())
            .rounded(radius::MD)
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface)
            .overflow_hidden()
            .child(
                div()
                    .min_h(layout::CODE_HEADER)
                    .pl(space::S3)
                    .pr(space::S1_5)
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border_strong)
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(muted)
                    .child(
                        div()
                            .debug_selector(|| "md-code-language".into())
                            .font(font::mono())
                            .child(label),
                    )
                    .child(
                        div()
                            .id(("copy-code", index))
                            .debug_selector(move || format!("copy-code-{index}"))
                            .focusable()
                            .tab_stop(true)
                            .role(gpui::Role::Button)
                            .aria_label(if copied { "Code copied" } else { "Copy code" })
                            .h(control::XS)
                            .px(space::S1_5)
                            .flex()
                            .items_center()
                            .gap(space::S1)
                            .rounded(radius::SM)
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .focus_visible(move |style| style.border_color(focus))
                            .on_click(move |_, window, cx| copy(index, &source, window, cx))
                            .child(ui::icon(
                                if copied { Icon::Check } else { Icon::Copy },
                                icon_size::XS,
                                if copied { theme.success_text } else { muted },
                            ))
                            .child(copy_label),
                    ),
            )
            .child(
                div()
                    .id(("code-scroll", index))
                    .max_h(layout::CODE_MAX_HEIGHT)
                    .overflow_scroll()
                    .p(space::S3)
                    .child(
                        div()
                            .debug_selector(|| "md-code-text".into())
                            .whitespace_nowrap()
                            .text_size(text::CODE.size)
                            .line_height(text::CODE.leading)
                            .child(chunk),
                    ),
            )
            .into_any_element()
    }
}

pub fn message(
    key: i64,
    blocks: &[Rc<Block>],
    color: Hsla,
    selection: &SharedSelection,
    actions: &Actions,
    theme: &Theme,
) -> MessageText {
    let mut builder = Builder {
        key,
        offset: 0,
        code_blocks: 0,
        registry: Registry::default(),
        selection,
        actions,
        theme,
    };
    let tone = Tone {
        color,
        weight: FontWeight::NORMAL,
    };
    let children: Vec<AnyElement> = blocks
        .iter()
        .map(|block| builder.block(block, tone))
        .collect();
    MessageText {
        key,
        child: div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(layout::BLOCK_GAP)
            .children(children)
            .into_any_element(),
        registry: builder.registry,
        selection: selection.clone(),
        open_link: actions.open_link.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_styles_become_runs_of_weight_slant_strike_font_and_link() {
        use crate::markdown::parse::{Block, parse};

        let theme = Theme::dark();
        let source = "plain **bold** *slanted* ~~struck~~ `code` [link](https://example.com)";
        let Some((_, Block::Paragraph(inline))) = parse(source).into_iter().next() else {
            panic!("not a paragraph");
        };
        let tone = Tone {
            color: theme.text.into(),
            weight: FontWeight::NORMAL,
        };
        let (runs, links) = inline_runs(&inline, tone, &theme);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            inline.text.len()
        );
        let run_at = |word: &str| {
            let start = inline.text.find(word).unwrap();
            let mut offset = 0;
            for run in &runs {
                if offset == start {
                    return run.clone();
                }
                offset += run.len;
            }
            panic!("no run starts at {word}");
        };
        let plain = run_at("plain");
        assert_eq!(plain.font.weight, FontWeight::NORMAL);
        assert_eq!(plain.font.family.as_ref(), font::SANS);
        assert_eq!(run_at("bold").font.weight, FontWeight::SEMIBOLD);
        assert_eq!(run_at("slanted").font.style, FontStyle::Italic);
        assert!(run_at("struck").strikethrough.is_some());
        let code = run_at("code");
        assert_eq!(code.font.family.as_ref(), font::MONO);
        assert!(code.background_color.is_some());
        let link = run_at("link");
        assert!(link.underline.is_some());
        assert_eq!(link.color, theme.accent_text.into());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1.as_ref(), "https://example.com");
        assert_eq!(&inline.text[links[0].0.clone()], "link");
    }

    #[test]
    fn a_code_block_is_set_in_the_monospace_font() {
        let run = code_run("fn main() {}", &Theme::light());
        assert_eq!(run.font.family.as_ref(), font::MONO);
        assert_eq!(run.len, 12);
    }

    #[test]
    fn a_double_click_takes_the_word_under_it() {
        let text = "alpha beta, gamma";
        assert_eq!(word_at(text, 0), 0..5);
        assert_eq!(word_at(text, 7), 6..10);
        assert_eq!(word_at(text, 13), 12..17);
        assert_eq!(word_at(text, 17), 17..17);
    }

    #[test]
    fn a_drag_keeps_the_word_or_block_the_selection_started_on() {
        let mut selection = Selection {
            key: 1,
            anchor: 6,
            head: 10,
            origin: (6, 10),
            dragging: true,
        };
        selection.extend(14);
        assert_eq!(selection.range(), 6..14);
        selection.extend(2);
        assert_eq!(selection.range(), 2..10);
        selection.extend(8);
        assert_eq!(selection.range(), 6..10);
    }
}
