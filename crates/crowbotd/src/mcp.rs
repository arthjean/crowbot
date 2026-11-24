//! The daemon's MCP server: JSON-RPC 2.0 with `initialize`, `tools/list`,
//! and `tools/call`, written here with no MCP library. MCP types stay in this
//! module, as ACP types stay in the provider adapter.
//!
//! Each provider session gets an endpoint of its own (`SessionEndpoint`),
//! and the agent starts the daemon's own program with `--mcp-bridge` and that
//! endpoint as its MCP server (`bridge.rs`). The session a call belongs to,
//! and so its bot and conversation, comes from the endpoint the call arrives
//! on.

mod bridge;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crowbot_protocol::transport::{Listener, MessageReader, MessageWriter};
use crowbot_protocol::{APPROVAL_TARGET_BYTES, Endpoint, ToolEffect, cut_text};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub use bridge::{BRIDGE, run as run_bridge};

use crate::provider::acp::ApprovalAnswer;
use crate::tools::{Done, Mutation, Registry, ToolContext, TurnGate};

/// The name of the daemon's server, which agents put in front of its tools:
/// `mcp__crowbot__<tool>` for Claude, `mcp.crowbot.<tool>` for ChatGPT, and
/// `crowbot__<tool>` for Grok (`docs/harness-providers.md`).
pub const SERVER_NAME: &str = "crowbot";

/// The MCP versions this server speaks, the newest first.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INVALID_REQUEST: i64 = -32600;

/// What a call asks of the conversation it belongs to.
pub enum ToolSignal {
    /// An `External` tool waits for an approval, which the bot's permission
    /// level and a person give as for any other request, in the turn its
    /// permit was given in.
    Approval {
        turn: u64,
        /// The agent's id of the tool call, when the agent sent it with the
        /// call.
        tool_call_id: Option<String>,
        title: String,
        target: Option<String>,
        cut: bool,
        reply: oneshot::Sender<ApprovalAnswer>,
    },
    /// A change to the bot's own state, which the conversation applies only
    /// while the turn it was made in still runs with no stop asked.
    Apply {
        mutation: Mutation,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// A tool that this session may not call, that does not exist, or that
    /// was called while no turn ran.
    Refused { tool: String, message: String },
}

/// The command an agent runs as the daemon's MCP server for one session.
pub struct ServerCommand {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// The endpoint of one provider session. Dropping it ends the session's
/// tools: no new bridge connects, and a bridge still connected gets an error
/// for each request.
pub struct SessionEndpoint {
    endpoint: Endpoint,
    ended: Arc<AtomicBool>,
    accepting: JoinHandle<()>,
}

impl Drop for SessionEndpoint {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::SeqCst);
        self.accepting.abort();
    }
}

impl SessionEndpoint {
    pub fn open(
        registry: Arc<Registry>,
        context: ToolContext,
        gate: TurnGate,
        signals: mpsc::UnboundedSender<ToolSignal>,
    ) -> std::io::Result<Self> {
        let endpoint = session_endpoint();
        let mut listener = Listener::bind(&endpoint)?;
        let ended = Arc::new(AtomicBool::new(false));
        let session = Arc::new(Session {
            registry,
            context,
            gate,
            signals,
            ended: ended.clone(),
        });
        let accepting = tokio::spawn(async move {
            while let Ok((reader, writer)) = listener.accept().await {
                // A connection outlives the listener, so that a bridge whose
                // session ended is told so.
                tokio::spawn(serve(session.clone(), reader, writer));
            }
        });
        Ok(Self {
            endpoint,
            ended,
            accepting,
        })
    }

    pub fn command(&self) -> ServerCommand {
        ServerCommand {
            name: SERVER_NAME.to_owned(),
            program: own_program(),
            args: vec![BRIDGE.to_owned(), self.endpoint.address().to_owned()],
        }
    }
}

/// The program of the running daemon. On Linux it is named through `/proc`,
/// which still leads to it once a build replaced the file it started from.
fn own_program() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from(format!("/proc/{}/exe", std::process::id()))
    } else {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("crowbotd"))
    }
}

/// A name no other session takes, of this daemon or another.
fn endpoint_name() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::AtomicU64;
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let random = RandomState::new().hash_one((std::process::id(), count));
    format!("crowbot-mcp-{}-{count}-{random:016x}", std::process::id())
}

/// On Linux an abstract socket, which leaves no file; on Windows a pipe that
/// only the daemon's account may open; elsewhere a socket in a folder of the
/// user's temporary folder that only the user may enter.
fn session_endpoint() -> Endpoint {
    let name = endpoint_name();
    #[cfg(target_os = "linux")]
    {
        Endpoint::abstract_socket(&name)
    }
    #[cfg(windows)]
    {
        Endpoint::new(format!(r"\\.\pipe\{name}"))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // A short path: a socket path holds `SOCKET_PATH_LIMIT` bytes. The
        // listener makes the folder 0700 and the socket 0600. This side is
        // not verified (`AGENTS.md`, "Open decisions").
        // SAFETY: `geteuid` has no preconditions.
        let user = unsafe { libc::geteuid() };
        let short = name
            .rsplit_once('-')
            .map_or(name.as_str(), |(head, _)| head);
        Endpoint::new(format!("/tmp/crowbot-mcp-{user}/{short}.sock"))
    }
}

struct Session {
    registry: Arc<Registry>,
    context: ToolContext,
    /// Open while a turn of the conversation runs and no stop is asked.
    gate: TurnGate,
    signals: mpsc::UnboundedSender<ToolSignal>,
    ended: Arc<AtomicBool>,
}

/// Serves one bridge: each request is answered on its own, so that a call
/// that waits for a person does not hold the others.
async fn serve(session: Arc<Session>, mut reader: MessageReader, mut writer: MessageWriter) {
    let (answers, mut answered) = mpsc::unbounded_channel::<Value>();
    let writing = tokio::spawn(async move {
        while let Some(answer) = answered.recv().await {
            if writer.write(&answer).await.is_err() {
                break;
            }
        }
    });
    while let Ok(Some(message)) = reader.read::<Value>().await {
        let session = session.clone();
        let answers = answers.clone();
        tokio::spawn(async move {
            if let Some(answer) = answer(&session, message).await {
                answers.send(answer).ok();
            }
        });
    }
    drop(answers);
    writing.await.ok();
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn tool_result(id: Value, text: String, failed: bool) -> Value {
    result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": failed }),
    )
}

/// The answer to one message; none to a notification or an answer.
async fn answer(session: &Session, message: Value) -> Option<Value> {
    let Value::Object(mut message) = message else {
        return Some(error(
            Value::Null,
            INVALID_REQUEST,
            "A message is an object.",
        ));
    };
    let id = message.remove("id")?;
    let method = message.get("method").and_then(Value::as_str)?;
    if session.ended.load(Ordering::SeqCst) {
        return Some(error(
            id,
            INVALID_REQUEST,
            "The session of this server ended, so its tools no longer run.",
        ));
    }
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => result(id, initialized(&params)),
        "ping" => result(id, json!({})),
        "tools/list" => {
            let tools: Vec<Value> = session
                .registry
                .for_kind(session.context.kind)
                .map(crate::tools::ToolEntry::listing)
                .collect();
            result(id, json!({ "tools": tools }))
        }
        "tools/call" => call(session, id, params).await,
        _ => error(id, METHOD_NOT_FOUND, &format!("Unknown method {method}.")),
    })
}

fn initialized(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "title": "Crow Bot", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// The agent's id of the tool call, which Claude and ChatGPT send with an
/// MCP call (`docs/harness-providers.md`).
fn agent_call_id(params: &Value) -> Option<String> {
    let meta = params.get("_meta")?;
    ["claudecode/toolUseId", "callId"]
        .iter()
        .find_map(|key| meta.get(*key)?.as_str())
        .map(str::to_owned)
}

async fn call(session: &Session, id: Value, params: Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error(id, INVALID_PARAMS, "A tool call names its tool.");
    };
    let refuse = |message: String| {
        session
            .signals
            .send(ToolSignal::Refused {
                tool: name.to_owned(),
                message: message.clone(),
            })
            .ok();
        error(id.clone(), INVALID_PARAMS, &message)
    };
    let tool = session
        .registry
        .get(name)
        .filter(|tool| tool.allows(session.context.kind));
    let Some(tool) = tool else {
        return refuse(format!("This session has no tool named {name}."));
    };
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let shown_arguments = arguments.to_string();
    let call = match tool.prepare(session.context.clone(), arguments) {
        Ok(call) => call,
        Err(message) => return tool_result(id, message, true),
    };
    // Every call, of every effect, belongs to the turn that runs: none runs
    // between turns or once a stop is asked.
    let Some(permit) = session.gate.permit() else {
        return refuse(format!(
            "No turn of this conversation runs, or it is being stopped, so the tool {name} did not run."
        ));
    };
    if tool.effect == ToolEffect::External {
        let mut target = format!("{name} {shown_arguments}");
        let cut = cut_text(&mut target, APPROVAL_TARGET_BYTES);
        let (reply, answer) = oneshot::channel();
        let asked = session.signals.send(ToolSignal::Approval {
            turn: permit.turn(),
            tool_call_id: agent_call_id(&params),
            title: format!("Crow Bot tool {name}"),
            target: Some(target),
            cut,
            reply,
        });
        let answer = match asked {
            Ok(()) => answer.await.unwrap_or(ApprovalAnswer::Cancel),
            Err(_) => ApprovalAnswer::Cancel,
        };
        match answer {
            ApprovalAnswer::Allow => {}
            ApprovalAnswer::Decline => {
                return tool_result(
                    id,
                    "A person declined this call, so the tool did not run and nothing was changed."
                        .to_owned(),
                    true,
                );
            }
            ApprovalAnswer::Cancel => return tool_result(id, stopped(), true),
        }
        if session.ended.load(Ordering::SeqCst) {
            return error(id, INVALID_REQUEST, "The session of this server ended.");
        }
    }
    let output = match call(permit).await {
        Ok(Done::Output(output)) => Ok(output),
        Ok(Done::Mutation(mutation)) => apply(session, mutation).await,
        Err(message) => Err(message),
    };
    match output {
        Ok(output) => tool_result(id, session.context.outputs.deliver(output), false),
        Err(message) => tool_result(id, message, true),
    }
}

fn stopped() -> String {
    "The turn was stopped before this call ran, so nothing was changed.".to_owned()
}

/// Hands a change to the conversation, which applies it or refuses it.
async fn apply(session: &Session, mutation: Mutation) -> Result<String, String> {
    let (reply, applied) = oneshot::channel();
    if session
        .signals
        .send(ToolSignal::Apply { mutation, reply })
        .is_err()
    {
        return Err(stopped());
    }
    applied.await.unwrap_or_else(|_| Err(stopped()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Outputs, SessionKind};

    fn session(kind: SessionKind) -> (Session, mpsc::UnboundedReceiver<ToolSignal>) {
        let (signals, received) = mpsc::unbounded_channel();
        let session = Session {
            registry: Arc::new(Registry::new(&crate::store::SharedStore::in_memory())),
            context: ToolContext {
                bot_id: 1,
                conversation_id: 2,
                kind,
                outputs: Outputs::default(),
            },
            gate: TurnGate::default(),
            signals,
            ended: Arc::new(AtomicBool::new(false)),
        };
        (session, received)
    }

    #[tokio::test]
    async fn the_server_answers_initialize_ping_and_list_and_refuses_an_unknown_method() {
        let (session, _) = session(SessionKind::Conversation);
        let initialized = answer(
            &session,
            json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        )
        .await
        .unwrap();
        assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(initialized["result"]["serverInfo"]["name"], SERVER_NAME);
        let newer = answer(
            &session,
            json!({ "id": 1, "method": "initialize", "params": { "protocolVersion": "2099-01-01" } }),
        )
        .await
        .unwrap();
        assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(
            answer(
                &session,
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .await
            .is_none()
        );
        let listed = answer(&session, json!({ "id": "a", "method": "tools/list" }))
            .await
            .unwrap();
        assert_eq!(listed["id"], "a");
        assert!(
            listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "read_output")
        );
        let unknown = answer(&session, json!({ "id": 2, "method": "server/discover" }))
            .await
            .unwrap();
        assert_eq!(unknown["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_tool_is_refused_and_signalled() {
        let (session, mut signals) = session(SessionKind::Conversation);
        let refused = answer(
            &session,
            json!({ "id": 3, "method": "tools/call", "params": { "name": "nothing" } }),
        )
        .await
        .unwrap();
        assert_eq!(refused["error"]["code"], INVALID_PARAMS);
        assert!(matches!(
            signals.try_recv(),
            Ok(ToolSignal::Refused { tool, .. }) if tool == "nothing"
        ));
    }

    #[tokio::test]
    async fn a_call_runs_only_while_the_gate_is_open() {
        let (session, mut signals) = session(SessionKind::Conversation);
        let call = json!({ "id": 5, "method": "tools/call", "params": { "name": "read_output", "arguments": { "cursor": "output-1:0" } } });
        let refused = answer(&session, call.clone()).await.unwrap();
        assert_eq!(refused["error"]["code"], INVALID_PARAMS, "{refused}");
        assert!(matches!(
            signals.try_recv(),
            Ok(ToolSignal::Refused { tool, .. }) if tool == "read_output"
        ));
        session.gate.open(1);
        let ran = answer(&session, call).await.unwrap();
        // The tool ran: its own answer says the cursor names no output.
        assert_eq!(ran["result"]["isError"], true, "{ran}");
        assert!(signals.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_session_that_ended_answers_every_request_with_an_error() {
        let (session, _) = session(SessionKind::Conversation);
        session.ended.store(true, Ordering::SeqCst);
        let refused = answer(&session, json!({ "id": 4, "method": "tools/list" }))
            .await
            .unwrap();
        assert_eq!(refused["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn the_call_id_comes_from_the_meta_claude_or_chatgpt_sends() {
        assert_eq!(
            agent_call_id(&json!({ "_meta": { "claudecode/toolUseId": "toolu_1" } })).as_deref(),
            Some("toolu_1")
        );
        assert_eq!(
            agent_call_id(&json!({ "_meta": { "callId": "exec-1" } })).as_deref(),
            Some("exec-1")
        );
        assert_eq!(
            agent_call_id(&json!({ "_meta": { "progressToken": 1 } })),
            None
        );
    }
}
