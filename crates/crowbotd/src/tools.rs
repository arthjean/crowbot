//! The daemon's own tools, which an agent calls through the bridge. Each tool
//! has a typed input, whose JSON schema the agent reads, and one effect,
//! fixed in its type: what the tool may change decides whether a call asks a
//! person (`mcp.rs`).

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use crowbot_protocol::{TOOL_OUTPUT_BYTES, ToolEffect, cut_text};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// What a provider session is for. A kind may call only the tools that list it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Conversation,
    // The kinds below start with the routines, the reviews, and the
    // delegations of the harness; a tool lists them already.
    #[allow(dead_code)]
    Routine,
    #[allow(dead_code)]
    Review,
    #[allow(dead_code)]
    Delegated,
}

pub const ALL_KINDS: &[SessionKind] = &[
    SessionKind::Conversation,
    SessionKind::Routine,
    SessionKind::Review,
    SessionKind::Delegated,
];

/// Who calls a tool: the session's bot and conversation, which come from
/// the endpoint the call arrived on and from nowhere else.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub bot_id: i64,
    pub conversation_id: i64,
    pub kind: SessionKind,
    pub outputs: Outputs,
}

pub type ToolFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

/// A call whose arguments fit the tool's input, and that has not run: the
/// tool runs when the call is invoked, so that nothing of it runs before the
/// approval an `External` tool waits for (`mcp.rs`).
pub type ToolCall = Box<dyn FnOnce() -> ToolFuture + Send>;

/// A tool: its name, what it tells the model, its effect, the session kinds
/// that may call it, its input type, and what it does.
pub trait Tool: Send + Sync + 'static {
    type Input: DeserializeOwned + JsonSchema + Send;
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const EFFECT: ToolEffect;
    const KINDS: &'static [SessionKind];

    /// Runs the tool. The daemon calls it only once the call may run: after
    /// the approval of an `External` tool.
    fn run(&self, context: ToolContext, input: Self::Input) -> ToolFuture;
}

type Runner = Box<dyn Fn(ToolContext, Value) -> Result<ToolCall, String> + Send + Sync>;

/// One tool as the registry holds it, its input type erased.
pub struct ToolEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub effect: ToolEffect,
    kinds: &'static [SessionKind],
    schema: Value,
    runner: Runner,
}

impl ToolEntry {
    fn of<T: Tool>(tool: T) -> Self {
        let tool = Arc::new(tool);
        Self {
            name: T::NAME,
            description: T::DESCRIPTION,
            effect: T::EFFECT,
            kinds: T::KINDS,
            schema: input_schema::<T::Input>(),
            runner: Box::new(move |context, arguments| {
                let input = parse_input::<T::Input>(arguments)?;
                let tool = tool.clone();
                Ok(Box::new(move || tool.run(context, input)))
            }),
        }
    }

    pub fn allows(&self, kind: SessionKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// The tool as `tools/list` describes it. A tool that changes nothing
    /// says so, which lets an agent call it without asking (ChatGPT does,
    /// `docs/harness-providers.md`).
    pub fn listing(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.schema,
            "annotations": {
                "readOnlyHint": self.effect == ToolEffect::Read,
                "destructiveHint": false,
                "openWorldHint": self.effect == ToolEffect::External,
            },
        })
    }

    /// Checks the arguments against the input type and gives the call, which
    /// runs nothing until it is invoked. An argument that does not fit names
    /// the first field that is wrong, and the tool does not run.
    pub fn prepare(&self, context: ToolContext, arguments: Value) -> Result<ToolCall, String> {
        (self.runner)(context, arguments)
    }
}

/// The JSON schema of an input type, inline, with no meta-schema.
fn input_schema<T: JsonSchema>() -> Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|settings| {
        settings.meta_schema = None;
        settings.inline_subschemas = true;
    });
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut schema = schema.to_value();
    if let Some(fields) = schema.as_object_mut() {
        fields.remove("title");
        fields
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    schema
}

fn parse_input<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
    let arguments = match arguments {
        Value::Null => Value::Object(Map::new()),
        other => other,
    };
    serde_path_to_error::deserialize(arguments).map_err(|error| {
        let path = error.path().to_string();
        let inner = error.into_inner();
        if path == "." {
            format!("The arguments do not fit the tool's input: {inner}.")
        } else {
            format!("The argument `{path}` does not fit the tool's input: {inner}.")
        }
    })
}

/// Every tool of the daemon. The list is built once; a session sees the
/// tools its kind may call.
pub struct Registry {
    tools: BTreeMap<&'static str, ToolEntry>,
}

impl Registry {
    pub fn new() -> Self {
        let mut registry = Self {
            tools: BTreeMap::new(),
        };
        registry.add(ReadOutput);
        #[cfg(debug_assertions)]
        if let Some(folder) = crate::test_harness::folder() {
            crate::test_harness::add_tools(&mut registry, &folder);
        }
        registry
    }

    pub fn add<T: Tool>(&mut self, tool: T) {
        self.tools.insert(T::NAME, ToolEntry::of(tool));
    }

    pub fn get(&self, name: &str) -> Option<&ToolEntry> {
        self.tools.get(name)
    }

    pub fn effect(&self, name: &str) -> Option<ToolEffect> {
        self.get(name).map(|tool| tool.effect)
    }

    pub fn for_kind(&self, kind: SessionKind) -> impl Iterator<Item = &ToolEntry> {
        self.tools.values().filter(move |tool| tool.allows(kind))
    }
}

/// How many long outputs a session keeps for `read_output`, the most
/// recent ones.
const KEPT_OUTPUTS: usize = 8;

/// The long outputs of one session, which the agent reads past their
/// preview with `read_output`. They end with the session.
#[derive(Debug, Clone, Default)]
pub struct Outputs {
    inner: Arc<Mutex<OutputsInner>>,
}

#[derive(Debug, Default)]
struct OutputsInner {
    next: u64,
    kept: VecDeque<(String, String)>,
}

impl Outputs {
    /// The output as the agent receives it: whole when it fits in
    /// `TOOL_OUTPUT_BYTES`, and otherwise a preview that ends with the
    /// cursor of the rest.
    pub fn deliver(&self, output: String) -> String {
        if output.len() <= TOOL_OUTPUT_BYTES {
            return output;
        }
        let total = output.len();
        let id = {
            let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            inner.next += 1;
            let id = format!("output-{}", inner.next);
            inner.kept.push_back((id.clone(), output.clone()));
            while inner.kept.len() > KEPT_OUTPUTS {
                inner.kept.pop_front();
            }
            id
        };
        page(&output, &id, 0, total)
    }

    fn read(&self, cursor: &str) -> Result<String, String> {
        let (id, offset) = cursor
            .rsplit_once(':')
            .and_then(|(id, offset)| Some((id, offset.parse::<usize>().ok()?)))
            .ok_or_else(|| format!("`{cursor}` is not a cursor of read_output."))?;
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, output) = inner
            .kept
            .iter()
            .find(|(kept, _)| kept == id)
            .ok_or_else(|| format!("The output `{id}` is no longer kept."))?;
        if offset > output.len() || !output.is_char_boundary(offset) {
            return Err(format!("`{cursor}` does not point inside its output."));
        }
        Ok(page(output, id, offset, output.len()))
    }
}

/// The note that ends a page with more to read.
const MORE_NOTE_BYTES: usize = 256;

fn page(output: &str, id: &str, offset: usize, total: usize) -> String {
    let mut text = output[offset..].to_owned();
    if !cut_text(&mut text, TOOL_OUTPUT_BYTES - MORE_NOTE_BYTES) {
        return text;
    }
    let next = offset + text.len();
    text.push_str(&format!(
        "\n\n[Bytes {offset} to {next} of {total}. Call the tool read_output with the cursor \"{id}:{next}\" to read the rest.]"
    ));
    text
}

/// Reads a long output past its preview.
struct ReadOutput;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadOutputInput {
    /// The cursor that ends the previous page of the output.
    cursor: String,
}

impl Tool for ReadOutput {
    type Input = ReadOutputInput;
    const NAME: &'static str = "read_output";
    const DESCRIPTION: &'static str = "Reads the next part of a long output of a Crow Bot tool, from the cursor its last part ends with.";
    const EFFECT: ToolEffect = ToolEffect::Read;
    const KINDS: &'static [SessionKind] = ALL_KINDS;

    fn run(&self, context: ToolContext, input: ReadOutputInput) -> ToolFuture {
        let page = context.outputs.read(&input.cursor);
        Box::pin(async move { page })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct NoteInput {
        /// The note.
        text: String,
        #[serde(default)]
        #[allow(dead_code)]
        count: Option<u32>,
    }

    struct Note;

    impl Tool for Note {
        type Input = NoteInput;
        const NAME: &'static str = "note";
        const DESCRIPTION: &'static str = "Keeps a note.";
        const EFFECT: ToolEffect = ToolEffect::BotState;
        const KINDS: &'static [SessionKind] = &[SessionKind::Review];

        fn run(&self, _context: ToolContext, input: NoteInput) -> ToolFuture {
            Box::pin(async move { Ok(input.text) })
        }
    }

    fn context(kind: SessionKind) -> ToolContext {
        ToolContext {
            bot_id: 1,
            conversation_id: 2,
            kind,
            outputs: Outputs::default(),
        }
    }

    #[test]
    fn the_schema_comes_from_the_input_type() {
        let mut registry = Registry::new();
        registry.add(Note);
        let listing = registry.get("note").unwrap().listing();
        let schema = &listing["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["text"]["type"], "string");
        assert_eq!(schema["properties"]["text"]["description"], "The note.");
        assert_eq!(schema["required"], serde_json::json!(["text"]));
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema.get("$schema").is_none());
        assert_eq!(listing["annotations"]["readOnlyHint"], false);
        let read = registry.get("read_output").unwrap().listing();
        assert_eq!(read["annotations"]["readOnlyHint"], true);
    }

    #[test]
    fn each_kind_sees_the_tools_that_list_it() {
        let mut registry = Registry::new();
        registry.add(Note);
        let names = |kind| {
            registry
                .for_kind(kind)
                .map(|tool| tool.name)
                .filter(|name| ["note", "read_output"].contains(name))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(SessionKind::Conversation), ["read_output"]);
        assert_eq!(names(SessionKind::Routine), ["read_output"]);
        assert_eq!(names(SessionKind::Delegated), ["read_output"]);
        assert_eq!(names(SessionKind::Review), ["note", "read_output"]);
        assert_eq!(registry.effect("note"), Some(ToolEffect::BotState));
        assert_eq!(registry.effect("nothing"), None);
    }

    #[tokio::test]
    async fn arguments_that_do_not_fit_name_their_first_wrong_field_and_run_nothing() {
        let mut registry = Registry::new();
        registry.add(Note);
        let note = registry.get("note").unwrap();
        let start = |arguments: Value| note.prepare(context(SessionKind::Review), arguments);
        let wrong = start(serde_json::json!({ "text": 3 })).err().unwrap();
        assert!(wrong.contains("`text`"), "{wrong}");
        let missing = start(serde_json::json!({})).err().unwrap();
        assert!(missing.contains("missing field `text`"), "{missing}");
        let unknown = start(serde_json::json!({ "text": "a", "colour": 1 }))
            .err()
            .unwrap();
        assert!(unknown.contains("unknown field `colour`"), "{unknown}");
        let count = start(serde_json::json!({ "text": "a", "count": "two" }))
            .err()
            .unwrap();
        assert!(count.contains("`count`"), "{count}");
        let call = start(serde_json::json!({ "text": "kept" })).ok().unwrap();
        let ran = call().await;
        assert_eq!(ran, Ok("kept".to_owned()));
    }

    #[test]
    fn a_long_output_comes_as_pages_that_join_into_the_whole() {
        let outputs = Outputs::default();
        assert_eq!(outputs.deliver("short".to_owned()), "short");
        let whole: String = (0..TOOL_OUTPUT_BYTES * 2 + 1000)
            .map(|index| if index % 7 == 0 { 'é' } else { 'x' })
            .collect();
        let mut page = outputs.deliver(whole.clone());
        let mut joined = String::new();
        loop {
            assert!(page.len() <= TOOL_OUTPUT_BYTES, "{}", page.len());
            match page.rsplit_once("\n\n[Bytes ") {
                Some((text, note)) => {
                    joined.push_str(text);
                    let cursor = note.split('"').nth(1).unwrap();
                    page = outputs.read(cursor).unwrap();
                }
                None => {
                    joined.push_str(&page);
                    break;
                }
            }
        }
        assert_eq!(joined, whole);
        assert!(outputs.read("output-9:0").is_err());
        assert!(outputs.read("nonsense").is_err());
    }
}
