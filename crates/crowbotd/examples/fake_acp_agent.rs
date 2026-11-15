//! A scripted ACP agent for the daemon's integration tests.
//!
//! It speaks the Agent Client Protocol over stdio, one JSON-RPC message per line,
//! and reads what to do from the first word of each prompt:
//!
//! - `say <text>`: answer with the text.
//! - `stream <count> <interval ms>`: answer with `count` words, one per interval,
//!   and end as cancelled when a cancel arrives.
//! - `ask <title>`: request permission for a tool call that runs the title as a
//!   command, then report the outcome.
//! - `demand <title>`: the same, with a request that offers no allow-once option.
//! - `askfile <path>`: the same as `ask`, for a tool call titled `Edit <path>`
//!   that edits the file at that path.
//! - `asklong <bytes>`: the same as `ask`, for a command of that many `x` bytes.
//! - `work`: think, read `notes.txt` with a tool call that completes, run tests
//!   with a tool call that fails, then answer.
//! - `hang <title>`: start a tool call that runs the title as a command, never
//!   complete it, and end as cancelled when a cancel arrives.
//! - `ponder <text>`: think the text, send no answer, and end as cancelled when
//!   a cancel arrives.
//! - `bigout <bytes>`: complete one tool call with an output of that many `x`
//!   bytes, then answer `done`.
//! - `fail auth`: fail the turn as a provider that needs a sign-in.
//! - `fail limit`: fail the turn as a provider whose usage limit is reached.
//! - `fail other`: fail the turn with an error kind that has no known cause.
//! - `sample <interval ms>`: answer with a Markdown text that holds every kind
//!   of block, one word per interval, and end as cancelled when a cancel arrives.
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
//!
//! When `FAKE_AGENT_AUTH` names a file, the agent reports a sign-in: it is
//! signed in when the file holds `authenticated`, and signed out otherwise. It
//! then offers one sign-in method of the `terminal` kind, which runs this
//! program with `--sign-in`. That run reads how to behave from the file of the
//! same name with the extension `mode`: `wait` never ends and starts a child
//! that never ends either, `guest` signs in at once and leaves behind a child
//! that goes on for a few seconds (as a sign-in leaves the browser it opened),
//! `fail` exits with an error, a number waits that many milliseconds before it
//! signs in, and no file signs in at once. A run that waits rewrites a growing
//! counter in the files with the extensions `beat` (itself) and `child` (its
//! child).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HEARTBEAT: &str = "FAKE_AGENT_HEARTBEAT";
const SYSTEM_PROMPT: &str = "FAKE_AGENT_SYSTEM_PROMPT";
const AUTH: &str = "FAKE_AGENT_AUTH";
const SIGNED_IN: &str = "authenticated";
const MODELS: [&str; 2] = ["fast", "smart"];
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(50);
/// How long the process that a finished sign-in leaves behind goes on.
const GUEST_LIFETIME: Duration = Duration::from_secs(4);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

const SAMPLE: &str = "## Release notes

This build has **bold** text, *italic* text, ~~struck~~ text, `inline code`, and a [link](https://example.com/docs).

- First item
- Second item
- Third item

1. Step one
2. Step two

- [x] Done task
- [ ] Open task

> A quote that spans
> two lines.

| Name | Kind | Count |
| --- | --- | ---: |
| alpha | read | 1 |
| beta | edit | 22 |

---

```rust
fn main() {
    let words = [\"caw\", \"caw\"];
    for word in words {
        println!(\"{word}\");
    }
}
```

Done.";

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

    fn update(&self, session: &Value, update: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": update }
        }));
    }

    fn think(&self, session: &Value, text: &str) {
        self.update(
            session,
            json!({
                "sessionUpdate": "agent_thought_chunk",
                "content": { "type": "text", "text": text }
            }),
        );
    }

    fn start_tool(&self, session: &Value, mut call: Value) -> String {
        let id = format!(
            "call-{}",
            1000 + self.next_request.fetch_add(1, Ordering::SeqCst)
        );
        call["sessionUpdate"] = "tool_call".into();
        call["toolCallId"] = id.as_str().into();
        self.update(session, call);
        id
    }

    fn update_tool(&self, session: &Value, id: &str, status: &str, output: Option<&str>) {
        let mut update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": status
        });
        if let Some(output) = output {
            update["content"] = json!([{
                "type": "content",
                "content": { "type": "text", "text": output }
            }]);
        }
        self.update(session, update);
    }

    fn fail(&self, id: &Value, error: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error }));
    }

    fn wait_for_cancel(&self) {
        while !self.cancelled.load(Ordering::SeqCst) {
            std::thread::sleep(POLL_INTERVAL);
        }
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
    fn ask(&self, session: &Value, mut tool_call: Value, allow_once: bool) -> String {
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
        tool_call["toolCallId"] = format!("tool-{request}").into();
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request,
            "method": "session/request_permission",
            "params": {
                "sessionId": session,
                "toolCall": tool_call,
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
            "ask" | "demand" | "askfile" | "asklong" => {
                let tool_call = match command {
                    "askfile" => json!({
                        "title": format!("Edit {rest}"),
                        "kind": "edit",
                        "locations": [{ "path": rest }]
                    }),
                    "asklong" => json!({
                        "title": "Run a long command",
                        "kind": "execute",
                        "rawInput": { "command": "x".repeat(rest.parse().unwrap_or(1)) }
                    }),
                    _ => json!({
                        "title": rest,
                        "kind": "execute",
                        "rawInput": { "command": rest }
                    }),
                };
                let outcome = self.ask(session, tool_call, command != "demand");
                self.say(session, &format!("outcome={outcome}"));
            }
            "work" => {
                self.think(session, "Looking at the files.");
                let notes = format!("{}/notes.txt", self.session().cwd);
                let read = self.start_tool(
                    session,
                    json!({
                        "title": "Read notes.txt",
                        "kind": "read",
                        "status": "pending",
                        "rawInput": { "file_path": notes }
                    }),
                );
                self.update_tool(session, &read, "in_progress", None);
                self.update_tool(session, &read, "completed", Some("line one\nline two"));
                let tests = self.start_tool(
                    session,
                    json!({
                        "title": "Run tests",
                        "kind": "execute",
                        "status": "in_progress",
                        "rawInput": { "command": "cargo test" }
                    }),
                );
                self.update_tool(session, &tests, "failed", Some("error: 1 test failed"));
                self.say(session, "Done with **two** steps.");
            }
            "hang" => {
                self.start_tool(
                    session,
                    json!({
                        "title": rest,
                        "kind": "execute",
                        "status": "in_progress",
                        "rawInput": { "command": rest }
                    }),
                );
                self.wait_for_cancel();
            }
            "ponder" => {
                self.think(session, rest);
                self.wait_for_cancel();
            }
            "bigout" => {
                let bytes = rest.parse().unwrap_or(1);
                let call = self.start_tool(
                    session,
                    json!({
                        "title": "Print output",
                        "kind": "execute",
                        "status": "in_progress",
                        "rawInput": { "command": format!("print {bytes}") }
                    }),
                );
                self.update_tool(session, &call, "completed", Some(&"x".repeat(bytes)));
                self.say(session, "done");
            }
            "fail" => {
                let error = match rest {
                    "auth" => json!({ "code": -32000, "message": "Authentication required" }),
                    "limit" => json!({
                        "code": -32603,
                        "message": "Internal error",
                        "data": { "errorKind": "rate_limit" }
                    }),
                    _ => json!({
                        "code": -32603,
                        "message": "Internal error",
                        "data": { "errorKind": "worker_shutdown" }
                    }),
                };
                return self.fail(id, error);
            }
            "sample" => {
                let interval = Duration::from_millis(rest.parse().unwrap_or(1));
                let mut words = SAMPLE.split(' ').peekable();
                while let Some(word) = words.next() {
                    if self.cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    let chunk = match words.peek() {
                        Some(_) => format!("{word} "),
                        None => word.to_owned(),
                    };
                    self.say(session, &chunk);
                    std::thread::sleep(interval);
                }
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
                        },
                        "auth": if auth_file().is_some() {
                            json!({ "status": {} })
                        } else {
                            json!({})
                        }
                    },
                    "authMethods": if auth_file().is_some()
                        && params["clientCapabilities"]["auth"]["terminal"] == true
                    {
                        json!([{
                            "id": "fake-login",
                            "name": "Fake login",
                            "type": "terminal",
                            "args": ["--sign-in"]
                        }])
                    } else {
                        json!([])
                    }
                }),
            ),
            "auth/status" if auth_file().is_some() => self.respond(
                &id,
                if auth_file().is_some_and(|file| signed_in(&file)) {
                    json!({
                        "state": SIGNED_IN,
                        "accounts": [{
                            "label": "Fake Plan",
                            "active": true,
                            "email": "person@example.com",
                            "organization": "Example Org"
                        }]
                    })
                } else {
                    json!({ "state": "unauthenticated" })
                },
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

fn auth_file() -> Option<PathBuf> {
    std::env::var_os(AUTH).map(PathBuf::from)
}

fn signed_in(file: &Path) -> bool {
    std::fs::read_to_string(file).is_ok_and(|state| state.trim() == SIGNED_IN)
}

/// Rewrites a growing counter in the file, for ever or for a given time.
fn beat(file: &Path, lifetime: Option<Duration>) -> ! {
    let started = Instant::now();
    let mut beat = 0_u64;
    while lifetime.is_none_or(|lifetime| started.elapsed() < lifetime) {
        std::fs::write(file, beat.to_string()).ok();
        beat = beat.wrapping_add(1);
        std::thread::sleep(HEARTBEAT_INTERVAL);
    }
    std::process::exit(0)
}

fn start_child(argument: &str) {
    let child = std::env::current_exe()
        .and_then(|program| std::process::Command::new(program).arg(argument).spawn());
    if child.is_err() {
        std::process::exit(3);
    }
}

/// The run that a client starts for the `terminal` sign-in method.
fn sign_in(file: &Path) -> ! {
    let mode = std::fs::read_to_string(file.with_extension("mode")).unwrap_or_default();
    match mode.trim() {
        "wait" => {
            start_child("--sign-in-child");
            beat(&file.with_extension("beat"), None)
        }
        "guest" => {
            start_child("--sign-in-guest");
            let written = std::fs::write(file, SIGNED_IN);
            std::process::exit(i32::from(written.is_err()))
        }
        "fail" => std::process::exit(1),
        delay => {
            std::thread::sleep(Duration::from_millis(delay.parse().unwrap_or(0)));
            let written = std::fs::write(file, SIGNED_IN);
            std::process::exit(i32::from(written.is_err()))
        }
    }
}

fn main() {
    if let Some(file) = auth_file() {
        if std::env::args().any(|argument| argument == "--sign-in") {
            sign_in(&file);
        }
        if std::env::args().any(|argument| argument == "--sign-in-child") {
            beat(&file.with_extension("child"), None);
        }
        if std::env::args().any(|argument| argument == "--sign-in-guest") {
            beat(&file.with_extension("child"), Some(GUEST_LIFETIME));
        }
    }
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
