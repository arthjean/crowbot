//! The system prompt in tiers. A session's system prompt holds the bot's
//! instructions, then the registered sections (memory, the skills index) in
//! the order of their rank, as they were when the session opened. A section
//! that changes later does not touch the prompt, which the provider's prompt
//! cache keeps, nor the session fingerprint: the next prompt carries what
//! changed in one note before the person's text (decision D7).

use std::collections::BTreeMap;

/// A section as it stands now: a version that changes with its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionState {
    pub version: String,
    pub text: String,
}

/// One part of the system prompt that the daemon keeps.
pub trait Section: Send + Sync {
    /// The stable name its versions are stored under.
    fn id(&self) -> &'static str;
    /// Where it stands in the prompt: lower ranks come first.
    fn rank(&self) -> u32;
    /// The section for a bot, or `None` when the bot has nothing in it.
    fn current(&self, bot_id: i64) -> Option<SectionState>;
    /// What changed since the version `seen`, for the note.
    fn difference(&self, bot_id: i64, seen: Option<&str>, now: Option<&SectionState>) -> String;
}

/// What a session was given when it opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub versions: BTreeMap<String, String>,
    texts: Vec<String>,
}

/// The registered sections, in the order of their rank.
#[derive(Default)]
pub struct Sections {
    sections: Vec<Box<dyn Section>>,
}

/// The block before the person's text that tells what changed.
const NOTE_HEAD: &str = "[Crow Bot note: what follows changed since this session started, and replaces what the session was given then.]";

impl Sections {
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut sections = Self::default();
        #[cfg(debug_assertions)]
        if let Some(folder) = crate::test_harness::folder() {
            crate::test_harness::add_sections(&mut sections, &folder);
        }
        sections
    }

    /// Registers a section. Two sections of one rank keep the order they
    /// were registered in.
    pub fn register(&mut self, section: impl Section + 'static) {
        let rank = section.rank();
        let at = self
            .sections
            .iter()
            .position(|other| other.rank() > rank)
            .unwrap_or(self.sections.len());
        self.sections.insert(at, Box::new(section));
    }

    pub fn snapshot(&self, bot_id: i64) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for section in &self.sections {
            if let Some(state) = section.current(bot_id) {
                snapshot
                    .versions
                    .insert(section.id().to_owned(), state.version);
                snapshot.texts.push(state.text);
            }
        }
        snapshot
    }

    /// The system prompt: the instructions first, then each section.
    pub fn system_prompt(instructions: &str, snapshot: &Snapshot) -> String {
        std::iter::once(instructions)
            .chain(snapshot.texts.iter().map(String::as_str))
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The note of what changed since the versions `seen`, and the versions
    /// the session has seen once it reads the note. No change, no note.
    pub fn note(
        &self,
        bot_id: i64,
        seen: &BTreeMap<String, String>,
    ) -> (Option<String>, BTreeMap<String, String>) {
        let mut now = BTreeMap::new();
        let mut parts = Vec::new();
        for section in &self.sections {
            let id = section.id();
            let state = section.current(bot_id);
            let before = seen.get(id).map(String::as_str);
            if let Some(state) = &state {
                now.insert(id.to_owned(), state.version.clone());
            }
            if before != state.as_ref().map(|state| state.version.as_str()) {
                parts.push(section.difference(bot_id, before, state.as_ref()));
            }
        }
        if parts.is_empty() {
            return (None, now);
        }
        let note = std::iter::once(NOTE_HEAD.to_owned())
            .chain(parts)
            .collect::<Vec<_>>()
            .join("\n\n");
        (Some(note), now)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct Fixed {
        id: &'static str,
        rank: u32,
        text: Arc<Mutex<Option<String>>>,
    }

    impl Section for Fixed {
        fn id(&self) -> &'static str {
            self.id
        }

        fn rank(&self) -> u32 {
            self.rank
        }

        fn current(&self, _bot_id: i64) -> Option<SectionState> {
            let text = self.text.lock().unwrap().clone()?;
            Some(SectionState {
                version: format!("v-{text}"),
                text,
            })
        }

        fn difference(&self, _bot: i64, _seen: Option<&str>, now: Option<&SectionState>) -> String {
            match now {
                Some(now) => format!("{} is now: {}", self.id, now.text),
                None => format!("{} is gone", self.id),
            }
        }
    }

    fn section(id: &'static str, rank: u32, text: &str) -> (Fixed, Arc<Mutex<Option<String>>>) {
        let text = Arc::new(Mutex::new(Some(text.to_owned())));
        (
            Fixed {
                id,
                rank,
                text: text.clone(),
            },
            text,
        )
    }

    #[test]
    fn the_instructions_come_first_and_the_sections_follow_by_rank() {
        let mut sections = Sections::default();
        let (late, _) = section("skills", 20, "Skills index");
        let (early, _) = section("memory", 10, "Core memory");
        sections.register(late);
        sections.register(early);
        let snapshot = sections.snapshot(1);
        assert_eq!(
            Sections::system_prompt("Be brief.", &snapshot),
            "Be brief.\n\nCore memory\n\nSkills index"
        );
        assert_eq!(
            Sections::system_prompt("", &snapshot),
            "Core memory\n\nSkills index"
        );
        assert_eq!(snapshot.versions["memory"], "v-Core memory");
    }

    #[test]
    fn a_change_makes_one_note_and_the_versions_follow_it() {
        let mut sections = Sections::default();
        let (memory, memory_text) = section("memory", 10, "one");
        let (skills, skills_text) = section("skills", 20, "index");
        sections.register(memory);
        sections.register(skills);
        let seen = sections.snapshot(1).versions;
        let (none, same) = sections.note(1, &seen);
        assert_eq!(none, None);
        assert_eq!(same, seen);
        *memory_text.lock().unwrap() = Some("two".to_owned());
        *skills_text.lock().unwrap() = None;
        let (note, now) = sections.note(1, &seen);
        let note = note.unwrap();
        assert!(note.starts_with(NOTE_HEAD), "{note}");
        assert!(note.contains("memory is now: two"), "{note}");
        assert!(note.contains("skills is gone"), "{note}");
        assert_eq!(now.get("memory").map(String::as_str), Some("v-two"));
        assert!(!now.contains_key("skills"));
        assert_eq!(sections.note(1, &now).0, None);
    }
}
