use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use crowbot_protocol::{ClientMessage, MessageAuthor, SEARCH_MESSAGE_TERM_LENGTH, SearchResults};
use gpui::{
    AnyElement, Context, Div, FocusHandle, Focusable, FollowMode, FontWeight, HighlightStyle,
    KeyDownEvent, ListOffset, Pixels, Role, SharedString, Stateful, StyledText, Task, Window, div,
    prelude::*,
};

use super::{HEAD, Workspace};
use crate::assets::Icon;
use crate::bots;
use crate::markdown::Selection;
use crate::theme::{Theme, duration, icon as icon_size, layout, radius, size, space, text};
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
        let count = self.hits().len();
        if let Some(palette) = self.palette.as_mut()
            && count > 0
        {
            palette.active = if forward {
                (palette.active + 1) % count
            } else {
                (palette.active + count - 1) % count
            };
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
        let hover = theme.hover;
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
            .h(layout::PALETTE_ROW)
            .px(space::S2)
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded(radius::SM)
            .cursor_pointer()
            .when(active, |row| row.bg(theme.control_active))
            .when(!active, |row| row.hover(move |style| style.bg(hover)))
    }

    fn group_heading(&self, title: &'static str, count: usize, theme: &Theme) -> Div {
        div()
            .flex_none()
            .h(layout::PALETTE_HEADING)
            .px(space::S2)
            .flex()
            .items_center()
            .justify_between()
            .text_size(text::SM.size)
            .line_height(text::SM.leading)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .child(title)
            .child(SharedString::from(count.to_string()))
    }

    fn render_results(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(palette) = self.palette.as_ref() else {
            return Vec::new();
        };
        let Some(results) = palette.results.as_ref() else {
            return Vec::new();
        };
        let now = now_ms();
        let muted = |text: SharedString| {
            div()
                .flex_none()
                .max_w(layout::HEADER_NAME_MAX)
                .truncate()
                .text_size(text::SM.size)
                .line_height(text::SM.leading)
                .text_color(theme.text_muted)
                .child(text)
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
        let hits = self.hits();
        let mut rows = Vec::new();
        let mut index = 0;
        let open = |row: Stateful<Div>, index: usize, cx: &mut Context<Self>| {
            let hit = hits[index];
            row.on_click(cx.listener(move |this, _, window, cx| this.open_hit(hit, window, cx)))
                .into_any_element()
        };

        let bots: Vec<_> = results.bots.iter().filter_map(|id| self.bot(*id)).collect();
        if !bots.is_empty() {
            rows.push(
                self.group_heading("Bots", bots.len(), theme)
                    .into_any_element(),
            );
        }
        for summary in bots {
            let bot = &summary.bot;
            let row = self
                .result_row(index, palette.active == index, bot.name.clone(), theme)
                .child(ui::avatar(
                    bot,
                    size::AVATAR_RESULT,
                    size::AVATAR_RESULT_GLYPH,
                    theme,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_secondary)
                        .child(marked(&bot.name)),
                )
                .children(
                    (!bot.title.is_empty()).then(|| muted(SharedString::from(bot.title.clone()))),
                );
            rows.push(open(row, index, cx));
            index += 1;
        }

        if !results.conversations.is_empty() {
            rows.push(
                self.group_heading("Conversations", results.conversations.len(), theme)
                    .into_any_element(),
            );
        }
        for hit in &results.conversations {
            let title = hit.title.clone();
            let owner = self
                .bot(hit.bot_id)
                .map(|summary| summary.bot.name.clone())
                .unwrap_or_default();
            let row = self
                .result_row(
                    index,
                    palette.active == index,
                    format!("{title}, {owner}"),
                    theme,
                )
                .child(
                    div()
                        .flex_none()
                        .size(size::AVATAR_RESULT)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::icon(
                            Icon::MessageSquare,
                            icon_size::MD,
                            theme.text_muted,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_secondary)
                        .child(marked(&title)),
                )
                .child(muted(owner.into()))
                .child(muted(bots::ago(now, hit.last_activity_ms).into()));
            rows.push(open(row, index, cx));
            index += 1;
        }

        if !results.messages.is_empty() {
            rows.push(
                self.group_heading("Messages", results.messages.len(), theme)
                    .into_any_element(),
            );
        }
        for hit in &results.messages {
            let owner = self.bot(hit.bot_id).map(|summary| &summary.bot);
            let name = owner.map(|bot| bot.name.clone()).unwrap_or_default();
            let author = match hit.author {
                MessageAuthor::User => "You".to_owned(),
                MessageAuthor::Bot => name.clone(),
            };
            let row = self
                .result_row(
                    index,
                    palette.active == index,
                    format!("{author}: {}", hit.snippet),
                    theme,
                )
                .children(owner.map(|bot| {
                    ui::avatar(bot, size::AVATAR_RESULT, size::AVATAR_RESULT_GLYPH, theme)
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(marked(&hit.snippet)),
                )
                .child(muted(
                    match hit.author {
                        MessageAuthor::User => format!("You to {name}"),
                        MessageAuthor::Bot => name,
                    }
                    .into(),
                ))
                .child(muted(bots::ago(now, hit.at_ms).into()));
            rows.push(open(row, index, cx));
            index += 1;
        }
        rows
    }

    pub(super) fn render_palette(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let palette = self.palette.as_ref()?;
        let rows = self.render_results(theme, cx);
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
        let hint = |keys: &'static str, action: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(space::S1)
                .child(
                    div()
                        .px(space::S1)
                        .rounded(radius::SM)
                        .border_1()
                        .border_color(theme.border_strong)
                        .child(keys),
                )
                .child(action)
        };
        let panel = div()
            .id("search-palette")
            .debug_selector(|| "search-palette".into())
            .occlude()
            .role(Role::Dialog)
            .aria_label("Search")
            .w(layout::PALETTE_WIDTH)
            .max_w_full()
            .flex()
            .flex_col()
            .rounded(radius::LG)
            .bg(theme.sheet)
            .border_1()
            .border_color(theme.dialog_ring)
            .shadow(theme.menu_shadow())
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
                    .h(layout::PALETTE_INPUT)
                    .px(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .border_b_1()
                    .border_color(theme.border_strong)
                    .child(ui::icon(Icon::Search, icon_size::MD, theme.text_muted))
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
                    .max_h(layout::PALETTE_RESULTS)
                    .overflow_y_scroll()
                    .p(space::S1_5)
                    .flex()
                    .flex_col()
                    .children(status.map(|(id, message)| {
                        div()
                            .id(id)
                            .debug_selector(move || id.into())
                            .role(Role::Status)
                            .aria_label(message.clone())
                            .px(space::S2)
                            .py(space::S4)
                            .text_center()
                            .text_color(theme.text_muted)
                            .child(message)
                    }))
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .h(layout::PALETTE_ROW)
                    .px(space::S4)
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .border_t_1()
                    .border_color(theme.border_strong)
                    .text_size(text::SM.size)
                    .line_height(text::SM.leading)
                    .text_color(theme.text_muted)
                    .child(hint("↑↓", "to move"))
                    .child(hint("Enter", "to open"))
                    .child(hint("Esc", "to close")),
            );
        Some(
            div()
                .id("search-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .pt(layout::PALETTE_TOP)
                .px(space::S6)
                .pb(space::S6)
                .flex()
                .justify_center()
                .items_start()
                .bg(theme.overlay)
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
}
