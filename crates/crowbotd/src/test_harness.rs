//! Tools and prompt sections for the daemon's own tests, in debug builds
//! only, and only when `CROWBOT_TEST_HARNESS` names a folder. A release build
//! holds none of this. They stand for the harness's real tools and sections
//! (memory, skills, routines) until those exist, one per effect, and read
//! and write files of that folder so that a test sees what ran.

use std::io::Write;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;

use crate::digest::sha256_hex;
use crate::prompt::{Section, SectionState, Sections};
use crate::tools::{
    ALL_KINDS, Applied, Mutation, Registry, SessionKind, Tool, ToolContext, ToolFuture, TurnPermit,
    effect,
};

const VARIABLE: &str = "CROWBOT_TEST_HARNESS";

pub fn folder() -> Option<PathBuf> {
    std::env::var_os(VARIABLE)
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
}

pub fn add_tools(registry: &mut Registry, folder: &Path) {
    registry.add(ProbeRead);
    registry.add(ProbeState(folder.join("state.log")));
    registry.add(ProbeExternal(folder.join("external.log")));
    registry.add(ProbeReview);
    registry.add(ProbeBig);
}

/// Two sections whose text is the file `section-<id>.txt` of the folder,
/// registered out of the order of their ranks.
pub fn add_sections(sections: &mut Sections, folder: &Path) {
    sections.register(FileSection {
        id: "test-b",
        rank: 20,
        file: folder.join("section-b.txt"),
    });
    sections.register(FileSection {
        id: "test-a",
        rank: 10,
        file: folder.join("section-a.txt"),
    });
}

struct FileSection {
    id: &'static str,
    rank: u32,
    file: PathBuf,
}

impl Section for FileSection {
    fn id(&self) -> &'static str {
        self.id
    }

    fn rank(&self) -> u32 {
        self.rank
    }

    fn current(&self, _bot_id: i64) -> Option<SectionState> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        Some(SectionState {
            version: sha256_hex(text.as_bytes()),
            text,
        })
    }

    fn difference(&self, _bot_id: i64, _seen: Option<&str>, now: Option<&SectionState>) -> String {
        match now {
            Some(now) => format!("Section {} now reads: {}", self.id, now.text),
            None => format!("Section {} is gone.", self.id),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoInput {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TextInput {
    /// What to keep.
    text: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StateInput {
    /// What to keep.
    text: String,
    /// How long the tool works before it gives its change, so that a test
    /// can stop the turn in between.
    #[serde(default)]
    delay_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BigInput {
    /// How many bytes the output holds.
    bytes: usize,
}

fn who(context: &ToolContext) -> String {
    format!(
        "bot={} conversation={}",
        context.bot_id, context.conversation_id
    )
}

fn turn_of(permit: TurnPermit) -> String {
    format!("turn={}", permit.turn())
}

fn append(file: &Path, line: &str) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut file| writeln!(file, "{line}"))
        .map_err(|error| error.to_string())
}

struct ProbeRead;

impl Tool for ProbeRead {
    type Input = NoInput;
    type Effect = effect::Read;
    const NAME: &'static str = "probe_read";
    const DESCRIPTION: &'static str = "Tells which bot and conversation call it.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(&self, _permit: TurnPermit, context: ToolContext, _input: NoInput) -> ToolFuture {
        Box::pin(async move { Ok(who(&context)) })
    }
}

struct ProbeState(PathBuf);

impl Tool for ProbeState {
    type Input = StateInput;
    type Effect = effect::BotState;
    const NAME: &'static str = "probe_state";
    const DESCRIPTION: &'static str = "Keeps a text in the bot's own state.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    /// The line reaches the file only when the conversation applies the
    /// change, and names the turn the change was made in.
    fn run(
        &self,
        permit: TurnPermit,
        context: ToolContext,
        input: StateInput,
    ) -> ToolFuture<Mutation> {
        let file = self.0.clone();
        let line = format!("{} {} {}", who(&context), turn_of(permit), input.text);
        Box::pin(async move {
            if let Some(delay) = input.delay_ms {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            Ok(permit.mutation(move |_store| {
                append(&file, &line)
                    .map_err(|error| crate::store::StoreError::Io(std::io::Error::other(error)))?;
                Ok(Applied {
                    output: "kept".to_owned(),
                    event: None,
                })
            }))
        })
    }
}

struct ProbeExternal(PathBuf);

impl Tool for ProbeExternal {
    type Input = TextInput;
    type Effect = effect::External;
    const NAME: &'static str = "probe_external";
    const DESCRIPTION: &'static str = "Sends a text outside the bot's own state.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    /// It writes before its future is polled, so that the tests of an
    /// approval show the daemon calls `run` only once the call may run.
    fn run(&self, permit: TurnPermit, context: ToolContext, input: TextInput) -> ToolFuture {
        let sent = append(
            &self.0,
            &format!("{} {} {}", who(&context), turn_of(permit), input.text),
        );
        Box::pin(async move { sent.map(|()| "sent".to_owned()) })
    }
}

struct ProbeReview;

impl Tool for ProbeReview {
    type Input = NoInput;
    type Effect = effect::Read;
    const NAME: &'static str = "probe_review";
    const DESCRIPTION: &'static str = "A tool only a review may call.";
    const KINDS: &'static [SessionKind] = &[SessionKind::Review];

    fn run(&self, _permit: TurnPermit, _context: ToolContext, _input: NoInput) -> ToolFuture {
        Box::pin(async { Ok("reviewed".to_owned()) })
    }
}

struct ProbeBig;

impl Tool for ProbeBig {
    type Input = BigInput;
    type Effect = effect::Read;
    const NAME: &'static str = "probe_big";
    const DESCRIPTION: &'static str = "Returns an output of the given size.";
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(&self, _permit: TurnPermit, _context: ToolContext, input: BigInput) -> ToolFuture {
        Box::pin(async move { Ok("x".repeat(input.bytes)) })
    }
}
