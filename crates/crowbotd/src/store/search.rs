use crowbot_protocol::{
    ConversationHit, Event, MessageAuthor, MessageHit, SEARCH_MESSAGE_TERM_LENGTH,
    SEARCH_QUERY_LENGTH, SearchResults,
};
use rusqlite::params;

use super::{Store, StoreError};

const BOT_HITS: i64 = 5;
const CONVERSATION_HITS: i64 = 5;
const MESSAGE_HITS: i64 = 20;
const SNIPPET_BEFORE: usize = 32;
const SNIPPET_LENGTH: usize = 160;

fn like_pattern(query: &str) -> String {
    let mut pattern = String::from("%");
    for character in query.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

fn message_terms(query: &str) -> Vec<&str> {
    query
        .split_whitespace()
        .filter(|term| term.chars().count() >= SEARCH_MESSAGE_TERM_LENGTH)
        .collect()
}

fn match_expression(terms: &[&str]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn char_start(text: &str, characters: usize) -> usize {
    text.char_indices()
        .nth(characters)
        .map_or(text.len(), |(index, _)| index)
}

fn snippet(text: &str, terms: &[&str]) -> String {
    let lower = text.to_lowercase();
    let found = terms
        .iter()
        .filter_map(|term| lower.find(&term.to_lowercase()))
        .min()
        .filter(|at| text.is_char_boundary(*at))
        .unwrap_or(0);
    let before = text[..found].chars().count();
    let start = char_start(text, before.saturating_sub(SNIPPET_BEFORE));
    let window = &text[start..];
    let end = char_start(window, SNIPPET_LENGTH);
    let mut shown = String::new();
    if start > 0 {
        shown.push('…');
    }
    shown.push_str(
        &window[..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
    if end < window.len() {
        shown.push('…');
    }
    shown
}

impl Store {
    pub fn search(&self, request: u64, query: &str) -> Result<SearchResults, StoreError> {
        let query: String = query.trim().chars().take(SEARCH_QUERY_LENGTH).collect();
        let query = query.trim_end();
        let mut results = SearchResults {
            request,
            query: query.to_owned(),
            ..SearchResults::default()
        };
        if query.is_empty() {
            return Ok(results);
        }
        let pattern = like_pattern(query);
        results.bots = self.search_bots(&pattern)?;
        results.conversations = self.search_conversations(&pattern)?;
        let terms = message_terms(query);
        if !terms.is_empty() {
            results.messages = self.search_messages(&terms)?;
            results.messages_searched = true;
        }
        Ok(results)
    }

    fn search_bots(&self, pattern: &str) -> Result<Vec<i64>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM bots
             WHERE name LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
             ORDER BY name COLLATE NOCASE, id LIMIT ?2",
        )?;
        let bots = statement
            .query_map(params![pattern, BOT_HITS], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(bots)
    }

    fn search_conversations(&self, pattern: &str) -> Result<Vec<ConversationHit>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.bot_id, c.title, COALESCE(
                    (SELECT MAX(created_at_ms) FROM events WHERE conversation_id = c.id),
                    c.created_at_ms)
             FROM conversations c
             WHERE c.bot_id IS NOT NULL AND c.title LIKE ?1 ESCAPE '\\'
             ORDER BY 4 DESC, c.id DESC LIMIT ?2",
        )?;
        let conversations = statement
            .query_map(params![pattern, CONVERSATION_HITS], |row| {
                Ok(ConversationHit {
                    conversation_id: row.get(0)?,
                    bot_id: row.get(1)?,
                    title: row.get(2)?,
                    last_activity_ms: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(conversations)
    }

    fn search_messages(&self, terms: &[&str]) -> Result<Vec<MessageHit>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT e.id, e.conversation_id, c.bot_id, e.payload, e.created_at_ms
             FROM event_text
             JOIN events e ON e.id = event_text.rowid
             JOIN conversations c ON c.id = e.conversation_id
             WHERE event_text MATCH ?1 AND c.bot_id IS NOT NULL
             ORDER BY event_text.rowid DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![match_expression(terms), MESSAGE_HITS], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut messages = Vec::new();
        for (event_id, conversation_id, bot_id, payload, at_ms) in rows {
            let (author, text) = match serde_json::from_str(&payload)? {
                Event::UserText(text) => (MessageAuthor::User, text),
                Event::MessageText(text) => (MessageAuthor::Bot, text),
                _ => continue,
            };
            messages.push(MessageHit {
                conversation_id,
                bot_id,
                event_id,
                author,
                snippet: snippet(&text, terms),
                at_ms,
            });
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crowbot_protocol::{Avatar, PermissionLevel, TurnEnd};
    use rusqlite::Connection;

    use super::super::{
        EventsWindow, NewBot, SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_VERSION,
    };
    use super::*;

    fn store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn bot(store: &Store, name: &str, title: &str) -> (i64, i64) {
        let row = store
            .create_bot(&NewBot {
                name: name.to_owned(),
                title: title.to_owned(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#3b82f6".to_owned(),
                    glyph: "A".to_owned(),
                },
                provider: "claude".to_owned(),
                model: None,
                permission_level: PermissionLevel::AskFirst,
                workspace: "/work".to_owned(),
            })
            .unwrap();
        (row.bot.id, row.current_conversation_id)
    }

    fn turn(store: &Store, conversation_id: i64, asked: &str, answered: &str) -> (i64, i64) {
        let user = store
            .append(conversation_id, &Event::UserText(asked.to_owned()))
            .unwrap();
        let answer = store
            .append(conversation_id, &Event::MessageText(answered.to_owned()))
            .unwrap();
        store
            .append(conversation_id, &Event::TurnEnded(TurnEnd::Completed))
            .unwrap();
        (user, answer)
    }

    fn snippets(results: &SearchResults) -> Vec<&str> {
        results
            .messages
            .iter()
            .map(|hit| hit.snippet.as_str())
            .collect()
    }

    fn found(store: &Store, query: &str) -> usize {
        store.search(1, query).unwrap().messages.len()
    }

    #[test]
    fn a_query_finds_bots_then_conversations_by_title_then_messages() {
        let store = store();
        let (atlas, atlas_chat) = bot(&store, "Atlas", "Migration planner");
        let (wren, wren_chat) = bot(&store, "Wren", "Writer");
        store.set_title(atlas_chat, "Plan the migration").unwrap();
        store.set_title(wren_chat, "Draft the newsletter").unwrap();
        let (_, answer) = turn(
            &store,
            atlas_chat,
            "Where do we start?",
            "Start with the migration of the events table.",
        );
        turn(&store, wren_chat, "Write a haiku", "Crows over the field");

        let results = store.search(7, "  migration ").unwrap();
        assert_eq!(results.request, 7);
        assert_eq!(results.query, "migration");
        assert_eq!(results.bots, [atlas]);
        assert_eq!(results.conversations.len(), 1);
        assert_eq!(results.conversations[0].conversation_id, atlas_chat);
        assert_eq!(results.conversations[0].bot_id, atlas);
        assert_eq!(results.conversations[0].title, "Plan the migration");
        assert!(results.messages_searched);
        assert_eq!(results.messages.len(), 1);
        let hit = &results.messages[0];
        assert_eq!(
            (hit.event_id, hit.conversation_id, hit.bot_id),
            (answer, atlas_chat, atlas)
        );
        assert_eq!(hit.author, MessageAuthor::Bot);
        assert_eq!(hit.snippet, "Start with the migration of the events table.");

        let by_name = store.search(8, "wre").unwrap();
        assert_eq!(by_name.bots, [wren]);
        assert!(by_name.conversations.is_empty());
        assert!(by_name.messages.is_empty());
    }

    #[test]
    fn messages_match_word_fragments_in_any_letter_case_newest_first() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let (first, _) = turn(&store, chat, "Rename fetchUserProfile please", "Done.");
        let (_, second) = turn(
            &store,
            chat,
            "And now?",
            "I also renamed FETCHUSERPROFILE in the tests.",
        );

        let results = store.search(1, "userprof").unwrap();
        let ids: Vec<i64> = results.messages.iter().map(|hit| hit.event_id).collect();
        assert_eq!(ids, [second, first]);
        assert_eq!(results.messages[1].author, MessageAuthor::User);

        assert_eq!(found(&store, "resume"), 0);
        turn(&store, chat, "Envoie le résumé", "Voilà.");
        assert_eq!(
            snippets(&store.search(3, "resume").unwrap()),
            ["Envoie le résumé"]
        );
    }

    #[test]
    fn every_term_of_a_query_must_be_in_the_message() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        turn(&store, chat, "deploy the staging server", "ok");
        turn(&store, chat, "deploy the production server", "ok");
        assert_eq!(
            snippets(&store.search(1, "deploy production").unwrap()),
            ["deploy the production server"]
        );
        assert_eq!(found(&store, "deploy server"), 2);
        assert_eq!(found(&store, "deploy laptop"), 0);
    }

    #[test]
    fn a_query_with_operators_or_quotes_is_searched_as_plain_text() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        turn(
            &store,
            chat,
            "say \"hello world\" AND (leave",
            "NEAR the end* col:on",
        );
        for query in [
            "\"hello",
            "hello\" AND",
            "AND (leave",
            "(leave",
            "end*",
            "col:on",
            "NEAR the",
            "NEAR(hello world, 5)",
            "\"\"\"",
            "\"",
            "* OR *",
            "a AND b",
            "NOT hello",
            "-hello",
            "^hello",
            "{hello}",
            "hello +",
            "body:hello",
            "100%",
            "under_score",
            "back\\slash",
            "'; DROP TABLE events; --",
        ] {
            let results = store.search(1, query);
            assert!(results.is_ok(), "{query}: {results:?}");
        }
        assert_eq!(found(&store, "\"hello"), 1);
        assert_eq!(found(&store, "AND (leave"), 1);
        assert_eq!(found(&store, "end*"), 1);
        assert_eq!(found(&store, "col:on"), 1);
        assert_eq!(found(&store, "hello OR missing"), 0);
        assert_eq!(found(&store, "hello NOT world"), 0);
        assert_eq!(found(&store, "hel*"), 0);
        assert_eq!(store.events(chat).unwrap().len(), 3);
    }

    #[test]
    fn a_like_wildcard_in_a_query_is_a_plain_character() {
        let store = store();
        let (plain, _) = bot(&store, "Atlas", "");
        let (percent, _) = bot(&store, "100% done", "");
        let (underscore, _) = bot(&store, "snake_case", "");
        assert_eq!(store.search(1, "%").unwrap().bots, [percent]);
        assert_eq!(store.search(1, "_").unwrap().bots, [underscore]);
        assert_eq!(store.search(1, "a").unwrap().bots, [plain, underscore]);
    }

    #[test]
    fn a_query_with_no_term_of_three_characters_skips_the_messages() {
        let store = store();
        let (atlas, chat) = bot(&store, "Atlas", "");
        turn(&store, chat, "at the end", "ok");
        let results = store.search(1, "at").unwrap();
        assert_eq!(results.bots, [atlas]);
        assert!(!results.messages_searched);
        assert!(results.messages.is_empty());
        let empty = store.search(2, "   ").unwrap();
        assert_eq!(
            empty,
            SearchResults {
                request: 2,
                ..SearchResults::default()
            }
        );
    }

    #[test]
    fn a_query_with_no_match_gives_empty_groups() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        turn(&store, chat, "hello", "caw");
        let results = store.search(1, "zeppelin").unwrap();
        assert!(results.bots.is_empty());
        assert!(results.conversations.is_empty());
        assert!(results.messages.is_empty());
        assert!(results.messages_searched);
    }

    #[test]
    fn streamed_text_that_grows_in_place_is_found_with_its_latest_words() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let row = store
            .append(chat, &Event::MessageText("The first half".to_owned()))
            .unwrap();
        assert_eq!(found(&store, "second half"), 0);
        store
            .rewrite(
                row,
                &Event::MessageText("The first half and the second half".to_owned()),
            )
            .unwrap();
        let results = store.search(2, "second half").unwrap();
        assert_eq!(results.messages.len(), 1);
        assert_eq!(results.messages[0].event_id, row);
        assert_eq!(found(&store, "first half"), 1);
    }

    #[test]
    fn deleted_history_leaves_the_index_and_the_bot_gets_a_new_conversation() {
        let store = store();
        let (atlas, chat) = bot(&store, "Atlas", "");
        let (wren, wren_chat) = bot(&store, "Wren", "");
        turn(&store, chat, "remember the lighthouse", "noted");
        turn(&store, wren_chat, "the lighthouse keeper", "noted");
        store.new_conversation(atlas).unwrap().unwrap();
        store.enqueue(chat, "queued").unwrap();
        assert_eq!(found(&store, "lighthouse"), 2);

        let current = store.delete_history(atlas).unwrap().unwrap();
        assert_ne!(current, chat);
        assert_eq!(
            snippets(&store.search(2, "lighthouse").unwrap()),
            ["the lighthouse keeper"]
        );
        assert_eq!(store.conversation_ids(atlas).unwrap(), [current]);
        assert_eq!(
            store.bot(atlas).unwrap().unwrap().current_conversation_id,
            current
        );
        assert!(store.events(current).unwrap().is_empty());
        assert_eq!(store.conversation_ids(wren).unwrap(), [wren_chat]);
        assert_eq!(store.events(wren_chat).unwrap().len(), 3);
        assert_eq!(store.delete_history(99).unwrap(), None);
    }

    #[test]
    fn a_long_message_shows_the_text_around_the_match() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let long = format!(
            "{} the needle sits here {}",
            "hay ".repeat(60),
            "straw ".repeat(60)
        );
        turn(&store, chat, &long, "ok");
        let results = store.search(1, "needle").unwrap();
        let shown = &results.messages[0].snippet;
        assert!(shown.starts_with('…') && shown.ends_with('…'), "{shown}");
        assert!(shown.contains("the needle sits here"));
        assert!(shown.chars().count() <= SNIPPET_LENGTH + 2);
    }

    #[test]
    fn a_page_from_a_hit_starts_at_its_turn_and_ends_where_the_client_starts() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let (_, other) = bot(&store, "Wren", "");
        turn(&store, chat, "one", "first answer");
        turn(&store, other, "elsewhere", "other answer");
        let (asked, answer) = turn(&store, chat, "two", "second answer");
        turn(&store, chat, "three", "third answer");
        let (last, _) = turn(&store, chat, "four", "fourth answer");

        let page = store.events_around(chat, answer, last, 100, 10).unwrap();
        let ids: Vec<i64> = page.events.iter().map(|event| event.id).collect();
        assert_eq!(ids.first(), Some(&asked));
        assert_eq!(ids.len(), 6);
        assert!(ids.iter().all(|id| (asked..last).contains(id)));
        assert_eq!(page.older, Some(asked));
        assert_eq!(page.newer, None);

        let first = store.events_around(chat, 2, last, 100, 10).unwrap();
        assert_eq!(first.events[0].id, 1);
        assert_eq!(first.older, None);
        assert_eq!(first.newer, None);
    }

    #[test]
    fn a_hit_far_from_what_the_client_holds_comes_as_a_page_around_it() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let (_, other) = bot(&store, "Wren", "");
        turn(&store, chat, "zero", "an answer");
        let (asked, answer) = turn(&store, chat, "one", "the needle");
        let mut last = 0;
        for index in 0..20 {
            turn(&store, other, "elsewhere", "other answer");
            (last, _) = turn(&store, chat, &format!("filler {index}"), "ok");
        }
        let ids = |window: &EventsWindow| -> Vec<i64> {
            window.events.iter().map(|event| event.id).collect()
        };

        // 60 events lie from the turn of the hit to the client's: more
        // than the 20 that may join them, so 8 come, from the turn's start.
        let window = store.events_around(chat, answer, last, 20, 8).unwrap();
        let shown = ids(&window);
        assert_eq!(shown.len(), 8);
        assert_eq!(shown[0], asked);
        assert!(shown.contains(&answer));
        assert_eq!(window.older, Some(asked));
        assert_eq!(window.newer, shown.last().copied());

        // The pages after it reach the client's events with no gap and no
        // event of another conversation.
        let mut all = shown;
        let mut newer = window.newer;
        let mut pages = 0;
        while let Some(after) = newer {
            let page = store.events_between(chat, after, last, 8).unwrap();
            assert!(page.events.len() <= 8);
            all.extend(ids(&page));
            newer = page.newer;
            pages += 1;
        }
        assert_eq!(pages, 7);
        assert_eq!(all.len(), 60);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(all.iter().all(|id| (asked..last).contains(id)));
        let joined = store.events_around(chat, answer, last, 100, 8).unwrap();
        assert_eq!(ids(&joined), all, "the same events as in one answer");
    }

    #[test]
    fn a_hit_deep_in_a_long_turn_is_inside_its_page() {
        let store = store();
        let (_, chat) = bot(&store, "Atlas", "");
        let asked = store
            .append(chat, &Event::UserText("one long turn".to_owned()))
            .unwrap();
        let mut hit = 0;
        for index in 0..40 {
            let id = store
                .append(chat, &Event::MessageText(format!("part {index}")))
                .unwrap();
            if index == 30 {
                hit = id;
            }
        }
        let (last, _) = turn(&store, chat, "later", "ok");

        let window = store.events_around(chat, hit, last, 20, 8).unwrap();
        let shown: Vec<i64> = window.events.iter().map(|event| event.id).collect();
        assert_eq!(shown.len(), 8);
        assert!(shown[0] > asked, "the start of the turn is too far to come");
        assert_eq!(shown[3], hit, "half of the page leads to the hit");
        assert_eq!(window.older, Some(shown[0]));
        assert_eq!(window.newer, Some(shown[7]));
    }

    #[test]
    fn events_stored_before_the_index_existed_are_searchable_after_the_migration() {
        let connection = Connection::open_in_memory().unwrap();
        for schema in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5] {
            connection.execute_batch(schema).unwrap();
        }
        connection.pragma_update(None, "user_version", 5).unwrap();
        connection
            .execute_batch(
                r##"INSERT INTO bots (name, avatar_color, avatar_glyph, provider, workspace, created_at_ms)
                    VALUES ('Atlas', '#3b82f6', 'A', 'claude', '/work', 5);
                 INSERT INTO conversations (bot_id, provider, cwd, created_at_ms)
                    VALUES (1, 'claude', '/work', 6);
                 UPDATE bots SET current_conversation_id = 1;
                 INSERT INTO events (conversation_id, kind, payload, created_at_ms) VALUES
                    (1, 'user_text', '{"type":"user_text","data":"find the old albatross"}', 7),
                    (1, 'thought_text', '{"type":"thought_text","data":"albatross thoughts"}', 8),
                    (1, 'message_text', '{"type":"message_text","data":"The albatross was here."}', 9),
                    (1, 'message_text', 'not json albatross', 10),
                    (1, 'turn_ended', '{"type":"turn_ended","data":"completed"}', 11);"##,
            )
            .unwrap();
        let store = Store::from_connection(connection).unwrap();
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let results = store.search(1, "albatross").unwrap();
        let ids: Vec<i64> = results.messages.iter().map(|hit| hit.event_id).collect();
        assert_eq!(ids, [3, 1]);
        let kept: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 5);
        store
            .append(1, &Event::UserText("a new albatross".to_owned()))
            .unwrap();
        assert_eq!(found(&store, "albatross"), 3);
    }

    #[test]
    #[ignore = "seeds 100,000 events: run with --ignored"]
    fn a_search_over_100_000_events_answers_within_200_ms() {
        let store = store();
        let (_, first) = bot(&store, "Atlas", "Planner");
        let mut chats = vec![first];
        for index in 0..19 {
            chats.push(bot(&store, &format!("Bot {index}"), "").1);
        }
        let words = [
            "migration",
            "release",
            "invoice",
            "sparrow",
            "lantern",
            "harbor",
            "cobalt",
            "meadow",
            "quartz",
            "ember",
            "willow",
            "summit",
        ];
        let transaction = store.connection.unchecked_transaction().unwrap();
        for index in 0..100_000_usize {
            let chat = chats[index % chats.len()];
            let text = format!(
                "Message {index} about the {} of the {} near the {}, written to fill a few lines of a conversation the way a bot answers.",
                words[index % words.len()],
                words[(index / 7) % words.len()],
                words[(index / 31) % words.len()]
            );
            let event = match index % 3 {
                0 => Event::UserText(text),
                1 => Event::MessageText(text),
                _ => Event::TurnEnded(TurnEnd::Completed),
            };
            store.append(chat, &event).unwrap();
        }
        store
            .append(
                first,
                &Event::MessageText("the only pterodactyl in the database".to_owned()),
            )
            .unwrap();
        transaction.commit().unwrap();

        for query in [
            "pterodactyl",
            "sparrow",
            "the",
            "lantern harbor",
            "Message 4242",
            "written to fill a few lines",
            "zzzzzz",
            "at",
            "Bot 1",
        ] {
            let started = Instant::now();
            let results = store.search(1, query).unwrap();
            let elapsed = started.elapsed();
            println!(
                "{query:?}: {} messages in {elapsed:?}",
                results.messages.len()
            );
            assert!(elapsed.as_millis() < 200, "{query}: {elapsed:?}");
        }
        assert_eq!(found(&store, "pterodactyl"), 1);
        assert_eq!(found(&store, "sparrow"), 20);
    }
}
