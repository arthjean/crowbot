//! Skills: each version stored by its hash, each file of a version stored
//! once by the SHA-256 of its content, and a log of every change of a
//! skill, which no operation rewrites or removes (`SCHEMA_V13`). A skill
//! names its active version; putting an earlier one back is a change like
//! any other.

use std::collections::BTreeMap;

use crowbot_protocol::{
    Event, SkillAuthor, SkillChange, SkillChangeKind, SkillState, SkillSummary, SkillVersion,
};
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, now_ms};
use crate::skills::format::{self, Version};

/// A skill as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRow {
    pub id: i64,
    pub name: String,
    /// The hash of the active version.
    pub version: String,
    pub description: String,
    pub state: SkillState,
    pub pinned: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl SkillRow {
    pub fn summary(&self) -> SkillSummary {
        SkillSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            state: self.state,
            pinned: self.pinned,
            hash: self.version.clone(),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

/// Who changes skills, and in which conversation.
#[derive(Debug, Clone, Copy)]
pub struct SkillWriter {
    pub author: SkillAuthor,
    pub bot_id: i64,
    pub conversation_id: Option<i64>,
}

/// What a change expects of a skill before it applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// A person's change, made on the skill as it is.
    Any,
    /// No skill has the name yet.
    New,
    /// The active version is the one the writer read.
    Version(String),
}

/// The change of one skill: a version to make active, a state, a pin.
#[derive(Debug, Clone)]
pub struct SkillPlan {
    pub name: String,
    pub expected: Expected,
    pub version: Option<(Version, SkillChangeKind)>,
    pub state: Option<SkillState>,
    pub pinned: Option<bool>,
}

impl SkillPlan {
    pub fn new(name: &str, expected: Expected) -> Self {
        Self {
            name: name.to_owned(),
            expected,
            version: None,
            state: None,
            pinned: None,
        }
    }
}

/// A change of the skills index since a version of it, for the note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChange {
    pub kind: SkillChangeKind,
    pub name: String,
    pub description: String,
}

/// The kinds of change that change what the index shows.
const INDEX_KINDS: &str =
    "'created', 'corrected', 'rolled_back', 'imported', 'archived', 'restored'";
/// The kinds of change that make a version active.
const VERSION_KINDS: &str = "'created', 'corrected', 'rolled_back', 'imported'";

fn kind_name(kind: SkillChangeKind) -> &'static str {
    match kind {
        SkillChangeKind::Created => "created",
        SkillChangeKind::Corrected => "corrected",
        SkillChangeKind::Archived => "archived",
        SkillChangeKind::Restored => "restored",
        SkillChangeKind::RolledBack => "rolled_back",
        SkillChangeKind::Imported => "imported",
        SkillChangeKind::Pinned => "pinned",
        SkillChangeKind::Unpinned => "unpinned",
        SkillChangeKind::Other => "other",
    }
}

fn kind_of(name: &str) -> SkillChangeKind {
    match name {
        "created" => SkillChangeKind::Created,
        "corrected" => SkillChangeKind::Corrected,
        "archived" => SkillChangeKind::Archived,
        "restored" => SkillChangeKind::Restored,
        "rolled_back" => SkillChangeKind::RolledBack,
        "imported" => SkillChangeKind::Imported,
        "pinned" => SkillChangeKind::Pinned,
        "unpinned" => SkillChangeKind::Unpinned,
        _ => SkillChangeKind::Other,
    }
}

fn author_name(author: SkillAuthor) -> &'static str {
    match author {
        SkillAuthor::Bot => "bot",
        SkillAuthor::Review => "review",
        SkillAuthor::User => "user",
    }
}

fn author_of(name: &str) -> SkillAuthor {
    match name {
        "review" => SkillAuthor::Review,
        "user" => SkillAuthor::User,
        _ => SkillAuthor::Bot,
    }
}

fn state_name(state: SkillState) -> &'static str {
    match state {
        SkillState::Active => "active",
        SkillState::Archived => "archived",
    }
}

const SKILL_COLUMNS: &str =
    "id, name, version, description, state, pinned, created_at_ms, updated_at_ms";

fn skill_row(row: &Row<'_>) -> rusqlite::Result<SkillRow> {
    let state: String = row.get(4)?;
    Ok(SkillRow {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        description: row.get(3)?,
        state: if state == "archived" {
            SkillState::Archived
        } else {
            SkillState::Active
        },
        pinned: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn refused(message: impl Into<String>) -> StoreError {
    StoreError::Refused(message.into())
}

impl Store {
    /// The skills of a bot, by name.
    pub fn skills(&self, bot_id: i64) -> Result<Vec<SkillRow>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {SKILL_COLUMNS} FROM skills WHERE bot_id = ?1 ORDER BY name"
        ))?;
        let rows = statement
            .query_map(params![bot_id], skill_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The active skills of a bot, by name.
    pub fn active_skills(&self, bot_id: i64) -> Result<Vec<SkillRow>, StoreError> {
        Ok(self
            .skills(bot_id)?
            .into_iter()
            .filter(|skill| skill.state == SkillState::Active)
            .collect())
    }

    pub fn skill(&self, bot_id: i64, name: &str) -> Result<Option<SkillRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!("SELECT {SKILL_COLUMNS} FROM skills WHERE bot_id = ?1 AND name = ?2"),
                params![bot_id, name],
                skill_row,
            )
            .optional()?;
        Ok(row)
    }

    /// The files of a stored version.
    pub fn skill_version(&self, hash: &str) -> Result<Option<Version>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT v.path, f.content FROM skill_version_files v
             JOIN skill_files f ON f.hash = v.file WHERE v.version = ?1",
        )?;
        let files = statement
            .query_map(params![hash], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok((!files.is_empty()).then(|| Version::new(files)))
    }

    /// The SHA-256 of each file of a stored version, by path.
    pub fn skill_version_files(&self, hash: &str) -> Result<BTreeMap<String, String>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT path, file FROM skill_version_files WHERE version = ?1")?;
        let files = statement
            .query_map(params![hash], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(files)
    }

    /// The content of one stored file, by its SHA-256.
    pub fn skill_file(&self, hash: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let content = self
            .connection
            .query_row(
                "SELECT content FROM skill_files WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(content)
    }

    /// Stores a version of the skill `name`, checked against the format and
    /// its limits, and gives its hash. A version already stored, and each
    /// file already stored, is kept once.
    pub fn store_skill_version(&self, name: &str, version: &Version) -> Result<String, StoreError> {
        format::check(name, version).map_err(StoreError::Refused)?;
        let hash = version.hash();
        self.connection.execute(
            "INSERT OR IGNORE INTO skill_versions (hash, created_at_ms) VALUES (?1, ?2)",
            params![hash, now_ms()],
        )?;
        for (path, content) in version.files() {
            let file = crate::digest::sha256_hex(content);
            self.connection.execute(
                "INSERT OR IGNORE INTO skill_files (hash, content) VALUES (?1, ?2)",
                params![file, content],
            )?;
            self.connection.execute(
                "INSERT OR IGNORE INTO skill_version_files (version, path, file) VALUES (?1, ?2, ?3)",
                params![hash, path, file],
            )?;
        }
        Ok(hash)
    }

    /// The versions a skill had, the oldest first, each with who made it.
    pub fn skill_history(
        &self,
        bot_id: i64,
        name: &str,
    ) -> Result<Option<Vec<SkillVersion>>, StoreError> {
        let Some(skill) = self.skill(bot_id, name)? else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT version, created_at_ms, author, conversation_id, kind FROM skill_changes
             WHERE skill_id = ?1 AND kind IN ({VERSION_KINDS}) ORDER BY seq"
        ))?;
        let mut versions = statement
            .query_map(params![skill.id], |row| {
                Ok(SkillVersion {
                    hash: row.get(0)?,
                    created_at_ms: row.get(1)?,
                    author: author_of(&row.get::<_, String>(2)?),
                    conversation_id: row.get(3)?,
                    kind: kind_of(&row.get::<_, String>(4)?),
                    active: false,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(active) = versions
            .iter_mut()
            .rev()
            .find(|version| version.hash == skill.version)
        {
            active.active = true;
        }
        Ok(Some(versions))
    }

    /// The last change of what a bot's skills index shows; `None` when its
    /// skills never changed.
    pub fn skills_index_version(&self, bot_id: i64) -> Result<Option<i64>, StoreError> {
        let version = self.connection.query_row(
            &format!(
                "SELECT MAX(c.seq) FROM skill_changes c JOIN skills s ON s.id = c.skill_id
                 WHERE s.bot_id = ?1 AND c.kind IN ({INDEX_KINDS})"
            ),
            params![bot_id],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    /// The changes of what a bot's skills index shows since a version of it.
    pub fn skill_index_changes_since(
        &self,
        bot_id: i64,
        seq: i64,
    ) -> Result<Vec<IndexChange>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT c.kind, s.name, s.description FROM skill_changes c
             JOIN skills s ON s.id = c.skill_id
             WHERE s.bot_id = ?1 AND c.seq > ?2 AND c.kind IN ({INDEX_KINDS}) ORDER BY c.seq"
        ))?;
        let changes = statement
            .query_map(params![bot_id, seq], |row| {
                Ok(IndexChange {
                    kind: kind_of(&row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    description: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(changes)
    }

    /// Applies a change and stores the event that records it in the
    /// conversation, in one transaction; the event, when anything changed.
    pub fn record_skill_change(
        &self,
        conversation_id: i64,
        writer: &SkillWriter,
        plans: &[SkillPlan],
    ) -> Result<Option<Event>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let changes = self.change_skills(writer, plans)?;
        let event = changed(writer.author, changes);
        if let Some(event) = &event {
            self.append(conversation_id, event)?;
        }
        transaction.commit()?;
        Ok(event)
    }

    /// Applies the change of every skill, in order, or none: a plan that
    /// cannot apply fails the whole change with `StoreError::Refused`. Only
    /// a person archives a pinned skill. No change removes a stored version.
    pub fn change_skills(
        &self,
        writer: &SkillWriter,
        plans: &[SkillPlan],
    ) -> Result<Vec<SkillChange>, StoreError> {
        if self
            .connection
            .query_row(
                "SELECT 1 FROM bots WHERE id = ?1",
                params![writer.bot_id],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(refused(format!("Bot {} does not exist.", writer.bot_id)));
        }
        self.connection.execute_batch("SAVEPOINT skill_change")?;
        let outcome = plans
            .iter()
            .map(|plan| self.change_skill(writer, plan))
            .collect::<Result<Vec<_>, _>>();
        match outcome {
            Ok(changes) => {
                self.connection.execute_batch("RELEASE skill_change")?;
                Ok(changes.into_iter().flatten().collect())
            }
            Err(error) => {
                self.connection
                    .execute_batch("ROLLBACK TO skill_change; RELEASE skill_change")?;
                Err(error)
            }
        }
    }

    fn change_skill(
        &self,
        writer: &SkillWriter,
        plan: &SkillPlan,
    ) -> Result<Option<SkillChange>, StoreError> {
        let name = &plan.name;
        let current = self.skill(writer.bot_id, name)?;
        match (&plan.expected, &current) {
            (Expected::New, Some(skill)) => {
                let archived = if skill.state == SkillState::Archived {
                    ", archived,"
                } else {
                    ""
                };
                return Err(refused(format!(
                    "A skill named `{name}` exists already{archived} so it was not created: correct it instead, against the version skill_view gives."
                )));
            }
            (Expected::Version(_) | Expected::Any, None) if plan.version.is_none() => {
                return Err(refused(format!("No skill is named `{name}`.")));
            }
            (Expected::Version(_), None) => {
                return Err(refused(format!("No skill is named `{name}`.")));
            }
            (Expected::Version(read), Some(skill)) if *read != skill.version => {
                return Err(refused(format!(
                    "The skill `{name}` changed since you read it: its version is now {}, not {read}. Read it again with skill_view, then make your change against that version.",
                    skill.version
                )));
            }
            _ => {}
        }
        if let Some(skill) = &current
            && skill.pinned
            && writer.author != SkillAuthor::User
            && plan.state == Some(SkillState::Archived)
            && skill.state != SkillState::Archived
        {
            return Err(refused(format!(
                "The skill `{name}` is pinned, so only a person can archive it."
            )));
        }
        let now = now_ms();
        let old_hash = current.as_ref().map(|skill| skill.version.clone());
        let mut skill = match current {
            Some(skill) => skill,
            None => {
                let Some((version, _)) = &plan.version else {
                    return Err(refused(format!("No skill is named `{name}`.")));
                };
                let front = format::check(name, version).map_err(StoreError::Refused)?;
                let hash = self.store_skill_version(name, version)?;
                self.connection.execute(
                    "INSERT INTO skills (bot_id, name, version, description, state, pinned,
                        created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, 'active', 0, ?5, ?5)",
                    params![writer.bot_id, name, hash, front.description, now],
                )?;
                let id = self.connection.last_insert_rowid();
                self.log_skill_change(writer, id, creation(plan), &hash, now)?;
                SkillRow {
                    id,
                    name: name.clone(),
                    version: hash,
                    description: front.description,
                    state: SkillState::Active,
                    pinned: false,
                    created_at_ms: now,
                    updated_at_ms: now,
                }
            }
        };
        let mut kinds = Vec::new();
        if old_hash.is_none() {
            kinds.push(creation(plan));
        } else if let Some((version, kind)) = &plan.version {
            let front = format::check(name, version).map_err(StoreError::Refused)?;
            let hash = self.store_skill_version(name, version)?;
            if hash != skill.version {
                self.connection.execute(
                    "UPDATE skills SET version = ?2, description = ?3, updated_at_ms = ?4
                     WHERE id = ?1",
                    params![skill.id, hash, front.description, now],
                )?;
                self.log_skill_change(writer, skill.id, *kind, &hash, now)?;
                skill.version = hash;
                skill.description = front.description;
                kinds.push(*kind);
            }
        }
        if let Some(state) = plan.state
            && state != skill.state
        {
            self.connection.execute(
                "UPDATE skills SET state = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![skill.id, state_name(state), now],
            )?;
            let kind = match state {
                SkillState::Active => SkillChangeKind::Restored,
                SkillState::Archived => SkillChangeKind::Archived,
            };
            self.log_skill_change(writer, skill.id, kind, &skill.version, now)?;
            kinds.push(kind);
        }
        if let Some(pinned) = plan.pinned
            && pinned != skill.pinned
        {
            self.connection.execute(
                "UPDATE skills SET pinned = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![skill.id, pinned, now],
            )?;
            let kind = if pinned {
                SkillChangeKind::Pinned
            } else {
                SkillChangeKind::Unpinned
            };
            self.log_skill_change(writer, skill.id, kind, &skill.version, now)?;
            kinds.push(kind);
        }
        // One entry per skill: a state change says more than the version
        // that came with it, and a new skill is new whatever followed.
        let kind = if old_hash.is_none() {
            Some(creation(plan))
        } else {
            kinds
                .iter()
                .rev()
                .find(|kind| matches!(kind, SkillChangeKind::Archived | SkillChangeKind::Restored))
                .or(kinds.first())
                .copied()
        };
        Ok(kind.map(|kind| SkillChange {
            name: name.clone(),
            kind,
            old_hash,
            new_hash: Some(skill.version.clone()),
        }))
    }

    fn log_skill_change(
        &self,
        writer: &SkillWriter,
        skill_id: i64,
        kind: SkillChangeKind,
        version: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO skill_changes (skill_id, kind, version, author, conversation_id,
                created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                skill_id,
                kind_name(kind),
                version,
                author_name(writer.author),
                writer.conversation_id,
                now
            ],
        )?;
        Ok(())
    }

    /// Removes the skills of a bot that is deleted, and every stored
    /// version and file no other skill names.
    pub(super) fn forget_skills(&self, bot_id: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM skill_changes WHERE skill_id IN (SELECT id FROM skills WHERE bot_id = ?1)",
            params![bot_id],
        )?;
        self.connection
            .execute("DELETE FROM skills WHERE bot_id = ?1", params![bot_id])?;
        self.connection.execute_batch(
            "DELETE FROM skill_version_files WHERE version NOT IN
                (SELECT version FROM skills UNION SELECT version FROM skill_changes);
             DELETE FROM skill_versions WHERE hash NOT IN
                (SELECT version FROM skill_version_files);
             DELETE FROM skill_files WHERE hash NOT IN (SELECT file FROM skill_version_files);",
        )?;
        Ok(())
    }
}

/// How a plan that makes a new skill names its first version: imported by
/// a person, or created.
fn creation(plan: &SkillPlan) -> SkillChangeKind {
    match &plan.version {
        Some((_, SkillChangeKind::Imported)) => SkillChangeKind::Imported,
        _ => SkillChangeKind::Created,
    }
}

/// The event that records a change, when it changed anything.
pub fn changed(author: SkillAuthor, changes: Vec<SkillChange>) -> Option<Event> {
    (!changes.is_empty()).then_some(Event::SkillChanged { author, changes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::{Avatar, PermissionLevel};

    use crate::store::{NewBot, SharedStore};

    fn document(name: &str, description: &str, body: &str) -> Version {
        Version::of_document(&format!(
            "---\nname: {name}\ndescription: {description}\n---\n{body}\n"
        ))
    }

    fn bot(store: &Store, name: &str) -> i64 {
        store
            .create_bot(&NewBot {
                name: name.to_owned(),
                title: String::new(),
                instructions: String::new(),
                avatar: Avatar {
                    color: "#10b981".to_owned(),
                    shape: Default::default(),
                },
                provider: "claude".to_owned(),
                model: None,
                effort: None,
                permission_level: PermissionLevel::AskFirst,
                tool_call_limit: None,
                workspace: "/bots".to_owned(),
            })
            .unwrap()
            .bot
            .id
    }

    fn writer(bot_id: i64, author: SkillAuthor) -> SkillWriter {
        SkillWriter {
            author,
            bot_id,
            conversation_id: None,
        }
    }

    fn create(name: &str, version: Version) -> SkillPlan {
        SkillPlan {
            version: Some((version, SkillChangeKind::Created)),
            ..SkillPlan::new(name, Expected::New)
        }
    }

    fn count(store: &Store, table: &str) -> i64 {
        store
            .connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn the_same_content_is_one_version_and_one_stored_copy() {
        let shared = SharedStore::in_memory();
        shared.with(|store| {
            let one = bot(store, "One");
            let two = bot(store, "Two");
            let mut version = document("deploy", "Deploys.", "Run it.");
            version.put("scripts/run.sh", b"echo run".to_vec());
            for bot_id in [one, two] {
                store
                    .change_skills(
                        &writer(bot_id, SkillAuthor::Bot),
                        &[create("deploy", version.clone())],
                    )
                    .unwrap();
            }
            let first = store.skill(one, "deploy").unwrap().unwrap();
            let second = store.skill(two, "deploy").unwrap().unwrap();
            assert_eq!(first.version, second.version);
            assert_eq!(first.version, version.hash());
            assert_eq!(count(store, "skill_versions"), 1);
            assert_eq!(count(store, "skill_files"), 2);
            assert_eq!(store.skill_version(&first.version).unwrap(), Some(version));
        });
    }

    #[test]
    fn a_version_outside_the_format_is_refused_and_nothing_is_kept() {
        let shared = SharedStore::in_memory();
        shared.with(|store| {
            let id = bot(store, "One");
            let mut leaving = document("deploy", "Deploys.", "Run it.");
            leaving.put("scripts/../../escape", b"x".to_vec());
            let error = store
                .change_skills(
                    &writer(id, SkillAuthor::Bot),
                    &[
                        create("good", document("good", "Good.", "Body")),
                        create("deploy", leaving),
                    ],
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("leave the skill's folder"),
                "{error}"
            );
            assert_eq!(store.skills(id).unwrap(), Vec::new());
            assert_eq!(count(store, "skill_versions"), 0);
        });
    }

    #[test]
    fn a_change_against_an_old_version_is_refused_and_a_pinned_skill_stays() {
        let shared = SharedStore::in_memory();
        shared.with(|store| {
            let id = bot(store, "One");
            let bot_writer = writer(id, SkillAuthor::Bot);
            store
                .change_skills(
                    &bot_writer,
                    &[create("deploy", document("deploy", "One.", "1"))],
                )
                .unwrap();
            let first = store.skill(id, "deploy").unwrap().unwrap().version;
            let correct = |base: &str, body: &str| SkillPlan {
                version: Some((document("deploy", "Two.", body), SkillChangeKind::Corrected)),
                ..SkillPlan::new("deploy", Expected::Version(base.to_owned()))
            };
            let changes = store
                .change_skills(&bot_writer, &[correct(&first, "2")])
                .unwrap();
            assert_eq!(changes[0].kind, SkillChangeKind::Corrected);
            assert_eq!(changes[0].old_hash.as_deref(), Some(first.as_str()));
            let stale = store
                .change_skills(&bot_writer, &[correct(&first, "3")])
                .unwrap_err();
            assert!(stale.to_string().contains("Read it again"), "{stale}");
            let pin = SkillPlan {
                pinned: Some(true),
                ..SkillPlan::new("deploy", Expected::Any)
            };
            store
                .change_skills(&writer(id, SkillAuthor::User), &[pin])
                .unwrap();
            let archive = SkillPlan {
                state: Some(SkillState::Archived),
                ..SkillPlan::new("deploy", Expected::Any)
            };
            let pinned = store
                .change_skills(&bot_writer, std::slice::from_ref(&archive))
                .unwrap_err();
            assert!(pinned.to_string().contains("pinned"), "{pinned}");
            let archived = store
                .change_skills(&writer(id, SkillAuthor::User), &[archive])
                .unwrap();
            assert_eq!(archived[0].kind, SkillChangeKind::Archived);
            // Every version stays stored, and the history lists both.
            let history = store.skill_history(id, "deploy").unwrap().unwrap();
            assert_eq!(history.len(), 2);
            assert!(history[1].active);
            assert!(store.skill_version(&first).unwrap().is_some());
        });
    }

    #[test]
    fn a_deleted_bot_takes_its_skills_and_the_versions_no_one_else_names() {
        let shared = SharedStore::in_memory();
        shared.with(|store| {
            let one = bot(store, "One");
            let two = bot(store, "Two");
            let shared_version = document("shared", "Shared.", "Same");
            for bot_id in [one, two] {
                store
                    .change_skills(
                        &writer(bot_id, SkillAuthor::Bot),
                        &[create("shared", shared_version.clone())],
                    )
                    .unwrap();
            }
            store
                .change_skills(
                    &writer(one, SkillAuthor::Bot),
                    &[create("own", document("own", "Own.", "Mine"))],
                )
                .unwrap();
            assert_eq!(count(store, "skill_versions"), 2);
            store.delete_bot(one).unwrap();
            assert_eq!(store.skills(one).unwrap(), Vec::new());
            assert_eq!(count(store, "skill_versions"), 1);
            assert_eq!(count(store, "skill_files"), 1);
            assert!(
                store
                    .skill_version(&shared_version.hash())
                    .unwrap()
                    .is_some()
            );
        });
    }
}
