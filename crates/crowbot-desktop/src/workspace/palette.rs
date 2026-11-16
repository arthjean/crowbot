use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{ClientMessage, MessageAuthor, SEARCH_MESSAGE_TERM_LENGTH, SearchResults};
use gpui::{
    AnyElement, Context, Div, FocusHandle, Focusable, FollowMode, FontWeight, HighlightStyle,
    KeyDownEvent, ListOffset, MouseMoveEvent, Pixels, Point, Role, ScrollHandle, SharedString,
    Stateful, StyledText, Task, Window, div, prelude::*, relative,
};

use super::{HEAD, Workspace};
use crate::assets::Icon;
use crate::bots;
use crate::markdown::Selection;
use crate::theme::{Theme, duration, icon as icon_size, layout, palette, size, space, text};
use crate::ui;

/// How many frames a revealed message gets to lay out its text before the
/// match in it is given up.
const HIGHLIGHT_FRAMES: usize = 30;

#[derive(Default)]
pub(super) struct Palette {
    request: u64,
    pub results: Option<SearchResults>,
    pub active: usize,
    waiting: bool,
    search: Option<Task<()>>,
    restore_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    /// Where the pointer last was: a result takes the highlight when the
    /// pointer moves onto it, not when the list moves under a still pointer.
    pointer: Option<Point<Pixels>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Bot(i64),
    Conversation {
        bot_id: i64,
        conversation_id: i64,
    },
    Message {
        bot_id: i64,
        conversation_id: i64,
        event_id: i64,
    },
}

/// A message that a search result opens: the conversation scrolls to it and
/// the match in it is selected.
pub(super) struct RevealTarget {
    conversation_id: i64,
    event_id: i64,
    query: String,
    asked: bool,
    key: Option<i64>,
    frames: usize,
}

/// The ranges of `text` that hold a word of the query, whatever the case of
/// its ASCII letters.
pub(super) fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let mut found: Vec<Range<usize>> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .flat_map(|term| {
            lower
                .match_indices(&term)
                .map(|(start, matched)| start..start + matched.len())
                .collect::<Vec<_>>()
        })
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .collect();
    found.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in found {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// The place of a result among the children of the list, which holds a
/// heading before each group.
fn child_of(hits: &[Hit], index: usize) -> usize {
    let mut headings = 0;
    let mut group = None;
    for hit in hits.iter().take(index + 1) {
        let kind = std::mem::discriminant(hit);
        if group != Some(kind) {
            group = Some(kind);
            headings += 1;
        }
    }
    index + headings
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

impl Workspace {
    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        if !self.live() || !self.loaded {
            return;
        }
        self.menu = None;
        self.palette = Some(Palette {
            restore_focus: window.focused(cx),
            pointer: Some(window.mouse_position()),
            ..Palette::default()
        });
        self.palette_query
            .update(cx, |query, cx| query.clear(window, cx));
        window.focus(&self.palette_query.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.take() else {
            return;
        };
        match palette.restore_focus {
            Some(focus) => window.focus(&focus, cx),
            None => self.focus_composer(window, cx),
        }
        cx.notify();
    }

    /// The query changed: the daemon is asked once the typing pauses.
    pub(super) fn palette_typed(&mut self, cx: &mut Context<Self>) {
        let query = self.palette_query.read(cx).text().trim().to_owned();
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if palette
            .results
            .as_ref()
            .is_some_and(|results| results.query == query)
            && !palette.waiting
        {
            return;
        }
        palette.request += 1;
        palette.active = 0;
        let request = palette.request;
        if query.is_empty() {
            palette.results = None;
            palette.waiting = false;
            palette.search = None;
        } else {
            palette.waiting = true;
            palette.search = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(duration::SEARCH_DEBOUNCE)
                    .await;
                this.update(cx, |this, _| {
                    if this.live() {
                        this.connection
                            .send(ClientMessage::Search { request, query });
                    }
                })
                .ok();
            }));
        }
        cx.notify();
    }

    pub(super) fn palette_results(&mut self, results: SearchResults) {
        if let Some(palette) = self.palette.as_mut()
            && palette.request == results.request
        {
            palette.results = Some(results);
            palette.waiting = false;
            palette.active = 0;
        }
    }

    pub(super) fn hits(&self) -> Vec<Hit> {
        let Some(results) = self
            .palette
            .as_ref()
            .and_then(|palette| palette.results.as_ref())
        else {
            return Vec::new();
        };
        let bots = results
            .bots
            .iter()
            .filter(|id| self.bot(**id).is_some())
            .map(|id| Hit::Bot(*id));
        let conversations = results.conversations.iter().map(|hit| Hit::Conversation {
            bot_id: hit.bot_id,
            conversation_id: hit.conversation_id,
        });
        let messages = results.messages.iter().map(|hit| Hit::Message {
            bot_id: hit.bot_id,
            conversation_id: hit.conversation_id,
            event_id: hit.event_id,
        });
        bots.chain(conversations).chain(messages).collect()
    }

    pub(super) fn palette_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let hits = self.hits();
        let count = hits.len();
        if let Some(palette) = self.palette.as_mut()
            && count > 0
        {
            palette.active = if forward {
                (palette.active + 1) % count
            } else {
                (palette.active + count - 1) % count
            };
            // The list keeps its padding at both ends: the first result
            // brings its heading back, the last one the end of the list.
            if palette.active == 0 {
                palette.scroll.set_offset(Point::default());
            } else if palette.active + 1 == count {
                palette.scroll.scroll_to_bottom();
            } else {
                palette
                    .scroll
                    .scroll_to_item(child_of(&hits, palette.active));
            }
            cx.notify();
        }
    }

    /// The pointer moved over a result: it takes the highlight of the keys.
    fn palette_point(&mut self, index: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if palette.pointer.replace(position) != Some(position) && palette.active != index {
            palette.active = index;
            cx.notify();
        }
    }

    pub(super) fn open_active_hit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.palette.as_ref().map_or(0, |palette| palette.active);
        if let Some(hit) = self.hits().get(active).copied() {
            self.open_hit(hit, window, cx);
        }
    }

    fn open_hit(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.palette_query.read(cx).text().trim().to_owned();
        self.palette = None;
        // A result is a bot or a conversation: the settings page gives way.
        self.settings = None;
        self.executable_edit = None;
        let (bot_id, conversation_id) = match hit {
            Hit::Bot(bot_id) => {
                self.creation = None;
                self.open_bot(bot_id, window, cx);
                return;
            }
            Hit::Conversation {
                bot_id,
                conversation_id,
            }
            | Hit::Message {
                bot_id,
                conversation_id,
                ..
            } => (bot_id, conversation_id),
        };
        if self.bot(bot_id).is_none() {
            return;
        }
        self.creation = None;
        self.history = None;
        self.selected_bot = Some(bot_id);
        if let Hit::Message { event_id, .. } = hit {
            self.reveal = Some(RevealTarget {
                conversation_id,
                event_id,
                query,
                asked: false,
                key: None,
                frames: 0,
            });
            // The events that arrived while the conversation was open carry
            // no id: a new snapshot gives every stored event its own.
            if self.selected == Some(conversation_id) {
                self.open = None;
                self.connection
                    .send(ClientMessage::OpenConversation { conversation_id });
            }
        }
        self.select(conversation_id);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Moves the open conversation to the message a search result named, and
    /// asks the daemon for it when it is older than what is loaded.
    pub(super) fn continue_reveal(&mut self) {
        let now = (self.clock)();
        let (Some(target), Some(open)) = (self.reveal.as_mut(), self.open.as_mut()) else {
            return;
        };
        if target.conversation_id != open.id || target.key.is_some() {
            return;
        }
        if open.transcript.holds_event(target.event_id) {
            let Some(index) = open.transcript.row_of_event(target.event_id) else {
                self.reveal = None;
                return;
            };
            open.list.set_follow_mode(FollowMode::Normal);
            open.list.scroll_to(ListOffset {
                item_ix: HEAD + index,
                offset_in_item: Pixels::ZERO,
            });
            let key = open.transcript.rows()[index].key;
            target.key = Some(key);
            open.flash = Some((key, now));
            if open.at_end.replace(false) {
                self.connection.send(ClientMessage::MarkRead {
                    conversation_id: open.id,
                    following: false,
                });
            }
        } else if let Some(before) = open.transcript.older().filter(|_| !target.asked) {
            target.asked = true;
            open.loading_older = true;
            self.connection.send(ClientMessage::RevealEvent {
                conversation_id: open.id,
                before,
                event_id: target.event_id,
            });
        } else {
            self.reveal = None;
        }
    }

    /// Selects the match in the revealed message, once its text is laid out.
    pub(super) fn highlight_revealed(&mut self, window: &mut Window) {
        let Some(target) = self.reveal.as_mut() else {
            return;
        };
        let Some(key) = target.key else {
            return;
        };
        let shown = self
            .open
            .as_ref()
            .filter(|open| open.id == target.conversation_id)
            .and_then(|open| open.texts.get(&key))
            .map(|shown| shown.document.text());
        match shown {
            Some(shown) => {
                if let Some(range) = matches(&shown, &target.query).into_iter().next() {
                    *self.selection.borrow_mut() = Some(Selection::spanning(key, range));
                }
                self.reveal = None;
            }
            None if target.frames < HIGHLIGHT_FRAMES => {
                target.frames += 1;
                window.request_animation_frame();
            }
            None => self.reveal = None,
        }
    }

    fn result_row(
        &self,
        index: usize,
        active: bool,
        label: String,
        theme: &Theme,
    ) -> Stateful<Div> {
        div()
            .id(("search-result", index))
            .debug_selector(move || format!("search-result-{index}"))
            .role(Role::Button)
            .aria_label(SharedString::from(if active {
                format!("{label}, selected")
            } else {
                label
            }))
            .flex_none()
            .px(space::S2)
            .py(palette::ROW_PADDING_Y)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(palette::ROW_RADIUS)
            .cursor_pointer()
            .when(active, |row| row.bg(theme.menu_highlight))
    }

    fn group_heading(&self, title: &'static str, first: bool, theme: &Theme) -> Div {
        div()
            .flex_none()
            .when(!first, |heading| heading.mt(space::S1_5))
            .px(space::S2)
            .py(palette::ROW_PADDING_Y)
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .child(title)
    }

    fn render_results(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(palette) = self.palette.as_ref() else {
            return Vec::new();
        };
        let Some(results) = palette.results.as_ref() else {
            return Vec::new();
        };
        let now = now_ms();
        let small = || {
            div()
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_muted)
        };
        // The avatar or the icon of a result, beside its first line.
        let lead = |mark: AnyElement| {
            div()
                .flex_none()
                .self_start()
                .size(palette::LEAD)
                .flex()
                .items_center()
                .justify_center()
                .child(mark)
        };
        let marked = |shown: &str| {
            let style = HighlightStyle {
                color: Some(theme.text.into()),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..HighlightStyle::default()
            };
            StyledText::new(SharedString::from(shown.to_owned())).with_highlights(
                matches(shown, &results.query)
                    .into_iter()
                    .map(move |range| (range, style)),
            )
        };
        // The title of a result, then what tells it from its neighbors.
        let headline = |title: StyledText, context: String| {
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .text_size(text::LG.size)
                .line_height(text::LG.leading)
                .child(div().min_w_0().truncate().child(title))
                .children((!context.is_empty()).then(|| {
                    small()
                        .flex_none()
                        .max_w(relative(palette::CONTEXT_SHARE))
                        .truncate()
                        .child(SharedString::from(context))
                }))
        };
        let time = |at_ms: i64| {
            small()
                .flex_none()
                .min_w(palette::TIME_MIN)
                .text_right()
                .child(SharedString::from(bots::ago(now, at_ms)))
        };
        let hits = self.hits();
        let mut rows = Vec::new();
        let mut index = 0;
        let open = |row: Stateful<Div>, index: usize, cx: &mut Context<Self>| {
            let hit = hits[index];
            row.on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.palette_point(index, event.position, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.open_hit(hit, window, cx)))
            .into_any_element()
        };

        let bots: Vec<_> = results.bots.iter().filter_map(|id| self.bot(*id)).collect();
        if !bots.is_empty() {
            let heading = self.group_heading("Bots", rows.is_empty(), theme);
            rows.push(heading.into_any_element());
        }
        for summary in bots {
            let bot = &summary.bot;
            let avatar = ui::avatar(bot, palette::LEAD, theme);
            let row = self
                .result_row(index, palette.active == index, bot.name.clone(), theme)
                .child(lead(avatar.into_any_element()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(headline(marked(&bot.name), bot.title.clone())),
                );
            rows.push(open(row, index, cx));
            index += 1;
        }

        if !results.conversations.is_empty() {
            let heading = self.group_heading("Conversations", rows.is_empty(), theme);
            rows.push(heading.into_any_element());
        }
        for hit in &results.conversations {
            let owner = self
                .bot(hit.bot_id)
                .map(|summary| summary.bot.name.clone())
                .unwrap_or_default();
            let icon = ui::icon(Icon::MessageSquare, icon_size::MD, theme.text_muted);
            let row = self
                .result_row(
                    index,
                    palette.active == index,
                    format!("{}, {owner}", hit.title),
                    theme,
                )
                .child(lead(icon.into_any_element()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(headline(marked(&hit.title), owner)),
                )
                .child(time(hit.last_activity_ms));
            rows.push(open(row, index, cx));
            index += 1;
        }

        if !results.messages.is_empty() {
            let heading = self.group_heading("Messages", rows.is_empty(), theme);
            rows.push(heading.into_any_element());
        }
        for hit in &results.messages {
            let owner = self.bot(hit.bot_id).map(|summary| &summary.bot);
            let name = owner.map(|bot| bot.name.clone()).unwrap_or_default();
            let (author, context) = match hit.author {
                MessageAuthor::User if name.is_empty() => ("You".to_owned(), String::new()),
                MessageAuthor::User => ("You".to_owned(), format!("to {name}")),
                MessageAuthor::Bot => (name, String::new()),
            };
            let mark = match owner {
                Some(bot) => ui::avatar(bot, palette::LEAD, theme).into_any_element(),
                None => ui::icon(Icon::MessageSquare, icon_size::MD, theme.text_muted)
                    .into_any_element(),
            };
            let row = self
                .result_row(
                    index,
                    palette.active == index,
                    format!("{author}: {}", hit.snippet),
                    theme,
                )
                .child(lead(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(headline(
                            StyledText::new(SharedString::from(author)),
                            context,
                        ))
                        .child(small().truncate().child(marked(&hit.snippet))),
                )
                .child(time(hit.at_ms));
            rows.push(open(row, index, cx));
            index += 1;
        }
        rows
    }

    pub(super) fn render_palette(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let rows = self.render_results(theme, cx);
        let has_results = !rows.is_empty();
        let query = self.palette_query.read(cx).text().trim().to_owned();
        let status: Option<(&'static str, SharedString)> = match &palette.results {
            _ if query.is_empty() => Some((
                "search-hint",
                "Search bots, conversations by title, and messages.".into(),
            )),
            Some(results) if rows.is_empty() && results.query == query => Some((
                "search-empty",
                if results.messages_searched {
                    format!("Nothing matches “{query}”.").into()
                } else {
                    format!(
                        "Nothing matches “{query}”. Messages are searched from {SEARCH_MESSAGE_TERM_LENGTH} characters."
                    )
                    .into()
                },
            )),
            None => Some(("search-waiting", "Searching…".into())),
            Some(_) => None,
        };
        let key = |cap: AnyElement| {
            div()
                .flex_none()
                .h(palette::KEY)
                .min_w(palette::KEY)
                .px(space::S1)
                .flex()
                .items_center()
                .justify_center()
                .rounded(palette::KEY_RADIUS)
                .bg(theme.badge)
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_secondary)
                .child(cap)
        };
        let arrow = |glyph: Icon| {
            key(ui::icon(glyph, icon_size::XS, theme.text_secondary).into_any_element())
        };
        let word = |cap: &'static str| key(cap.into_any_element());
        let hint = |keys: Vec<Div>, action: &'static str| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(space::S1)
                .children(keys)
                .child(action)
        };
        let panel = div()
            .id("search-palette")
            .debug_selector(|| "search-palette".into())
            .occlude()
            .role(Role::Dialog)
            .aria_label("Search")
            .w(palette::WIDTH)
            .max_w_full()
            .max_h(palette::MAX_HEIGHT)
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(palette::RADIUS)
            .bg(theme.sheet)
            .border_1()
            .border_color(theme.dialog_ring)
            // One edge only: the ring of the menu shadow would double it.
            .shadow(vec![theme.drop_shadow()])
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_palette(window, cx);
                }
            }))
            .child(
                div()
                    .flex_none()
                    .h(palette::INPUT)
                    .px(palette::PADDING + space::S2)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .flex_none()
                            .size(palette::LEAD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(ui::icon(Icon::Search, icon_size::MD, theme.text_muted)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "search-query".into())
                            .flex_1()
                            .min_w_0()
                            .child(self.palette_query.clone()),
                    ),
            )
            .child(
                div()
                    .id("search-results")
                    .debug_selector(|| "search-results".into())
                    .role(Role::List)
                    .aria_label("Search results")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&palette.scroll)
                    .p(palette::PADDING)
                    .flex()
                    .flex_col()
                    .children(status.map(|(id, message)| {
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .role(Role::Status)
                            .aria_label(message.clone())
                            .flex_none()
                            .px(space::S2)
                            .py(palette::STATUS_PADDING_Y)
                            .text_center()
                            .text_size(text::LG.size)
                            .line_height(text::LG.leading)
                            .text_color(theme.text_muted)
                            .child(message)
                    }))
                    .children(rows),
            )
            .child(
                div()
                    .debug_selector(|| "search-keys".into())
                    .flex_none()
                    .px(palette::FOOTER_PADDING_X)
                    .py(palette::FOOTER_PADDING_Y)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(space::S3)
                    .gap_y(space::S1_5)
                    // The fill reaches the corners of the panel, inside its
                    // 1 px edge.
                    .rounded_b(palette::RADIUS - size::HAIRLINE)
                    .border_t_1()
                    .border_color(theme.menu_separator)
                    .bg(theme.palette_footer)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(hint(
                        vec![arrow(Icon::ArrowUp), arrow(Icon::ArrowDown)],
                        "Navigate",
                    ))
                    .children(has_results.then(|| hint(vec![word("Enter")], "Open")))
                    .child(hint(vec![word("Esc")], "Close")),
            );
        // A tenth of the window stays free over the palette, as under it,
        // and it never covers the title band.
        let room = window.viewport_size().height * palette::WINDOW_SHARE;
        Some(
            div()
                .id("search-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .pt(room.max(layout::TOP_ROW + space::S2))
                .px(space::S4)
                .pb(room.max(space::S4))
                .flex()
                .flex_col()
                .items_center()
                .bg(theme.palette_scrim)
                .on_click(cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_a_query_are_found_in_any_letter_case_and_merged() {
        assert_eq!(matches("The Harbor opens", "harbor"), vec![4..10]);
        assert_eq!(matches("abcabc", "ABC"), vec![0..6]);
        assert_eq!(
            matches("deploy the server", "server deploy"),
            [0..6, 11..17]
        );
        assert_eq!(matches("overlap", "over verl"), vec![0..5]);
        assert_eq!(matches("résumé du résumé", "du"), vec![9..11]);
        assert!(matches("nothing here", "zeppelin").is_empty());
        assert!(matches("anything", "   ").is_empty());
    }

    #[test]
    fn a_result_is_counted_among_the_rows_after_the_headings_of_its_groups() {
        let conversation = |conversation_id| Hit::Conversation {
            bot_id: 1,
            conversation_id,
        };
        let message = |event_id| Hit::Message {
            bot_id: 1,
            conversation_id: 10,
            event_id,
        };
        let hits = [
            Hit::Bot(1),
            Hit::Bot(2),
            conversation(10),
            message(7),
            message(8),
        ];
        assert_eq!(child_of(&hits, 0), 1);
        assert_eq!(child_of(&hits, 1), 2);
        assert_eq!(child_of(&hits, 2), 4);
        assert_eq!(child_of(&hits, 3), 6);
        assert_eq!(child_of(&hits, 4), 7);
        // A list with no bot starts with the heading of its conversations.
        assert_eq!(child_of(&hits[2..], 1), 3);
    }
}
