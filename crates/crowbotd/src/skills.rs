//! What a bot knows how to do, as the daemon serves it: the skills index in
//! the system prompt, one line per active skill, frozen when a session
//! opens; the changes since in the note before a message; the read tools
//! `skills_list` and `skill_view`; and `skill_manage`, the one way a bot
//! writes a skill (decisions D5 and D6).
//!
//! The database holds every version (`store/skills.rs`). Before each session
//! the daemon writes the active ones under `<data>/skills/<bot>/<skill>/`,
//! read-only, and checks them by hash (`projection.rs`). A skill whose
//! projection cannot be written is left out of that session's index, and
//! an event names the cause.

pub mod format;
pub mod projection;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crowbot_protocol::{
    APPROVAL_TARGET_BYTES, SKILL_DOCUMENT, SKILL_INDEX_DESCRIPTION_CHARACTERS, SKILL_OPERATIONS,
    SkillAuthor, SkillChange, SkillChangeKind, SkillState,
};
use schemars::JsonSchema;
use serde::Deserialize;

use self::format::Version;
use crate::prompt::{Section, SectionState, Sections};
use crate::store::{Expected, SharedStore, SkillPlan, SkillRow, SkillWriter, StoreError};
use crate::tools::{
    ALL_KINDS, Applied, Mutation, Registry, SessionKind, Tool, ToolContext, ToolFuture, TurnPermit,
    effect,
};

/// The rank of the skills index: after the memory sections.
const SKILLS_RANK: u32 = 20;

/// The skills of every bot: the store, the folder of their projection, and
/// the skills each bot's last projection could not write.
pub struct Library {
    store: SharedStore,
    root: PathBuf,
    unavailable: Mutex<HashMap<i64, BTreeMap<String, String>>>,
    /// Held while a projection is written: two conversations of one bot
    /// never write its folder at once.
    projecting: Mutex<()>,
}

impl Library {
    pub fn new(store: SharedStore, root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            store,
            root,
            unavailable: Mutex::new(HashMap::new()),
            projecting: Mutex::new(()),
        })
    }

    /// The folder of a bot's projection.
    pub fn bot_folder(&self, bot_id: i64) -> PathBuf {
        self.root.join(bot_id.to_string())
    }

    /// The folder of one skill's projection.
    pub fn folder(&self, bot_id: i64, name: &str) -> PathBuf {
        self.bot_folder(bot_id).join(name)
    }

    /// Writes the projection of a bot's active skills from the database,
    /// each folder checked against the hashes of its files and written again
    /// when it differs; the skills that could not be written, with the cause.
    pub fn project(&self, bot_id: i64) -> BTreeMap<String, String> {
        let _projecting = self
            .projecting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let wanted = self.store.with(|store| {
            let mut wanted = BTreeMap::new();
            for skill in store.active_skills(bot_id)? {
                wanted.insert(skill.name, store.skill_version_files(&skill.version)?);
            }
            Ok::<_, StoreError>(wanted)
        });
        let failed = match wanted {
            Ok(wanted) => {
                let content = |hash: &str| {
                    self.store
                        .with(|store| store.skill_file(hash))
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("the file {hash} is not stored"))
                };
                projection::sync(&self.bot_folder(bot_id), &wanted, &content)
            }
            Err(error) => {
                eprintln!("[crowbotd] bot {bot_id}: cannot read its skills: {error}");
                BTreeMap::new()
            }
        };
        self.unavailable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(bot_id, failed.clone());
        failed
    }

    /// The skills the last projection of a bot could not write.
    fn unavailable(&self, bot_id: i64) -> BTreeMap<String, String> {
        self.unavailable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&bot_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Removes the projection of a bot that is deleted.
    pub fn forget(&self, bot_id: i64) {
        let _projecting = self
            .projecting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.unavailable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&bot_id);
        if let Err(error) = projection::remove(&self.bot_folder(bot_id)) {
            eprintln!("[crowbotd] bot {bot_id}: cannot remove its skills folder: {error}");
        }
    }

    /// The active skills a session of the bot is given: those whose
    /// projection is in place.
    fn indexed(&self, bot_id: i64) -> Result<Vec<SkillRow>, StoreError> {
        let unavailable = self.unavailable(bot_id);
        Ok(self
            .store
            .with(|store| store.active_skills(bot_id))?
            .into_iter()
            .filter(|skill| !unavailable.contains_key(&skill.name))
            .collect())
    }
}

pub fn add_section(sections: &mut Sections, library: &Arc<Library>) {
    sections.register(SkillsSection(library.clone()));
}

pub fn add_tools(registry: &mut Registry, library: &Arc<Library>) {
    registry.add(SkillsList(library.clone()));
    registry.add(SkillView(library.clone()));
    registry.add(SkillManage(library.clone()));
}

/// A description as one line of the index: on one line, unable to draw a
/// tag, and cut at `SKILL_INDEX_DESCRIPTION_CHARACTERS` at the last word
/// that fits, with an ellipsis. A description the scan flags is withheld.
pub fn index_description(description: &str) -> String {
    let findings = crate::scan::scan(description);
    if !findings.is_empty() {
        return format!(
            "[withheld: the description matched the scan for {}]",
            crate::scan::describe(&findings)
        );
    }
    let flat = crate::memory::flat(description);
    if flat.chars().count() <= SKILL_INDEX_DESCRIPTION_CHARACTERS {
        return flat;
    }
    let kept: String = flat
        .chars()
        .take(SKILL_INDEX_DESCRIPTION_CHARACTERS - 1)
        .collect();
    let cut = match kept.rfind(' ') {
        Some(space) if space >= kept.len() / 2 => &kept[..space],
        _ => kept.as_str(),
    };
    let trimmed = cut.trim_end_matches(|character: char| {
        character.is_whitespace() || ".,;:-(/".contains(character)
    });
    format!("{trimmed}\u{2026}")
}

/// The opening of the index, which tells the agent when to load a skill.
const INDEX_INSTRUCTION: &str = "Before you act on a task, read the skills below. When one matches the task, even in part, load it with the tool skill_view first and follow it. When a skill you loaded was wrong or missed a step, correct it with skill_manage against the version skill_view gave. When you finish a task that took several steps and no skill covers it, write one with skill_manage.";

/// The skills index of a bot's system prompt.
struct SkillsSection(Arc<Library>);

impl SkillsSection {
    fn lines(skills: &[SkillRow]) -> Vec<String> {
        skills
            .iter()
            .map(|skill| {
                format!(
                    "- {}: {}",
                    skill.name,
                    index_description(&skill.description)
                )
            })
            .collect()
    }
}

impl Section for SkillsSection {
    fn id(&self) -> &'static str {
        "skills"
    }

    fn rank(&self) -> u32 {
        SKILLS_RANK
    }

    /// The version is the last change of what the index shows, so that the
    /// note names a skill created or archived during a session once.
    fn current(&self, bot_id: i64) -> Option<SectionState> {
        let skills = self.0.indexed(bot_id).ok()?;
        if skills.is_empty() {
            return None;
        }
        let version = self
            .0
            .store
            .with(|store| store.skills_index_version(bot_id))
            .ok()??;
        Some(SectionState {
            version: version.to_string(),
            text: format!(
                "## Skills\n{INDEX_INSTRUCTION}\n{}",
                Self::lines(&skills).join("\n")
            ),
        })
    }

    fn difference(&self, bot_id: i64, seen: Option<&str>, _now: Option<&SectionState>) -> String {
        let Some(seen) = seen.and_then(|seen| seen.parse::<i64>().ok()) else {
            // The session started with no skill: every skill is new to it.
            let skills = self.0.indexed(bot_id).unwrap_or_default();
            return format!(
                "Your skills changed. {INDEX_INSTRUCTION}\n{}",
                Self::lines(&skills).join("\n")
            );
        };
        let changes = self
            .0
            .store
            .with(|store| store.skill_index_changes_since(bot_id, seen))
            .unwrap_or_default();
        let mut lines = vec!["Your skills changed:".to_owned()];
        for change in changes {
            let name = &change.name;
            let description = index_description(&change.description);
            lines.push(match change.kind {
                SkillChangeKind::Created | SkillChangeKind::Imported => {
                    format!("- New skill {name}: {description}")
                }
                SkillChangeKind::Restored => format!("- Skill {name} is back: {description}"),
                SkillChangeKind::Archived => {
                    format!("- Skill {name} was archived: it is no longer yours to use.")
                }
                _ => format!(
                    "- Skill {name} has a new version: load it again with skill_view before you use it."
                ),
            });
        }
        lines.join("\n")
    }

    /// A compaction may drop the index, and with it the instruction to load
    /// a skill: it comes again whole.
    fn renews(&self) -> bool {
        true
    }
}

/// Lists the bot's active skills.
struct SkillsList(Arc<Library>);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillsListInput {}

impl Tool for SkillsList {
    type Input = SkillsListInput;
    type Effect = effect::Read;
    const NAME: &'static str = "skills_list";
    const DESCRIPTION: &'static str = "Lists your skills: for each, its name, its whole description, and the hash of its active version. Load one with skill_view.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(
        &self,
        _permit: TurnPermit,
        context: ToolContext,
        _input: SkillsListInput,
    ) -> ToolFuture {
        let library = self.0.clone();
        Box::pin(async move {
            let skills = library
                .store
                .with(|store| store.active_skills(context.bot_id))
                .map_err(|error| error.to_string())?;
            if skills.is_empty() {
                return Ok("You have no skill yet. Write one with skill_manage once a task took several steps.".to_owned());
            }
            let lines: Vec<String> = skills
                .iter()
                .map(|skill| {
                    format!(
                        "- {} (version {}): {}",
                        skill.name,
                        skill.version,
                        crate::memory::flat(&skill.description)
                    )
                })
                .collect();
            Ok(lines.join("\n"))
        })
    }
}

/// Reads a skill.
struct SkillView(Arc<Library>);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillViewInput {
    /// The skill's name, as the index or skills_list gives it.
    name: String,
    /// A file of the skill to read instead of its SKILL.md, such as
    /// `references/api.md`.
    #[serde(default)]
    file: Option<String>,
}

impl Tool for SkillView {
    type Input = SkillViewInput;
    type Effect = effect::Read;
    const NAME: &'static str = "skill_view";
    const DESCRIPTION: &'static str = "Loads one of your skills: its SKILL.md, the list of its other files, the folder where its files are (read-only), and the hash of its version, which a correction with skill_manage names. Give `file` to read one of its other files.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(&self, _permit: TurnPermit, context: ToolContext, input: SkillViewInput) -> ToolFuture {
        let library = self.0.clone();
        Box::pin(async move { view(&library, context.bot_id, &input) })
    }
}

fn not_found(library: &Library, bot_id: i64, name: &str) -> String {
    let names: Vec<String> = library
        .store
        .with(|store| store.active_skills(bot_id))
        .unwrap_or_default()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    let archived = library
        .store
        .with(|store| store.skill(bot_id, name))
        .ok()
        .flatten()
        .is_some_and(|skill| skill.state == SkillState::Archived);
    let why = if archived {
        format!("The skill {name} is archived.")
    } else {
        format!("No skill is named {name}.")
    };
    if names.is_empty() {
        format!("{why} You have no active skill.")
    } else {
        format!("{why} Your skills are: {}.", names.join(", "))
    }
}

fn view(library: &Library, bot_id: i64, input: &SkillViewInput) -> Result<String, String> {
    let found = library
        .store
        .with(|store| {
            let Some(skill) = store.skill(bot_id, &input.name)? else {
                return Ok(None);
            };
            if skill.state != SkillState::Active {
                return Ok(None);
            }
            let version = store.skill_version(&skill.version)?;
            Ok::<_, StoreError>(version.map(|version| (skill, version)))
        })
        .map_err(|error| error.to_string())?;
    let Some((skill, version)) = found else {
        return Err(not_found(library, bot_id, &input.name));
    };
    let folder = library.folder(bot_id, &skill.name);
    if let Some(path) = &input.file {
        let Some(content) = version.file(path) else {
            let files: Vec<&str> = version.files().keys().map(String::as_str).collect();
            return Err(format!(
                "The skill {} holds no file {path}. Its files are: {}.",
                skill.name,
                files.join(", ")
            ));
        };
        return Ok(match std::str::from_utf8(content) {
            Ok(text) => format!(
                "File {path} of the skill {} (version {}):\n\n{text}",
                skill.name, skill.version
            ),
            Err(_) => format!(
                "File {path} of the skill {} is binary, {} bytes: read it at {}.",
                skill.name,
                content.len(),
                folder.join(path).display()
            ),
        });
    }
    let others: Vec<&str> = version
        .files()
        .keys()
        .map(String::as_str)
        .filter(|path| *path != SKILL_DOCUMENT)
        .collect();
    let files = if others.is_empty() {
        "none".to_owned()
    } else {
        others.join(", ")
    };
    let place = match library.unavailable(bot_id).get(&skill.name) {
        Some(cause) => format!("not written this session ({cause})"),
        None => format!("{} (read-only)", folder.display()),
    };
    Ok(format!(
        "Skill {} (version {}).\nFolder: {place}\nOther files: {files}\n\n{}",
        skill.name,
        skill.version,
        version.document().unwrap_or_default()
    ))
}

/// Writes the bot's skills.
struct SkillManage(Arc<Library>);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillManageInput {
    /// The operations, applied in order: all of them, or none.
    operations: Vec<SkillOperationInput>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum SkillOperationInput {
    /// Writes a new skill.
    Create {
        /// The skill's name: lower-case letters, digits, dots, hyphens, and
        /// underscores, 64 characters at most.
        name: String,
        /// The SKILL.md: a front matter with `name` (the same name) and
        /// `description` between two `---` lines, then the instructions.
        content: String,
    },
    /// Replaces the SKILL.md of a skill.
    Correct {
        name: String,
        /// The version skill_view gave. Leave it out for a skill this call
        /// creates.
        #[serde(default)]
        version: Option<String>,
        content: String,
    },
    /// Writes a file under `references`, `templates`, `scripts`, or
    /// `assets`.
    WriteFile {
        name: String,
        #[serde(default)]
        version: Option<String>,
        /// The file's path in the skill, such as `scripts/check.sh`.
        path: String,
        content: String,
    },
    /// Removes a file of a skill.
    RemoveFile {
        name: String,
        #[serde(default)]
        version: Option<String>,
        path: String,
    },
    /// Sets a skill aside: it leaves the index, and its versions stay.
    Archive { name: String },
}

impl SkillOperationInput {
    fn name(&self) -> &str {
        match self {
            Self::Create { name, .. }
            | Self::Correct { name, .. }
            | Self::WriteFile { name, .. }
            | Self::RemoveFile { name, .. }
            | Self::Archive { name } => name,
        }
    }
}

/// One skill as a call changes it, before it is applied.
struct Working {
    base: Option<SkillRow>,
    old: Option<Version>,
    version: Version,
    touched: bool,
    archive: bool,
}

/// A call checked against the skills as they are: what it would store, and
/// the text a person reads when the scan finds something dangerous in it.
struct Prepared {
    plans: Vec<SkillPlan>,
    review: Option<String>,
}

fn refused(index: usize, message: impl std::fmt::Display) -> String {
    format!(
        "Operation {} was refused, so nothing was changed: {message}",
        index + 1
    )
}

fn prepare(
    library: &Library,
    bot_id: i64,
    operations: &[SkillOperationInput],
) -> Result<Prepared, String> {
    if operations.is_empty() {
        return Err("A change of skills needs at least one operation.".to_owned());
    }
    if operations.len() > SKILL_OPERATIONS {
        return Err(format!(
            "A change of skills holds {SKILL_OPERATIONS} operations at most."
        ));
    }
    let mut order: Vec<String> = Vec::new();
    let mut working: BTreeMap<String, Working> = BTreeMap::new();
    let read = |name: &str| {
        library
            .store
            .with(|store| {
                let Some(skill) = store.skill(bot_id, name)? else {
                    return Ok(None);
                };
                let version = store.skill_version(&skill.version)?.unwrap_or_default();
                Ok::<_, StoreError>(Some((skill, version)))
            })
            .map_err(|error| error.to_string())
    };
    for (index, operation) in operations.iter().enumerate() {
        let name = operation.name();
        format::check_name(name).map_err(|error| refused(index, error))?;
        if let SkillOperationInput::Create { content, .. } = operation {
            if working.contains_key(name) || read(name)?.is_some() {
                return Err(refused(
                    index,
                    format!(
                        "a skill named {name} exists already: correct it instead, against the version skill_view gives"
                    ),
                ));
            }
            order.push(name.to_owned());
            working.insert(
                name.to_owned(),
                Working {
                    base: None,
                    old: None,
                    version: Version::of_document(content),
                    touched: true,
                    archive: false,
                },
            );
            continue;
        }
        if !working.contains_key(name) {
            let Some((skill, version)) = read(name)? else {
                return Err(refused(index, not_found(library, bot_id, name)));
            };
            if skill.state == SkillState::Archived {
                return Err(refused(
                    index,
                    format!("the skill {name} is archived; a person can restore it"),
                ));
            }
            order.push(name.to_owned());
            working.insert(
                name.to_owned(),
                Working {
                    base: Some(skill),
                    old: Some(version.clone()),
                    version,
                    touched: false,
                    archive: false,
                },
            );
        }
        let Some(entry) = working.get_mut(name) else {
            continue;
        };
        let given = match operation {
            SkillOperationInput::Correct { version, .. }
            | SkillOperationInput::WriteFile { version, .. }
            | SkillOperationInput::RemoveFile { version, .. } => Some(version.as_deref()),
            _ => None,
        };
        if let Some(given) = given {
            match (&entry.base, given) {
                (None, Some(_)) => {
                    return Err(refused(
                        index,
                        format!("the skill {name} is created by this call, so no version names it"),
                    ));
                }
                (Some(_), None) => {
                    return Err(refused(
                        index,
                        format!("name the version of {name} that skill_view gave you"),
                    ));
                }
                (Some(base), Some(given)) if given != base.version => {
                    return Err(refused(
                        index,
                        format!(
                            "the skill {name} is at version {}, not {given}: read it again with skill_view, then make your change against that version",
                            base.version
                        ),
                    ));
                }
                _ => {}
            }
        }
        match operation {
            SkillOperationInput::Create { .. } => {}
            SkillOperationInput::Correct { content, .. } => {
                entry
                    .version
                    .put(SKILL_DOCUMENT, content.as_bytes().to_vec());
                entry.touched = true;
            }
            SkillOperationInput::WriteFile { path, content, .. } => {
                if path == SKILL_DOCUMENT {
                    return Err(refused(index, "write the SKILL.md with `correct`"));
                }
                format::check_path(path).map_err(|error| refused(index, error))?;
                entry.version.put(path, content.as_bytes().to_vec());
                entry.touched = true;
            }
            SkillOperationInput::RemoveFile { path, .. } => {
                if path == SKILL_DOCUMENT {
                    return Err(refused(index, "a skill keeps its SKILL.md"));
                }
                if !entry.version.remove(path) {
                    return Err(refused(
                        index,
                        format!("the skill {name} holds no file {path}"),
                    ));
                }
                entry.touched = true;
            }
            SkillOperationInput::Archive { .. } => {
                if entry.base.is_none() {
                    return Err(refused(
                        index,
                        format!(
                            "the skill {name} is created by this call, so it is not archived in it"
                        ),
                    ));
                }
                if entry.base.as_ref().is_some_and(|base| base.pinned) {
                    return Err(refused(
                        index,
                        format!("the skill {name} is pinned, so only a person can archive it"),
                    ));
                }
                entry.archive = true;
            }
        }
    }
    let mut plans = Vec::new();
    let mut flagged = Vec::new();
    for name in &order {
        let Some(entry) = working.remove(name) else {
            continue;
        };
        let expected = match &entry.base {
            Some(base) => Expected::Version(base.version.clone()),
            None => Expected::New,
        };
        let mut plan = SkillPlan::new(name, expected);
        if entry.touched {
            format::check(name, &entry.version).map_err(|error| {
                format!("The skill {name} was not written, so nothing was changed: {error}")
            })?;
            let findings = format::findings(&entry.version);
            if format::dangerous(&findings) {
                flagged.push(format!(
                    "Skill {name}{}:\n{}\n\nDifference:\n{}",
                    if entry.base.is_some() { "" } else { " (new)" },
                    format::describe_findings(&findings),
                    format::difference(entry.old.as_ref(), &entry.version)
                ));
            }
            let kind = if entry.base.is_some() {
                SkillChangeKind::Corrected
            } else {
                SkillChangeKind::Created
            };
            plan.version = Some((entry.version, kind));
        }
        if entry.archive {
            plan.state = Some(SkillState::Archived);
        }
        plans.push(plan);
    }
    let review = if flagged.is_empty() {
        None
    } else {
        let text = format!(
            "The scan found something dangerous in the skills this call writes. The change is made only if a person allows it.\n\n{}",
            flagged.join("\n\n")
        );
        if text.len() > APPROVAL_TARGET_BYTES {
            return Err(format!(
                "The scan found something dangerous in this change, and the change is too long to show a person in full ({APPROVAL_TARGET_BYTES} bytes at most), so nothing was changed. Write the flagged file in a smaller call, or leave the flagged text out."
            ));
        }
        Some(text)
    };
    Ok(Prepared { plans, review })
}

/// What a change did, as the agent reads it.
pub fn report(changes: &[SkillChange]) -> String {
    if changes.is_empty() {
        return "Nothing changed: the skills already held this.".to_owned();
    }
    changes
        .iter()
        .map(|change| {
            let name = &change.name;
            let new = change.new_hash.as_deref().unwrap_or_default();
            match change.kind {
                SkillChangeKind::Created => format!("Created skill {name}, version {new}."),
                SkillChangeKind::Archived => format!("Archived skill {name}."),
                SkillChangeKind::Restored => format!("Restored skill {name}, version {new}."),
                SkillChangeKind::Pinned => format!("Pinned skill {name}."),
                SkillChangeKind::Unpinned => format!("Unpinned skill {name}."),
                _ => format!(
                    "Skill {name} is now at version {new} (was {}).",
                    change.old_hash.as_deref().unwrap_or_default()
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Tool for SkillManage {
    type Input = SkillManageInput;
    type Effect = effect::BotState;
    const NAME: &'static str = "skill_manage";
    const DESCRIPTION: &'static str = "Creates, corrects, or archives your skills in one call: every operation applies, or none does. A skill is how you did a task, written for the next time: a SKILL.md with a front matter (`name`, the skill's name, and `description`, one sentence that says when to use it, between two `---` lines) and then the steps, and files under references, templates, scripts, or assets. Facts about the user or the project belong in memory, not in a skill. Each change of a skill you did not create in this call names the version skill_view gave; a skill that changed since is refused, and you read it again. A skill the scan finds dangerous is written only once a person allows it. Archiving keeps every version; a pinned skill is archived by a person only.";
    const KINDS: &'static [SessionKind] = &[
        SessionKind::Conversation,
        SessionKind::Routine,
        SessionKind::Review,
    ];

    fn run(
        &self,
        permit: TurnPermit,
        context: ToolContext,
        input: SkillManageInput,
    ) -> ToolFuture<Mutation> {
        let prepared = prepare(&self.0, context.bot_id, &input.operations);
        Box::pin(async move {
            let Prepared { plans, review } = prepared?;
            let author = if context.kind == SessionKind::Review {
                SkillAuthor::Review
            } else {
                SkillAuthor::Bot
            };
            let writer = SkillWriter {
                author,
                bot_id: context.bot_id,
                conversation_id: Some(context.conversation_id),
            };
            let mutation = permit.mutation(move |store| {
                let changes = store.change_skills(&writer, &plans)?;
                Ok(Applied {
                    output: report(&changes),
                    event: crate::store::skill_changed(author, changes),
                })
            });
            Ok(match review {
                Some(target) => mutation.for_person("Crow Bot tool skill_manage", target),
                None => mutation,
            })
        })
    }
}

/// Reads a skill from a folder of this computer, for an import: its
/// `SKILL.md` and the files under the four folders. A hidden entry is left
/// out; a link, or any other file, refuses the import.
pub fn read_folder(folder: &Path) -> Result<(String, Version), String> {
    let failed =
        |path: &Path, error: std::io::Error| format!("cannot read {}: {error}", path.display());
    let metadata = folder.metadata().map_err(|error| failed(folder, error))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a folder.", folder.display()));
    }
    let mut files = BTreeMap::new();
    let mut folders = vec![(folder.to_owned(), String::new())];
    while let Some((current, prefix)) = folders.pop() {
        for entry in std::fs::read_dir(&current).map_err(|error| failed(&current, error))? {
            let entry = entry.map_err(|error| failed(&current, error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{prefix}{name}");
            let kind = entry
                .file_type()
                .map_err(|error| failed(&entry.path(), error))?;
            if kind.is_dir() {
                folders.push((entry.path(), format!("{path}/")));
            } else if kind.is_file() {
                format::check_path(&path)?;
                let size = entry
                    .metadata()
                    .map_err(|error| failed(&entry.path(), error))?
                    .len();
                if size > crowbot_protocol::SKILL_FILE_BYTES as u64 {
                    return Err(format!(
                        "The file `{path}` is larger than {} bytes (1 MiB), the most a file of a skill holds.",
                        crowbot_protocol::SKILL_FILE_BYTES
                    ));
                }
                if files.len() == crowbot_protocol::SKILL_FILES {
                    return Err(format!(
                        "{} holds more than {} files, the most a skill holds, its SKILL.md included.",
                        folder.display(),
                        crowbot_protocol::SKILL_FILES
                    ));
                }
                let content =
                    std::fs::read(entry.path()).map_err(|error| failed(&entry.path(), error))?;
                files.insert(path, content);
            } else {
                return Err(format!(
                    "`{path}` is a link or a special file, which a skill does not hold."
                ));
            }
        }
    }
    let version = Version::new(files);
    let document = version
        .document()
        .ok_or_else(|| format!("{} holds no {SKILL_DOCUMENT} in UTF-8.", folder.display()))?;
    let front = format::front_matter(document)?;
    format::check(&front.name, &version)?;
    Ok((front.name, version))
}

#[cfg(test)]
mod tests {
    use crowbot_protocol::{Avatar, Event, PermissionLevel};

    use super::*;
    use crate::store::NewBot;
    use crate::tools::Outputs;

    #[tokio::test]
    async fn a_review_session_archives_a_skill_through_the_tool() {
        let store = SharedStore::in_memory();
        let row = store
            .with(|store| {
                store.create_bot(&NewBot {
                    name: "Reviewed".to_owned(),
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
            })
            .unwrap();
        let scratch = crowbot_protocol::scratch::Scratch::new("crowbotd-skills");
        let library = Library::new(store.clone(), scratch.join("skills"));
        let mut registry = Registry::new(&store, &library);
        registry.add(SkillManage(library.clone()));
        let tool = registry.get("skill_manage").unwrap();
        let run = async |kind, operations: serde_json::Value| {
            let context = ToolContext {
                bot_id: row.bot.id,
                conversation_id: row.current_conversation_id,
                kind,
                outputs: Outputs::default(),
            };
            let call = tool
                .prepare(context, serde_json::json!({ "operations": operations }))
                .ok()
                .unwrap();
            let Ok(crate::tools::Done::Mutation(mutation)) = call(TurnPermit::for_test(1)).await
            else {
                panic!("skill_manage returns a mutation");
            };
            store
                .with(|store| store.apply_mutation(row.current_conversation_id, mutation))
                .ok()
                .unwrap()
        };
        let document = "---\nname: tidy\ndescription: Tidies.\n---\nSteps.\n";
        run(
            SessionKind::Conversation,
            serde_json::json!([{ "op": "create", "name": "tidy", "content": document }]),
        )
        .await;
        let applied = run(
            SessionKind::Review,
            serde_json::json!([{ "op": "archive", "name": "tidy" }]),
        )
        .await;
        assert!(
            matches!(
                &applied.event,
                Some(Event::SkillChanged { author: SkillAuthor::Review, changes })
                    if changes[0].kind == SkillChangeKind::Archived
            ),
            "{}",
            applied.output
        );
        let skill = store
            .with(|store| store.skill(row.bot.id, "tidy"))
            .unwrap()
            .unwrap();
        assert_eq!(skill.state, SkillState::Archived);
    }

    #[test]
    fn a_skill_created_by_a_call_is_not_archived_in_it() {
        let store = SharedStore::in_memory();
        let scratch = crowbot_protocol::scratch::Scratch::new("crowbotd-skills");
        let library = Library::new(store.clone(), scratch.join("skills"));
        let operations: Vec<SkillOperationInput> = serde_json::from_value(serde_json::json!([
            { "op": "create", "name": "tidy", "content": "---\nname: tidy\ndescription: Tidies.\n---\nSteps.\n" },
            { "op": "archive", "name": "tidy" }
        ]))
        .unwrap();
        let Err(error) = prepare(&library, 1, &operations) else {
            panic!("an archive of a skill the call creates is refused");
        };
        assert!(error.contains("Operation 2"), "{error}");
        assert!(error.contains("created by this call"), "{error}");
    }

    #[test]
    fn an_import_stops_at_the_most_files_a_skill_holds() {
        let scratch = crowbot_protocol::scratch::Scratch::new("crowbotd-import");
        let folder = scratch.join("many");
        std::fs::create_dir_all(folder.join("references")).unwrap();
        std::fs::write(
            folder.join(SKILL_DOCUMENT),
            "---\nname: many\ndescription: Many.\n---\nSteps.\n",
        )
        .unwrap();
        for index in 0..crowbot_protocol::SKILL_FILES {
            std::fs::write(folder.join("references").join(format!("{index}.md")), "x").unwrap();
        }
        let error = read_folder(&folder).unwrap_err();
        assert!(error.contains("more than 256 files"), "{error}");
    }

    #[test]
    fn a_description_in_the_index_is_cut_at_its_limit() {
        assert_eq!(index_description("Deploys the app."), "Deploys the app.");
        let long = "Deploys the application to the production cluster, then checks the health of every service.";
        let cut = index_description(long);
        assert!(
            cut.chars().count() <= SKILL_INDEX_DESCRIPTION_CHARACTERS,
            "{cut}"
        );
        assert!(cut.ends_with('\u{2026}'), "{cut}");
        assert!(long.starts_with(cut.trim_end_matches('\u{2026}')), "{cut}");
        assert_eq!(
            index_description("Two\nlines <tag>"),
            "Two lines \u{2039}tag\u{203a}"
        );
        let unbroken = "a".repeat(80);
        assert_eq!(
            index_description(&unbroken).chars().count(),
            SKILL_INDEX_DESCRIPTION_CHARACTERS
        );
    }
}
