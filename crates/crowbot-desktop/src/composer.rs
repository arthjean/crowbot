use std::ops::Range;

use gpui::accesskit::ActionData;
use gpui::{
    AccessibleAction, App, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Role, ScrollHandle, SharedString,
    Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div,
    fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{Theme, layout, size as sizes, text};

const KEY_CONTEXT: &str = "Composer";

#[derive(Clone, Copy)]
pub struct InputStyle {
    pub step: text::Step,
    pub single_line: bool,
    pub label: &'static str,
    pub min_rows: f32,
    pub max_rows: f32,
    /// The placeholder reads as a sample value, in `text_placeholder`, not as
    /// a muted hint.
    pub sample_placeholder: bool,
}

impl InputStyle {
    pub fn message() -> Self {
        Self {
            step: text::LG,
            single_line: false,
            label: "Message",
            min_rows: 1.,
            max_rows: layout::COMPOSER_MAX_ROWS,
            sample_placeholder: false,
        }
    }

    pub fn field(label: &'static str) -> Self {
        Self {
            step: text::LG,
            single_line: true,
            label,
            min_rows: 1.,
            max_rows: 1.,
            sample_placeholder: false,
        }
    }

    pub fn area(label: &'static str, min_rows: f32, max_rows: f32) -> Self {
        Self {
            step: text::LG,
            single_line: false,
            label,
            min_rows,
            max_rows,
            sample_placeholder: false,
        }
    }

    /// A field of a form page, such as the name of a new bot.
    pub fn form_field(label: &'static str) -> Self {
        Self {
            step: text::FIELD,
            sample_placeholder: true,
            ..Self::field(label)
        }
    }

    /// An area of a form page that grows with its text.
    pub fn form_area(label: &'static str, min_rows: f32, max_rows: f32) -> Self {
        Self {
            step: text::AREA,
            sample_placeholder: true,
            ..Self::area(label, min_rows, max_rows)
        }
    }
}

actions!(
    composer,
    [
        Submit,
        SubmitWithModifier,
        InsertLineBreak,
        Backspace,
        Delete,
        DeleteWordLeft,
        Left,
        Right,
        Up,
        Down,
        WordLeft,
        WordRight,
        Home,
        End,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
    ]
);

#[cfg(target_os = "macos")]
macro_rules! secondary {
    ($key:literal) => {
        concat!("cmd-", $key)
    };
}

#[cfg(not(target_os = "macos"))]
macro_rules! secondary {
    ($key:literal) => {
        concat!("ctrl-", $key)
    };
}

pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new(secondary!("enter"), SubmitWithModifier, context),
        KeyBinding::new("shift-enter", InsertLineBreak, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new(secondary!("backspace"), DeleteWordLeft, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new(secondary!("left"), WordLeft, context),
        KeyBinding::new(secondary!("right"), WordRight, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new(secondary!("shift-left"), SelectWordLeft, context),
        KeyBinding::new(secondary!("shift-right"), SelectWordRight, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new(secondary!("a"), SelectAll, context),
        KeyBinding::new(secondary!("c"), Copy, context),
        KeyBinding::new(secondary!("x"), Cut, context),
        KeyBinding::new(secondary!("v"), Paste, context),
    ]);
}

pub enum ComposerEvent {
    Submit,
    /// The up or the down key in a field of one line: the owner moves through
    /// what it lists under the field.
    Step(bool),
}

pub struct Composer {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<TextLayout>,
    is_selecting: bool,
    scroll: ScrollHandle,
    reveal_cursor: bool,
    style: InputStyle,
    enabled: bool,
    enter_submits: bool,
    max_length: Option<usize>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct Row {
    start: usize,
    end: usize,
    line: usize,
    line_start: usize,
}

struct TextLayout {
    lines: Vec<WrappedLine>,
    rows: Vec<Row>,
    bounds: Bounds<Pixels>,
    content_len: usize,
    line_height: Pixels,
}

impl TextLayout {
    fn new(
        lines: Vec<WrappedLine>,
        bounds: Bounds<Pixels>,
        content_len: usize,
        line_height: Pixels,
    ) -> Self {
        let mut rows = Vec::new();
        let mut line_start = 0;
        for (index, line) in lines.iter().enumerate() {
            let mut start = 0;
            for boundary in &line.wrap_boundaries {
                let end =
                    line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                rows.push(Row {
                    start: line_start + start,
                    end: line_start + end,
                    line: index,
                    line_start,
                });
                start = end;
            }
            rows.push(Row {
                start: line_start + start,
                end: line_start + line.len(),
                line: index,
                line_start,
            });
            line_start += line.len() + 1;
        }
        Self {
            lines,
            rows,
            bounds,
            content_len,
            line_height,
        }
    }

    fn row_for_offset(&self, offset: usize) -> usize {
        self.rows
            .iter()
            .rposition(|row| row.start <= offset)
            .unwrap_or(0)
    }

    fn x_in_row(&self, row: &Row, offset: usize) -> Pixels {
        let layout = &self.lines[row.line].unwrapped_layout;
        let offset = offset.clamp(row.start, row.end);
        layout.x_for_index(offset - row.line_start) - layout.x_for_index(row.start - row.line_start)
    }

    fn position(&self, offset: usize) -> Point<Pixels> {
        let index = self.row_for_offset(offset);
        match self.rows.get(index) {
            Some(row) => point(self.x_in_row(row, offset), self.line_height * index as f32),
            None => Point::default(),
        }
    }

    fn offset_in_row(&self, index: usize, x: Pixels) -> usize {
        let Some(row) = self.rows.get(index) else {
            return 0;
        };
        let layout = &self.lines[row.line].unwrapped_layout;
        let origin = layout.x_for_index(row.start - row.line_start);
        let local = layout.closest_index_for_x(x.max(px(0.)) + origin);
        (local + row.line_start).clamp(row.start, row.end)
    }

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let local = position - self.bounds.origin;
        if local.y < px(0.) {
            return 0;
        }
        let index = (local.y / self.line_height).floor() as usize;
        if index >= self.rows.len() {
            return self.content_len;
        }
        self.offset_in_row(index, local.x)
    }

    fn selection_quads(&self, range: &Range<usize>, color: gpui::Rgba) -> Vec<PaintQuad> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| range.start <= row.end && range.end >= row.start)
            .map(|(index, row)| {
                let left = self.x_in_row(row, range.start.max(row.start));
                let mut right = self.x_in_row(row, range.end.min(row.end));
                if range.end > row.end {
                    right += sizes::SELECTION_OVERHANG;
                }
                let top = self.bounds.top() + self.line_height * index as f32;
                fill(
                    Bounds::from_corners(
                        point(self.bounds.left() + left, top),
                        point(self.bounds.left() + right, top + self.line_height),
                    ),
                    color,
                )
            })
            .collect()
    }
}

impl Composer {
    pub fn new(
        placeholder: impl Into<SharedString>,
        style: InputStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: None,
            is_selecting: false,
            scroll: ScrollHandle::new(),
            reveal_cursor: false,
            style,
            enabled: true,
            enter_submits: true,
            max_length: None,
        }
    }

    /// The field stops taking text once it holds this many characters.
    pub fn with_max_length(mut self, characters: usize) -> Self {
        self.max_length = Some(characters);
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_full(&self) -> bool {
        self.max_length
            .is_some_and(|limit| self.content.chars().count() >= limit)
    }

    /// The part of `text` that still fits once `replaced` is taken out.
    fn fitted(&self, text: &str, replaced: &Range<usize>) -> String {
        let text = self.accepted(text);
        let Some(limit) = self.max_length else {
            return text;
        };
        let kept = self.content.chars().count() - self.content[replaced.clone()].chars().count();
        text.chars().take(limit.saturating_sub(kept)).collect()
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            cx.notify();
        }
    }

    pub fn set_enter_submits(&mut self, enter_submits: bool) {
        self.enter_submits = enter_submits;
    }

    fn accepted(&self, text: &str) -> String {
        let text = text.replace("\r\n", "\n");
        if self.style.single_line {
            text.replace('\n', " ")
        } else {
            text
        }
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.layout = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.content = self.fitted(text, &(0..self.content.len()));
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.reveal_cursor = true;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// Replaces the text and selects all of it, so that typing replaces a
    /// value the user did not choose, such as a default name.
    pub fn set_text_selected(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text(text, window, cx);
        self.selected_range = 0..self.content.len();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.selection_changed(window, cx);
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.selection_changed(window, cx);
    }

    fn move_or_select(
        &mut self,
        offset: usize,
        select: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if select {
            self.select_to(offset, window, cx);
        } else {
            self.move_to(offset, window, cx);
        }
    }

    fn selection_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.reveal_cursor = true;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.content[..offset]
            .unicode_word_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content[offset..]
            .unicode_word_indices()
            .next()
            .map_or(self.content.len(), |(index, word)| {
                offset + index + word.len()
            })
    }

    fn vertical_target(&self, rows: isize) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.layout.as_ref() else {
            return cursor;
        };
        let current = layout.row_for_offset(cursor);
        let Some(target) = current.checked_add_signed(rows) else {
            return 0;
        };
        if target >= layout.rows.len() {
            return self.content.len();
        }
        layout.offset_in_row(target, layout.position(cursor).x)
    }

    fn row_edge(&self, end: bool) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.layout.as_ref() else {
            return if end { self.content.len() } else { 0 };
        };
        match layout.rows.get(layout.row_for_offset(cursor)) {
            Some(row) if end => row.end,
            Some(row) => row.start,
            None => cursor,
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.enter_submits || self.style.single_line {
            cx.emit(ComposerEvent::Submit);
        } else {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn submit_with_modifier(
        &mut self,
        _: &SubmitWithModifier,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ComposerEvent::Submit);
    }

    fn insert_line_break(
        &mut self,
        _: &InsertLineBreak,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.style.single_line {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word(self.cursor_offset()), window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(target, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(target, window, cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        if self.style.single_line {
            cx.emit(ComposerEvent::Step(false));
        } else {
            self.move_to(self.vertical_target(-1), window, cx);
        }
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        if self.style.single_line {
            cx.emit(ComposerEvent::Step(true));
        } else {
            self.move_to(self.vertical_target(1), window, cx);
        }
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_edge(false), window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_edge(true), window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.vertical_target(-1), window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.vertical_target(1), window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_word(self.cursor_offset()), window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_word(self.cursor_offset()), window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.row_edge(false), window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.row_edge(true), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        } else {
            cx.propagate();
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        let offset = self.offset_for_mouse(event.position);
        self.move_or_select(offset, event.modifiers.shift, window, cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.offset_for_mouse(event.position), window, cx);
        }
    }

    fn offset_for_mouse(&self, position: Point<Pixels>) -> usize {
        match self.layout.as_ref() {
            Some(layout) if !self.content.is_empty() => layout.offset_at(position),
            _ => 0,
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for character in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_offset += character.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone())
    }

    fn current_layout(&self) -> Option<&TextLayout> {
        self.layout
            .as_ref()
            .filter(|layout| layout.content_len == self.content.len())
    }
}

impl EntityInputHandler for Composer {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        self.selection_changed(window, cx);
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let range = self.replacement_range(range_utf16);
        let new_text = &self.fitted(new_text, &range);
        self.content.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.reveal_cursor = true;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let range = self.replacement_range(range_utf16);
        let new_text = &self.fitted(new_text, &range);
        self.content.replace_range(range.clone(), new_text);
        let inserted_end = range.start + new_text.len();
        self.marked_range = (!new_text.is_empty()).then_some(range.start..inserted_end);
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or(inserted_end..inserted_end);
        self.selection_reversed = false;
        self.reveal_cursor = true;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let layout = self.current_layout()?;
        let start = layout.bounds.origin + layout.position(range.start);
        let end = layout.bounds.origin + layout.position(range.end);
        Some(Bounds::from_corners(
            start,
            point(end.x.max(start.x), start.y + layout.line_height),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.current_layout()?;
        Some(self.offset_to_utf16(layout.offset_at(point)))
    }
}

struct ComposerText {
    composer: Entity<Composer>,
    theme: Theme,
}

struct Prepainted {
    layout: Option<TextLayout>,
    keep_layout: bool,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

fn shape(
    text: SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    width: Option<Pixels>,
    window: &Window,
) -> Vec<WrappedLine> {
    window
        .text_system()
        .shape_text(text, font_size, runs, width, None)
        .map(|lines| lines.into_iter().collect())
        .unwrap_or_default()
}

fn row_count(lines: &[WrappedLine]) -> usize {
    lines
        .iter()
        .map(|line| line.wrap_boundaries.len() + 1)
        .sum::<usize>()
        .max(1)
}

impl ComposerText {
    fn display(&self, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
        let composer = self.composer.read(cx);
        let (text, color): (SharedString, gpui::Hsla) = if composer.content.is_empty() {
            let color = if composer.style.sample_placeholder {
                self.theme.text_placeholder
            } else {
                self.theme.text_muted
            };
            (composer.placeholder.clone(), color.into())
        } else {
            (composer.content.clone().into(), self.theme.text.into())
        };
        let run = TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match composer
            .marked_range
            .as_ref()
            .filter(|_| !composer.content.is_empty())
        {
            Some(marked) => vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: sizes::HAIRLINE,
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };
        (text, runs)
    }
}

impl Element for ComposerText {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

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
        let (text, runs) = self.display(window, cx);
        let step = self.composer.read(cx).style.step;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let lines = shape(text.clone(), step.size, &runs, width, window);
            size(
                width.unwrap_or_default(),
                step.leading * row_count(&lines) as f32,
            )
        });
        (layout, ())
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
        let (text, runs) = self.display(window, cx);
        let step = self.composer.read(cx).style.step;
        let lines = shape(text, step.size, &runs, Some(bounds.size.width), window);
        let composer = self.composer.read(cx);
        let empty = composer.content.is_empty();
        let layout = TextLayout::new(lines, bounds, composer.content.len(), step.leading);
        let cursor_position = if empty {
            bounds.origin
        } else {
            bounds.origin + layout.position(composer.cursor_offset())
        };
        let (cursor, selection) = if empty || composer.selected_range.is_empty() {
            let cursor = fill(
                Bounds::new(cursor_position, size(sizes::CURSOR, step.leading)),
                self.theme.text,
            );
            (Some(cursor), Vec::new())
        } else {
            (
                None,
                layout.selection_quads(&composer.selected_range, self.theme.accent_strong),
            )
        };
        let reveal = composer.reveal_cursor;
        let viewport = composer.scroll.bounds();
        let scroll = composer.scroll.clone();
        if reveal {
            self.composer
                .update(cx, |composer, _| composer.reveal_cursor = false);
        }
        if reveal && viewport.size.height > px(0.) {
            let above = viewport.top() - cursor_position.y;
            let below = cursor_position.y + step.leading - viewport.bottom();
            let shift = if above > px(0.) {
                above
            } else if below > px(0.) {
                -below
            } else {
                px(0.)
            };
            if shift != px(0.) {
                let offset = scroll.offset();
                scroll.set_offset(point(offset.x, offset.y + shift));
                window.request_animation_frame();
            }
        }
        Prepainted {
            layout: Some(layout),
            keep_layout: !empty,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.composer.read(cx).focus_handle.clone();
        if self.composer.read(cx).enabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.composer.clone()),
                cx,
            );
        }
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let layout = prepaint.layout.take();
        if let Some(layout) = layout.as_ref() {
            let mut origin = bounds.origin;
            for line in &layout.lines {
                line.paint(
                    origin,
                    layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
                origin.y += layout.line_height * (line.wrap_boundaries.len() + 1) as f32;
            }
        }
        let layout = layout.filter(|_| prepaint.keep_layout);
        self.composer
            .update(cx, |composer, _| composer.layout = layout);
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let step = self.style.step;
        div()
            .id(self.style.label)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .tab_stop(true)
            .role(if self.style.single_line {
                Role::TextInput
            } else {
                Role::MultilineTextInput
            })
            .aria_label(self.style.label)
            .aria_value(self.content.clone())
            .aria_placeholder(self.placeholder.clone())
            .on_a11y_action(AccessibleAction::Focus, {
                let focus_handle = self.focus_handle.clone();
                move |_, window, cx| window.focus(&focus_handle, cx)
            })
            .on_a11y_action(AccessibleAction::SetValue, {
                let composer = cx.entity().downgrade();
                move |data, window, cx| {
                    let Some(ActionData::Value(value)) = data else {
                        return;
                    };
                    composer
                        .update(cx, |composer, cx| composer.set_text(value, window, cx))
                        .ok();
                }
            })
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::submit_with_modifier))
            .on_action(cx.listener(Self::insert_line_break))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .text_size(step.size)
            .line_height(step.leading)
            .child(
                div()
                    .id("composer-scroll")
                    .w_full()
                    .min_h(step.leading * self.style.min_rows)
                    .max_h(step.leading * self.style.max_rows)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(ComposerText {
                        composer: cx.entity(),
                        theme,
                    }),
            )
    }
}
