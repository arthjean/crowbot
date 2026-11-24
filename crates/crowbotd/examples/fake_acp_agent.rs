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
//! - `askin <folder> <command>`: the same as `ask`, for a request that names
//!   the folder the command runs in as `rawInput.cwd`.
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
//! - `failfirst <n> <path> <command>`: fail as `fail other` while the counter
//!   in the file is below `n`, counting up; then run the command.
//! - `sayfail <text>`: answer with the text, then fail as `fail other`.
//! - `toolfail`: start a tool call, then fail as `fail other`.
//! - `spend <used> <size> <cost> <input> <output>`: send a `UsageUpdate` of
//!   that context size and that cumulative cost in USD, answer `spent`, and
//!   end with those input and output tokens; a `-` leaves a figure out.
//! - `calls <count> <inputs> <outputs>`: run `count` tool calls titled
//!   `Read`, each completed at once, whose inputs and outputs go round the
//!   comma-separated lists (`_` sends no output), then answer `calls done`.
//!   A cancel ends it between two calls.
//! - `sample <interval ms>`: answer with a Markdown text that holds every kind
//!   of block, one word per interval, and end as cancelled when a cancel arrives.
//! - `deaf`: never end the turn, and ignore the cancel and the end of stdin;
//!   the process exits on its own after 20 seconds.
//! - `crash`: exit in the middle of the turn.
//! - `escape <path>`: start `sleep 120` in a session of its own, outside the
//!   agent's process group, write its process id to the file, then answer
//!   `escaped`. Unix only.
//! - `session`: report the session id and whether the session was resumed.
//! - `config`: report the system prompt, the text sent before the prompt, the
//!   model, and the working folder of the session.
//! - `settings`: report the model, the reasoning level, and the mode of the
//!   session.
//! - `started`: report the arguments of the process, the `CODEX_`, `GROK_`,
//!   and `INITIAL_AGENT_MODE` variables it was started with, and the `_meta`
//!   of the session.
//! - `write <path>`: write `written` to the file. A session that asks first
//!   requests permission, and writes only when the request is allowed.
//! - `leave <mode> <path>`: move the session to that mode on its own and say
//!   so, then, unless a cancel arrives within two seconds, write `written` to
//!   the file without asking, as an agent in a mode that does not ask.
//!
//! - `mcpconfig`: report the MCP servers the session was given, as JSON.
//! - `mcp <method> [params]`: start the first MCP server of the session, if
//!   it is not running, and send it the request, with the params given as
//!   JSON; report the answer as JSON.
//! - `mcpcall <tool> [arguments]`: call a tool of the daemon's server as the
//!   real agent of the flavor does (`docs/harness-providers.md`): a tool call
//!   that names the server, a permission request, then the MCP call with
//!   the tool call's id; report the result's text and whether it failed.
//! - `mcpanon <tool> [arguments]`: the same with a permission request and a
//!   tool call that name the tool in their title only.
//! - `mcplate <tool> [arguments]`: say `waiting`, wait for a cancel, then do
//!   as `mcpcall` and end as cancelled: an agent that asks for a daemon tool
//!   while its turn is being stopped.
//! - `mcppid`: report the process id of the running MCP server.
//!
//! The command is the last part of a prompt: the parts before it are reported
//! by `config` as the preface.
//!
//! The agent offers the models `fast` and `smart` through a session config
//! option, and starts each process on `fast`.
//!
//! The process stands for a provider when the daemon's profile of that
//! provider started it. With `CODEX_HOME` it is Codex-like: it offers the
//! models `gpt-fast` and `gpt-smart`, three reasoning levels, and session
//! modes, and it asks before a write only in the mode `read-only`. It is
//! signed in when its home holds the file `signed-in`, pushes that state
//! after `initialize`, and offers sign-ins that it runs itself on
//! `authenticate`. With `GROK_HOME` it is Grok-like: no session mode, a
//! sign-in named `grok.com` that must be accepted before each session and
//! that ends only after the client asked `_x.ai/auth/get_url` for the page
//! where the person signs in (the answer that names the page and the end of
//! the sign-in leave in one write, so that they reach the client at the same
//! moment), and it
//! asks before a write only when it was started with `--permission-mode
//! default` and the `config.toml` of its home holds an `ask` rule for `Write`.
//! A session opened with the `_meta` of the Claude profile offers the modes
//! of Claude, and asks before a write in the mode `default`. A sign-in the
//! agent runs reads how to behave from the file `sign-in-mode` of its home.
//! `FAKE_AGENT_NO_ASK_MODE` leaves out the mode in which the agent asks, and
//! `FAKE_AGENT_NO_MODELS` makes it report no model.
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
//! `fail` exits with an error, `xdg-open` and `browser` open a page with the
//! program of that name, or the one `BROWSER` names, then wait, a number waits that many milliseconds before it
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
const NO_ASK_MODE: &str = "FAKE_AGENT_NO_ASK_MODE";
const NO_MODELS: &str = "FAKE_AGENT_NO_MODELS";
const SIGNED_IN: &str = "authenticated";
/// The page a Grok-like agent names for its sign-in.
/// The page a `terminal` sign-in in the modes `xdg-open` and `browser` opens.
const SIGN_IN_PAGE: &str = "https://login.example/oauth/authorize?state=FAKE";
const GROK_SIGN_IN_PAGE: &str = "https://accounts.example/oauth2/device?user_code=FAKE-1234";
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

/// Which provider the process stands for, read from what the daemon's
/// profile of that provider sets at each start.
#[derive(Clone, Default, PartialEq, Eq)]
enum Flavor {
    #[default]
    Plain,
    /// Started with `CODEX_HOME`: session modes, a pushed sign-in state, and
    /// sign-ins that the agent runs itself.
    Codex(PathBuf),
    /// Started with `GROK_HOME`: no session mode, a sign-in to name before
    /// each session, and permission rules read from its home.
    Grok(PathBuf),
}

impl Flavor {
    fn read() -> Self {
        if let Some(home) = std::env::var_os("CODEX_HOME") {
            Self::Codex(PathBuf::from(home))
        } else if let Some(home) = std::env::var_os("GROK_HOME") {
            Self::Grok(PathBuf::from(home))
        } else {
            Self::Plain
        }
    }

    fn home(&self) -> Option<&Path> {
        match self {
            Self::Plain => None,
            Self::Codex(home) | Self::Grok(home) => Some(home),
        }
    }

    fn models(&self) -> [&'static str; 2] {
        match self {
            Self::Plain => MODELS,
            Self::Codex(_) => ["gpt-fast", "gpt-smart"],
            Self::Grok(_) => ["grok-fast", "grok-smart"],
        }
    }

    /// The reasoning levels, with the one a session starts on first.
    fn efforts(&self) -> &'static [&'static str] {
        match self {
            Self::Plain => &[],
            Self::Codex(_) => &["medium", "low", "high"],
            Self::Grok(_) => &["high", "low"],
        }
    }

    fn signed_in_file(&self) -> Option<PathBuf> {
        self.home().map(|home| home.join("signed-in"))
    }

    fn signed_in(&self) -> bool {
        self.signed_in_file().is_none_or(|file| file.is_file())
    }

    /// What the file `sign-in-mode` of the home says, if anything.
    fn sign_in_mode(&self) -> String {
        self.home()
            .and_then(|home| std::fs::read_to_string(home.join("sign-in-mode")).ok())
            .map(|mode| mode.trim().to_owned())
            .unwrap_or_default()
    }

    /// Runs the sign-in the agent offers. The file `sign-in-mode` of the home
    /// tells how it goes: `fail` ends with an error, `wait` never ends, and
    /// no file signs in at once. `false` means the sign-in failed.
    fn sign_in(&self, method: &str) -> bool {
        let Some(home) = self.home() else {
            return true;
        };
        match self.sign_in_mode().as_str() {
            "fail" => false,
            "wait" => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            _ => std::fs::write(home.join("signed-in"), method).is_ok(),
        }
    }
}

/// The answer to a request for the page of a Grok sign-in.
fn sign_in_page_answer(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "auth_url": GROK_SIGN_IN_PAGE,
            "external_provider": false,
            "mode": "device"
        }
    })
}

/// The session modes of the Codex flavor and of a session that a Claude
/// profile opened. `FAKE_AGENT_NO_ASK_MODE` leaves out the mode in which the
/// agent asks before each change.
fn offered_modes(flavor: &Flavor, claude: bool) -> Vec<&'static str> {
    let modes: &[&str] = match flavor {
        Flavor::Codex(_) => &["read-only", "agent", "agent-full-access"],
        Flavor::Plain if claude => &["default", "acceptEdits", "bypassPermissions"],
        _ => &[],
    };
    let skipped = usize::from(std::env::var_os(NO_ASK_MODE).is_some());
    modes.iter().copied().skip(skipped).collect()
}

#[derive(Default)]
struct Agent {
    flavor: Flavor,
    /// The sign-in was accepted in this process.
    authenticated: AtomicBool,
    /// The client asked for the address of the sign-in page.
    url_asked: AtomicBool,
    /// A Grok sign-in waits for the client to ask for its page.
    awaiting_page: AtomicBool,
    /// The request for the page, held while a sign-in runs: it is answered
    /// in the same write as the end of the sign-in.
    held_page: Mutex<Option<Value>>,
    output: Mutex<()>,
    cancelled: AtomicBool,
    deaf: AtomicBool,
    resumed: AtomicBool,
    next_request: AtomicU64,
    answers: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    session: Mutex<Session>,
    mcp: Mutex<Option<Mcp>>,
}

/// The first MCP server of the session, which the agent starts at its first
/// use, as a real agent starts it at the session's start.
struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    next: u64,
    answers: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
}

impl Mcp {
    fn start(server: &Value) -> Result<Self, String> {
        let command = server["command"].as_str().ok_or("no command")?;
        let args: Vec<String> = server["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|arg| arg.as_str().map(str::to_owned))
            .collect();
        let mut child = std::process::Command::new(command)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let answers: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>> = Arc::default();
        let waiting = answers.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(answer) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let id = answer["id"].to_string();
                let sender = waiting
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                if let Some(sender) = sender {
                    sender.send(answer).ok();
                }
            }
            // The server ended: every request that waits learns it.
            waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        });
        let mut mcp = Self {
            child,
            stdin,
            next: 0,
            answers,
        };
        let initialized = mcp.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "fake", "version": "0" } }),
        );
        if initialized.get("result").is_none() {
            return Err(format!("initialize failed: {initialized}"));
        }
        mcp.notify("notifications/initialized");
        Ok(mcp)
    }

    fn notify(&mut self, method: &str) {
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "method": method })
        )
        .ok();
        self.stdin.flush().ok();
    }

    /// Sends a request and waits for its answer; `null` when the server
    /// ended first.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = json!(self.next);
        let (sender, answer) = mpsc::channel();
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string(), sender);
        let written = writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )
        .and_then(|()| self.stdin.flush());
        if written.is_err() {
            return Value::Null;
        }
        answer
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or(Value::Null)
    }
}

/// What the client set when it created or resumed the session.
#[derive(Default)]
struct Session {
    system_prompt: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    /// The session was opened with the `_meta` of the Claude profile.
    claude: bool,
    meta: Value,
    cwd: String,
    mcp_servers: Value,
}

/// A prompt `failfirst <n> <path> <command>`: `Ok` when this turn fails,
/// with the counter of the file one higher, and the command to run otherwise.
fn failing_first(text: &str) -> Option<Result<(), String>> {
    let rest = text.strip_prefix("failfirst ")?;
    let mut fields = rest.splitn(3, ' ');
    let limit: u32 = fields.next()?.parse().ok()?;
    let path = fields.next()?;
    let command = fields.next().unwrap_or_default().to_owned();
    let failed: u32 = std::fs::read_to_string(path)
        .ok()
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    if failed < limit {
        std::fs::write(path, (failed + 1).to_string()).ok();
        return Some(Ok(()));
    }
    Some(Err(command))
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

    /// Sends messages in one write, so that the client reads them together.
    fn send_together(&self, messages: &[Value]) {
        let _guard = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        let lines: String = messages
            .iter()
            .map(|message| format!("{message}\n"))
            .collect();
        let mut stdout = std::io::stdout().lock();
        let written = stdout
            .write_all(lines.as_bytes())
            .and_then(|()| stdout.flush());
        if written.is_err() && !self.deaf.load(Ordering::SeqCst) {
            std::process::exit(0);
        }
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
    fn ask(&self, session: &Value, tool_call: Value, allow_once: bool) -> String {
        self.ask_with(session, tool_call, allow_once, Value::Null)
    }

    /// The same, with the `_meta` of the request. A tool call that has an
    /// id keeps it.
    fn ask_with(
        &self,
        session: &Value,
        mut tool_call: Value,
        allow_once: bool,
        meta: Value,
    ) -> String {
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
        if tool_call.get("toolCallId").is_none() {
            tool_call["toolCallId"] = format!("tool-{request}").into();
        }
        let mut params = json!({
            "sessionId": session,
            "toolCall": tool_call,
            "options": options
        });
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request,
            "method": "session/request_permission",
            "params": params
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
    /// session's config options and modes.
    fn open_session(&self, params: &Value) -> Value {
        let mut session = self.session();
        session.cwd = params["cwd"].as_str().unwrap_or_default().to_owned();
        let system_prompt = texts(&params["systemPrompt"]["content"]);
        session.system_prompt = (!system_prompt.is_empty()).then(|| system_prompt.join("\n"));
        session.meta = params["_meta"].clone();
        session.mcp_servers = params["mcpServers"].clone();
        session.claude = session.meta["claudeCode"].is_object();
        let modes = offered_modes(&self.flavor, session.claude);
        if session.mode.is_none() {
            session.mode = match &self.flavor {
                Flavor::Codex(_) => Some(
                    std::env::var("INITIAL_AGENT_MODE")
                        .ok()
                        .filter(|mode| modes.contains(&mode.as_str()))
                        .unwrap_or_else(|| "agent".to_owned()),
                ),
                Flavor::Plain if session.claude => Some("default".to_owned()),
                _ => None,
            };
        }
        let mut opened = json!({ "configOptions": self.config_options(&session) });
        if let Some(mode) = &session.mode {
            opened["modes"] = json!({
                "currentModeId": mode,
                "availableModes": modes
                    .iter()
                    .map(|mode| json!({ "id": mode, "name": mode }))
                    .collect::<Vec<_>>(),
            });
        }
        opened
    }

    fn config_options(&self, session: &Session) -> Value {
        if std::env::var_os(NO_MODELS).is_some() {
            return json!([]);
        }
        let models = self.flavor.models();
        let mut options = vec![json!({
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": session.model.as_deref().unwrap_or(models[0]),
            "options": models.map(|model| json!({ "value": model, "name": model })),
        })];
        // The second model reasons further than the first: its levels are
        // its own, as with the real agents.
        let mut efforts = self.flavor.efforts().to_vec();
        if !efforts.is_empty() && session.model.as_deref() == Some(models[1]) {
            efforts.push("xhigh");
        }
        if let Some(first) = efforts.first() {
            options.push(json!({
                "id": "reasoning_effort",
                "name": "Reasoning effort",
                "category": "thought_level",
                "type": "select",
                "currentValue": session.effort.as_deref().unwrap_or(first),
                "options": efforts
                    .iter()
                    .map(|effort| json!({ "value": effort, "name": effort }))
                    .collect::<Vec<_>>(),
            }));
        }
        Value::Array(options)
    }

    /// Whether the session asks before it changes a file. Each provider has
    /// its own way to be put in that state, and the daemon's profile of the
    /// provider is what does it.
    fn asks_first(&self) -> bool {
        let session = self.session();
        match &self.flavor {
            Flavor::Codex(_) => session.mode.as_deref() == Some("read-only"),
            Flavor::Grok(home) => {
                let arguments: Vec<String> = std::env::args().collect();
                let default_mode = arguments
                    .windows(2)
                    .any(|pair| pair[0] == "--permission-mode" && pair[1] == "default");
                let rules = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
                let asks = rules
                    .lines()
                    .any(|line| line.trim_start().starts_with("ask") && line.contains("\"Write\""));
                default_mode && asks
            }
            Flavor::Plain => !session.claude || session.mode.as_deref() == Some("default"),
        }
    }

    fn prompt(&self, id: &Value, params: &Value) {
        self.cancelled.store(false, Ordering::SeqCst);
        let session = &params["sessionId"];
        let mut parts = texts(&params["prompt"]);
        let text = parts.pop().unwrap_or_default();
        let text = match failing_first(&text) {
            Some(Ok(())) => {
                return self.fail(
                    id,
                    json!({
                        "code": -32603,
                        "message": "Internal error",
                        "data": { "errorKind": "worker_shutdown" }
                    }),
                );
            }
            Some(Err(command)) => command,
            None => text,
        };
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
            "ask" | "demand" | "askfile" | "asklong" | "askin" => {
                let tool_call = match command {
                    "askfile" => json!({
                        "title": format!("Edit {rest}"),
                        "kind": "edit",
                        "locations": [{ "path": rest }]
                    }),
                    "askin" => {
                        let (folder, command) = rest.split_once(' ').unwrap_or((rest, ""));
                        json!({
                            "title": command,
                            "kind": "execute",
                            "rawInput": { "command": command, "cwd": folder }
                        })
                    }
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
            "sayfail" | "toolfail" => {
                if command == "sayfail" {
                    self.say(session, rest);
                } else {
                    self.start_tool(
                        session,
                        json!({ "title": "Run", "kind": "execute", "status": "in_progress" }),
                    );
                }
                return self.fail(
                    id,
                    json!({
                        "code": -32603,
                        "message": "Internal error",
                        "data": { "errorKind": "worker_shutdown" }
                    }),
                );
            }
            "spend" => {
                let figures: Vec<Option<f64>> =
                    rest.split(' ').map(|figure| figure.parse().ok()).collect();
                let figure = |index: usize| figures.get(index).copied().flatten();
                if let (Some(used), Some(size)) = (figure(0), figure(1)) {
                    let mut update = json!({
                        "sessionUpdate": "usage_update",
                        "used": used as u64,
                        "size": size as u64
                    });
                    if let Some(cost) = figure(2) {
                        update["cost"] = json!({ "amount": cost, "currency": "USD" });
                    }
                    self.update(session, update);
                }
                self.say(session, "spent");
                let mut answer = json!({ "stopReason": "end_turn" });
                if let (Some(input), Some(output)) = (figure(3), figure(4)) {
                    let (input, output) = (input as u64, output as u64);
                    answer["usage"] = json!({
                        "inputTokens": input,
                        "outputTokens": output,
                        "totalTokens": input + output
                    });
                }
                return self.respond(id, answer);
            }
            "calls" => {
                let mut fields = rest.split(' ');
                let count: usize = fields.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                let inputs: Vec<&str> = fields.next().unwrap_or("a").split(',').collect();
                let outputs: Vec<&str> = fields.next().unwrap_or("_").split(',').collect();
                for index in 0..count {
                    if self.cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    let call = self.start_tool(
                        session,
                        json!({
                            "title": "Read",
                            "kind": "read",
                            "status": "in_progress",
                            "rawInput": { "file_path": inputs[index % inputs.len()] }
                        }),
                    );
                    let output =
                        Some(outputs[index % outputs.len()]).filter(|output| *output != "_");
                    self.update_tool(session, &call, "completed", output);
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !self.cancelled.load(Ordering::SeqCst) {
                    self.say(session, "calls done");
                }
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
                // Long past what a test waits for, and short enough that an
                // agent left running ends on its own.
                std::thread::sleep(Duration::from_secs(20));
                std::process::exit(0);
            }
            "crash" => std::process::exit(3),
            #[cfg(unix)]
            "escape" => {
                use std::os::unix::process::CommandExt;

                let mut command = std::process::Command::new("sleep");
                command.arg("120").stdin(std::process::Stdio::null());
                // SAFETY: `setsid` is async-signal-safe.
                unsafe {
                    command.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                let escaped = command.spawn().and_then(|mut child| {
                    std::fs::write(rest, child.id().to_string())?;
                    // Reaped while the agent lives; outlives it otherwise.
                    std::thread::spawn(move || child.wait());
                    Ok(())
                });
                match escaped {
                    Ok(()) => self.say(session, "escaped"),
                    Err(error) => self.say(session, &format!("cannot escape: {error}")),
                }
            }
            "mcpconfig" => {
                let servers = self.session().mcp_servers.clone();
                self.say(session, &format!("mcp={servers}"));
            }
            "mcppid" => match self.mcp_running() {
                Ok(pid) => self.say(session, &format!("pid={pid}")),
                Err(error) => self.say(session, &format!("mcp failed: {error}")),
            },
            "mcp" => {
                let (method, params) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let params = serde_json::from_str(params).unwrap_or(Value::Null);
                let answer = self.mcp_request(method, params);
                self.say(session, &format!("answer={answer}"));
            }
            "mcplate" => {
                let (tool, arguments) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let arguments: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
                self.say(session, "waiting");
                self.wait_for_cancel();
                let report = self.mcp_call(session, tool, arguments, false);
                self.say(session, &report);
            }
            "mcpcall" | "mcpanon" => {
                let (tool, arguments) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let arguments: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
                let report = self.mcp_call(session, tool, arguments, command == "mcpanon");
                self.say(session, &report);
            }
            "session" => self.say(
                session,
                &format!(
                    "session={} resumed={}",
                    session.as_str().unwrap_or_default(),
                    self.resumed.load(Ordering::SeqCst)
                ),
            ),
            "settings" => {
                let report = {
                    let config = self.session();
                    format!(
                        "model={} effort={} mode={}",
                        config.model.as_deref().unwrap_or(self.flavor.models()[0]),
                        config
                            .effort
                            .as_deref()
                            .or(self.flavor.efforts().first().copied())
                            .unwrap_or("none"),
                        config.mode.as_deref().unwrap_or("none")
                    )
                };
                self.say(session, &report);
            }
            "started" => {
                let arguments: Vec<String> = std::env::args().skip(1).collect();
                let mut variables: Vec<String> = std::env::vars()
                    .filter(|(name, _)| {
                        name.starts_with("CODEX_")
                            || name.starts_with("GROK_")
                            || name == "INITIAL_AGENT_MODE"
                    })
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                variables.sort();
                let meta = self.session().meta.clone();
                self.say(
                    session,
                    &format!(
                        "args=[{}] vars=[{}] meta={meta}",
                        arguments.join(" "),
                        variables.join(" ")
                    ),
                );
            }
            "leave" => {
                let (mode, path) = rest.split_once(' ').unwrap_or((rest, ""));
                self.session().mode = Some(mode.to_owned());
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session,
                        "update": {
                            "sessionUpdate": "current_mode_update",
                            "currentModeId": mode
                        }
                    }
                }));
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while std::time::Instant::now() < deadline && !self.cancelled.load(Ordering::SeqCst)
                {
                    std::thread::sleep(POLL_INTERVAL);
                }
                if !self.cancelled.load(Ordering::SeqCst) {
                    std::fs::write(path, "written").ok();
                    self.say(session, "write=written");
                }
            }
            "write" => {
                let allowed = !self.asks_first() || {
                    let tool_call = json!({
                        "title": format!("Write {rest}"),
                        "kind": "edit",
                        "locations": [{ "path": rest }]
                    });
                    self.ask(session, tool_call, true) == "allow"
                };
                let outcome = if !allowed {
                    "declined"
                } else if std::fs::write(rest, "written").is_ok() {
                    "written"
                } else {
                    "failed"
                };
                self.say(session, &format!("write={outcome}"));
            }
            "config" => {
                let report = {
                    let config = self.session();
                    format!(
                        "system=[{}] preface=[{}] model={} cwd={}",
                        config.system_prompt.as_deref().unwrap_or_default(),
                        parts.join("|"),
                        config.model.as_deref().unwrap_or(self.flavor.models()[0]),
                        config.cwd
                    )
                };
                self.say(session, &report);
            }
            _ => self.say(session, text),
        }
        self.end(id);
    }

    /// The process id of the session's MCP server, started if need be.
    fn mcp_running(&self) -> Result<u32, String> {
        let mut mcp = self.mcp.lock().unwrap_or_else(PoisonError::into_inner);
        if mcp.is_none() {
            let servers = self.session().mcp_servers.clone();
            *mcp = Some(Mcp::start(&servers[0])?);
        }
        Ok(mcp.as_ref().map_or(0, |mcp| mcp.child.id()))
    }

    fn mcp_request(&self, method: &str, params: Value) -> Value {
        if let Err(error) = self.mcp_running() {
            return json!({ "failed": error });
        }
        let mut mcp = self.mcp.lock().unwrap_or_else(PoisonError::into_inner);
        mcp.as_mut()
            .map_or(Value::Null, |mcp| mcp.request(method, params))
    }

    /// Calls a tool of the daemon's server as the agent of the flavor does.
    fn mcp_call(&self, session: &Value, tool: &str, arguments: Value, anonymous: bool) -> String {
        let server = self.session().mcp_servers[0]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let id = format!(
            "call-{}",
            1000 + self.next_request.fetch_add(1, Ordering::SeqCst)
        );
        let (started, asked, request_meta, call_meta) = if anonymous {
            let title = format!("mcp__{server}__{tool}");
            (
                json!({ "title": title, "kind": "other", "status": "pending", "rawInput": arguments }),
                json!({ "toolCallId": id, "title": title, "kind": "other", "rawInput": arguments }),
                Value::Null,
                json!({}),
            )
        } else {
            match &self.flavor {
                Flavor::Codex(_) => (
                    json!({
                        "title": format!("mcp.{server}.{tool}"),
                        "kind": "execute",
                        "status": "in_progress",
                        "rawInput": { "server": server, "tool": tool, "arguments": arguments },
                        "_meta": { "is_mcp_tool_call": true }
                    }),
                    json!({ "toolCallId": id, "kind": "execute", "status": "pending" }),
                    json!({ "is_mcp_tool_approval": true }),
                    json!({ "callId": id }),
                ),
                Flavor::Grok(_) => {
                    let name = format!("{server}__{tool}");
                    let meta = json!({ "x.ai/tool": { "name": "use_tool", "kind": "use_tool" } });
                    (
                        json!({
                            "title": "use_tool",
                            "rawInput": { "tool_name": name, "tool_input": arguments },
                            "_meta": meta
                        }),
                        json!({
                            "toolCallId": id,
                            "kind": "other",
                            "title": name,
                            "rawInput": { "variant": "UseTool", "tool_name": name, "tool_input": arguments },
                            "_meta": meta
                        }),
                        Value::Null,
                        json!({}),
                    )
                }
                Flavor::Plain => {
                    let name = format!("mcp__{server}__{tool}");
                    (
                        json!({
                            "title": name,
                            "kind": "other",
                            "status": "pending",
                            "rawInput": arguments,
                            "_meta": { "claudeCode": { "toolName": name } }
                        }),
                        json!({
                            "toolCallId": id,
                            "title": name,
                            "kind": "other",
                            "rawInput": arguments,
                            "_meta": { "claudeCode": { "toolName": name, "mcpServer": { "name": server, "source": "dynamic" } } }
                        }),
                        Value::Null,
                        json!({ "claudecode/toolUseId": id }),
                    )
                }
            }
        };
        let mut started = started;
        started["sessionUpdate"] = "tool_call".into();
        started["toolCallId"] = id.as_str().into();
        self.update(session, started);
        let outcome = self.ask_with(session, asked, true, request_meta);
        if outcome != "allow" {
            self.update_tool(session, &id, "failed", None);
            return format!("permission={outcome}");
        }
        let answer = self.mcp_request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments, "_meta": call_meta }),
        );
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| answer.to_string());
        let failed = answer["result"]["isError"] == true || answer.get("error").is_some();
        let status = if failed { "failed" } else { "completed" };
        self.update_tool(session, &id, status, Some(&text));
        format!("call={text} failed={failed}")
    }

    fn error(&self, id: &Value, code: i64, message: &str, data: Value) {
        let mut error = json!({ "code": code, "message": message });
        if !data.is_null() {
            error["data"] = data;
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error }));
    }

    /// What the agent tells about itself: its capabilities and its sign-ins.
    fn initialized(&self, params: &Value) -> Value {
        let resume_only = json!({ "resume": {} });
        let with_system_prompt = json!({ "resume": {}, "systemPrompt": {} });
        let agent_sign_in = |id: &str, name: &str| json!({ "id": id, "name": name, "description": format!("{name} in the browser") });
        let (capabilities, methods) = match &self.flavor {
            Flavor::Codex(_) => (
                json!({
                    "sessionCapabilities": with_system_prompt,
                    "auth": {},
                    "_meta": { "authStatus": true }
                }),
                json!([
                    {
                        "id": "api-key",
                        "name": "API key",
                        "_meta": { "api-key": { "provider": "openai" } }
                    },
                    agent_sign_in("chat-gpt", "Sign in with ChatGPT"),
                    agent_sign_in("device-code", "Sign in with a device code"),
                ]),
            ),
            Flavor::Grok(_) => (
                json!({ "sessionCapabilities": resume_only, "auth": {} }),
                json!([agent_sign_in("grok.com", "Sign in with Grok")]),
            ),
            Flavor::Plain => (
                json!({
                    "sessionCapabilities": if std::env::var_os(SYSTEM_PROMPT).is_some() {
                        with_system_prompt
                    } else {
                        resume_only
                    },
                    "auth": if auth_file().is_some() {
                        json!({ "status": {} })
                    } else {
                        json!({})
                    }
                }),
                if auth_file().is_some() && params["clientCapabilities"]["auth"]["terminal"] == true
                {
                    json!([{
                        "id": "fake-login",
                        "name": "Fake login",
                        "type": "terminal",
                        "args": ["--sign-in"]
                    }])
                } else {
                    json!([])
                },
            ),
        };
        json!({
            "protocolVersion": params["protocolVersion"],
            "agentCapabilities": capabilities,
            "authMethods": methods
        })
    }

    /// Why the agent opens no session, when it is signed out.
    fn refuses_session(&self) -> Option<Value> {
        match &self.flavor {
            Flavor::Codex(_) if !self.flavor.signed_in() => Some(Value::Null),
            Flavor::Grok(_) if !self.authenticated.load(Ordering::SeqCst) => {
                Some(json!("no auth method id provided"))
            }
            _ => None,
        }
    }

    fn request(self: &Arc<Self>, id: Value, method: &str, params: Value) {
        match method {
            "initialize" => {
                self.respond(&id, self.initialized(&params));
                if matches!(self.flavor, Flavor::Codex(_)) {
                    let status = if self.flavor.signed_in() {
                        json!({
                            "kind": "account",
                            "label": "Fake Plus",
                            "account": { "email": "person@example.com", "plan": "plus" }
                        })
                    } else {
                        json!({ "kind": "none", "label": "Not logged in" })
                    };
                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "method": "_auth/status_update",
                        "params": { "authStatus": status }
                    }));
                }
            }
            "authenticate" => {
                let method = params["methodId"].as_str().unwrap_or_default().to_owned();
                let agent = self.clone();
                // A sign-in can last: the agent keeps reading meanwhile.
                std::thread::spawn(move || {
                    let known = match &agent.flavor {
                        Flavor::Codex(_) => ["chat-gpt", "device-code"].contains(&method.as_str()),
                        Flavor::Grok(_) => method == "grok.com",
                        Flavor::Plain => false,
                    };
                    if !known {
                        return agent.error(&id, -32602, "unknown sign-in method", Value::Null);
                    }
                    // As Grok does, a sign-in ends only once the client asked for
                    // the page where the person signs in.
                    let mut page = None;
                    if matches!(agent.flavor, Flavor::Grok(_)) && !agent.flavor.signed_in() {
                        agent.awaiting_page.store(true, Ordering::SeqCst);
                        let deadline = std::time::Instant::now() + Duration::from_secs(60);
                        while !agent.url_asked.load(Ordering::SeqCst) {
                            if std::time::Instant::now() > deadline {
                                return agent.error(
                                    &id,
                                    -32000,
                                    "Authentication required",
                                    Value::Null,
                                );
                            }
                            std::thread::sleep(POLL_INTERVAL);
                        }
                        page = agent
                            .held_page
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .take()
                            .map(|asked| sign_in_page_answer(&asked));
                        // A sign-in that never ends still names its page.
                        if agent.flavor.sign_in_mode() == "wait"
                            && let Some(page) = page.take()
                        {
                            agent.send(&page);
                        }
                    }
                    let end = if agent.flavor.signed_in() || agent.flavor.sign_in(&method) {
                        agent.authenticated.store(true, Ordering::SeqCst);
                        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
                    } else {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32000, "message": "Authentication required" }
                        })
                    };
                    let mut messages: Vec<Value> = page.into_iter().collect();
                    messages.push(end);
                    agent.send_together(&messages);
                });
            }
            "_x.ai/auth/get_url" if matches!(self.flavor, Flavor::Grok(_)) => {
                if self.awaiting_page.swap(false, Ordering::SeqCst) {
                    *self
                        .held_page
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(id);
                    self.url_asked.store(true, Ordering::SeqCst);
                } else {
                    self.url_asked.store(true, Ordering::SeqCst);
                    self.send(&sign_in_page_answer(&id));
                }
            }
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
            "session/new" | "session/resume" => {
                if let Some(data) = self.refuses_session() {
                    return self.error(&id, -32000, "Authentication required", data);
                }
                let mut opened = self.open_session(&params);
                if method == "session/resume" {
                    self.resumed.store(true, Ordering::SeqCst);
                } else {
                    opened["sessionId"] =
                        format!("fake-session-{}-{}", std::process::id(), started_at()).into();
                }
                self.respond(&id, opened);
            }
            "session/set_mode" => {
                let mode = params["modeId"].as_str().unwrap_or_default();
                let mut session = self.session();
                if offered_modes(&self.flavor, session.claude).contains(&mode) {
                    session.mode = Some(mode.to_owned());
                    drop(session);
                    self.respond(&id, json!({}));
                } else {
                    drop(session);
                    self.error(&id, -32602, &format!("unknown mode {mode}"), Value::Null);
                }
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or_default();
                let option = params["configId"].as_str().unwrap_or_default();
                let mut session = self.session();
                let set = match option {
                    "model" if self.flavor.models().contains(&value) => {
                        session.model = Some(value.to_owned());
                        true
                    }
                    "reasoning_effort" if self.flavor.efforts().contains(&value) => {
                        session.effort = Some(value.to_owned());
                        true
                    }
                    _ => false,
                };
                let options = self.config_options(&session);
                drop(session);
                if set {
                    self.respond(&id, json!({ "configOptions": options }));
                } else {
                    self.error(
                        &id,
                        -32602,
                        &format!("Invalid value for config option {option}: {value}"),
                        Value::Null,
                    );
                }
            }
            "session/prompt" => {
                let agent = self.clone();
                std::thread::spawn(move || agent.prompt(&id, &params));
            }
            _ => self.error(
                &id,
                -32601,
                &format!("unknown method {method}"),
                Value::Null,
            ),
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
        // Opens the page as an agent does, with `xdg-open` from `PATH` or with
        // the program `BROWSER` names, then waits. Only the opener of a
        // sign-in of the daemon is run: a test never opens a real browser.
        opener @ ("xdg-open" | "browser") => {
            if std::env::var_os("CROWBOT_SIGN_IN_PAGE").is_none() {
                std::process::exit(5);
            }
            let program = if opener == "browser" {
                std::env::var_os("BROWSER").unwrap_or_default()
            } else {
                "xdg-open".into()
            };
            let opened = std::process::Command::new(program)
                .arg(SIGN_IN_PAGE)
                .status()
                .is_ok_and(|status| status.success());
            if !opened {
                std::process::exit(6);
            }
            beat(&file.with_extension("beat"), None)
        }
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
    let agent = Arc::new(Agent {
        flavor: Flavor::read(),
        ..Agent::default()
    });
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
