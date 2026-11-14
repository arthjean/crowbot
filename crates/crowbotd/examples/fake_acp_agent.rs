//! A scripted ACP agent for the daemon's integration tests.
//!
//! It speaks the Agent Client Protocol over stdio, one JSON-RPC message per line,
//! and reads what to do from the first word of each prompt:
//!
//! - `say <text>`: answer with the text.
//! - `stream <count> <interval ms>`: answer with `count` words, one per interval,
//!   and end as cancelled when a cancel arrives.
//! - `ask <title>`: request permission for a tool call, then report the outcome.
//! - `demand <title>`: the same, with a request that offers no allow-once option.
//! - `deaf`: never end the turn, and ignore the cancel and the end of stdin.
//! - `crash`: exit in the middle of the turn.
//! - `session`: report the session id and whether the session was resumed.
//! - `config`: report the system prompt, the text sent before the prompt, the
//!   model, and the working folder of the session.
//!
//! The command is the last part of a prompt: the parts before it are reported
//! by `config` as the preface.
//!
//! The agent offers the models `fast` and `smart` through a session config
//! option, and starts each process on `fast`.
//!
//! When `FAKE_AGENT_HEARTBEAT` names a file, the agent rewrites it with a growing
//! counter while it runs, so a test can tell whether the process is alive. When
//! `FAKE_AGENT_SYSTEM_PROMPT` is set, the agent advertises that it takes a
//! system prompt at session creation.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

const HEARTBEAT: &str = "FAKE_AGENT_HEARTBEAT";
const SYSTEM_PROMPT: &str = "FAKE_AGENT_SYSTEM_PROMPT";
const MODELS: [&str; 2] = ["fast", "smart"];
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(50);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Default)]
struct Agent {
    output: Mutex<()>,
    cancelled: AtomicBool,
    deaf: AtomicBool,
    resumed: AtomicBool,
    next_request: AtomicU64,
    answers: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    session: Mutex<Session>,
}

/// What the client set when it created or resumed the session.
#[derive(Default)]
struct Session {
    system_prompt: Option<String>,
    model: Option<String>,
    cwd: String,
}

fn texts(blocks: &Value) -> Vec<String> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str().map(str::to_owned))
        .collect()
}

impl Agent {
    fn send(&self, message: &Value) {
        let _guard = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        let mut stdout = std::io::stdout().lock();
        let written = writeln!(stdout, "{message}").and_then(|()| stdout.flush());
        if written.is_err() && !self.deaf.load(Ordering::SeqCst) {
            std::process::exit(0);
        }
    }

    fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn say(&self, session: &Value, text: &str) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text }
                }
            }
        }));
    }

    fn end(&self, id: &Value) {
        let reason = if self.cancelled.load(Ordering::SeqCst) {
            "cancelled"
        } else {
            "end_turn"
        };
        self.respond(id, json!({ "stopReason": reason }));
    }

    /// Asks the client for permission and returns the option it selected.
    fn ask(&self, session: &Value, title: &str, allow_once: bool) -> String {
        let request = 1000 + self.next_request.fetch_add(1, Ordering::SeqCst);
        let mut options = vec![
            json!({ "optionId": "always", "name": "Always allow", "kind": "allow_always" }),
            json!({ "optionId": "reject", "name": "Reject", "kind": "reject_once" }),
        ];
        if allow_once {
            options.insert(
                1,
                json!({ "optionId": "allow", "name": "Allow", "kind": "allow_once" }),
            );
        }
        let (answer, answered) = mpsc::channel();
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request, answer);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request,
            "method": "session/request_permission",
            "params": {
                "sessionId": session,
                "toolCall": { "toolCallId": format!("tool-{request}"), "title": title },
                "options": options
            }
        }));
        let Ok(response) = answered.recv() else {
            return "none".to_owned();
        };
        let outcome = &response["result"]["outcome"];
        match outcome["outcome"].as_str() {
            Some("selected") => outcome["optionId"].as_str().unwrap_or("none").to_owned(),
            Some(other) => other.to_owned(),
            None => "none".to_owned(),
        }
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps what the client set for the session, and answers with the
    /// session's config options.
    fn open_session(&self, params: &Value) -> Value {
        let mut session = self.session();
        session.cwd = params["cwd"].as_str().unwrap_or_default().to_owned();
        let system_prompt = texts(&params["systemPrompt"]["content"]);
        session.system_prompt = (!system_prompt.is_empty()).then(|| system_prompt.join("\n"));
        Self::config_options(&session)
    }

    fn config_options(session: &Session) -> Value {
        json!([{
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": session.model.as_deref().unwrap_or(MODELS[0]),
            "options": MODELS.map(|model| json!({ "value": model, "name": model })),
        }])
    }

    fn prompt(&self, id: &Value, params: &Value) {
        self.cancelled.store(false, Ordering::SeqCst);
        let session = &params["sessionId"];
        let mut parts = texts(&params["prompt"]);
        let text = parts.pop().unwrap_or_default();
        let text = text.as_str();
        let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
        match command {
            "say" => self.say(session, rest),
            "stream" => {
                let mut numbers = rest
                    .split(' ')
                    .map(|number| number.parse().unwrap_or(1_u64));
                let count = numbers.next().unwrap_or(1);
                let interval = Duration::from_millis(numbers.next().unwrap_or(1));
                for index in 0..count {
                    if self.cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    self.say(session, &format!("w{index} "));
                    std::thread::sleep(interval);
                }
            }
            "ask" | "demand" => {
                let outcome = self.ask(session, rest, command == "ask");
                self.say(session, &format!("outcome={outcome}"));
            }
            "deaf" => {
                self.deaf.store(true, Ordering::SeqCst);
                self.say(session, "deaf");
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            "crash" => std::process::exit(3),
            "session" => self.say(
                session,
                &format!(
                    "session={} resumed={}",
                    session.as_str().unwrap_or_default(),
                    self.resumed.load(Ordering::SeqCst)
                ),
            ),
            "config" => {
                let report = {
                    let config = self.session();
                    format!(
                        "system=[{}] preface=[{}] model={} cwd={}",
                        config.system_prompt.as_deref().unwrap_or_default(),
                        parts.join("|"),
                        config.model.as_deref().unwrap_or(MODELS[0]),
                        config.cwd
                    )
                };
                self.say(session, &report);
            }
            _ => self.say(session, text),
        }
        self.end(id);
    }

    fn request(self: &Arc<Self>, id: Value, method: &str, params: Value) {
        match method {
            "initialize" => self.respond(
                &id,
                json!({
                    "protocolVersion": params["protocolVersion"],
                    "agentCapabilities": {
                        "sessionCapabilities": if std::env::var_os(SYSTEM_PROMPT).is_some() {
                            json!({ "resume": {}, "systemPrompt": {} })
                        } else {
                            json!({ "resume": {} })
                        }
                    },
                    "authMethods": []
                }),
            ),
            "session/new" => self.respond(
                &id,
                json!({
                    "sessionId": format!("fake-session-{}-{}", std::process::id(), started_at()),
                    "configOptions": self.open_session(&params),
                }),
            ),
            "session/resume" => {
                self.resumed.store(true, Ordering::SeqCst);
                self.respond(&id, json!({ "configOptions": self.open_session(&params) }));
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or_default();
                if params["configId"] == "model" && MODELS.contains(&value) {
                    let mut session = self.session();
                    session.model = Some(value.to_owned());
                    self.respond(
                        &id,
                        json!({ "configOptions": Self::config_options(&session) }),
                    );
                } else {
                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32602,
                            "message": format!("Invalid value for config option model: {value}")
                        }
                    }));
                }
            }
            "session/prompt" => {
                let agent = self.clone();
                std::thread::spawn(move || agent.prompt(&id, &params));
            }
            _ => self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unknown method {method}") }
            })),
        }
    }

    fn receive(self: &Arc<Self>, mut message: Value) {
        let id = message["id"].take();
        let method = message["method"].as_str().map(str::to_owned);
        match (method, id) {
            (Some(method), Value::Null) => {
                if method == "session/cancel" && !self.deaf.load(Ordering::SeqCst) {
                    self.cancelled.store(true, Ordering::SeqCst);
                }
            }
            (Some(method), id) => self.request(id, &method, message["params"].take()),
            (None, id) => {
                let waiting = id.as_u64().and_then(|id| {
                    self.answers
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id)
                });
                if let Some(answer) = waiting {
                    answer.send(message).ok();
                }
            }
        }
    }
}

/// The time in nanoseconds: two processes never share a session id, even
/// when the system gives the second one the process id of the first.
fn started_at() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

fn main() {
    if let Some(path) = std::env::var_os(HEARTBEAT) {
        std::thread::spawn(move || {
            let mut beat = 0_u64;
            loop {
                std::fs::write(&path, beat.to_string()).ok();
                beat = beat.wrapping_add(1);
                std::thread::sleep(HEARTBEAT_INTERVAL);
            }
        });
    }
    let agent = Arc::new(Agent::default());
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Ok(message) = serde_json::from_str(&line) {
            agent.receive(message);
        }
    }
    while agent.deaf.load(Ordering::SeqCst) {
        std::thread::sleep(POLL_INTERVAL);
    }
}
