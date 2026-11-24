//! The bridge: the daemon's own program, started by an agent as its MCP
//! server with `--mcp-bridge <endpoint>`. It holds no logic. It relays each
//! line the agent writes to the endpoint of its session, and each answer
//! back. Before it sends anything it checks that the endpoint belongs to a
//! process of its own account (`transport::connect`).
//!
//! When the endpoint cannot be reached, or closes because the daemon ended,
//! the bridge answers each request that waits with a JSON-RPC error and
//! exits: the turn goes on without the daemon's tools.

use std::collections::BTreeMap;
use std::process::ExitCode;

use crowbot_protocol::Endpoint;
use crowbot_protocol::transport::{self, MessageReader, MessageWriter};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};

/// The argument that makes the daemon's program the bridge of a session.
pub const BRIDGE: &str = "--mcp-bridge";

const UNREACHABLE: i64 = -32000;
const PARSE_ERROR: i64 = -32700;

/// Runs the bridge, given the endpoint that follows `--mcp-bridge`.
pub fn run(mut args: impl Iterator<Item = String>) -> ExitCode {
    let (Some(address), None) = (args.next(), args.next()) else {
        eprintln!("[crowbotd] {BRIDGE} needs the endpoint of a session");
        return ExitCode::from(2);
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("[crowbotd] the bridge cannot start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = runtime.block_on(relay(Endpoint::new(address)));
    // A read of the agent's input may still block a thread of the runtime:
    // it does not hold the exit back.
    runtime.shutdown_background();
    status
}

struct Agent {
    lines: tokio::io::Lines<BufReader<tokio::io::Stdin>>,
    stdout: Stdout,
}

impl Agent {
    async fn write(&mut self, message: &Value) -> std::io::Result<()> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdout.write_all(line.as_bytes()).await?;
        self.stdout.flush().await
    }

    async fn refuse(&mut self, id: Value, code: i64, message: &str) -> std::io::Result<()> {
        self.write(
            &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        )
        .await
    }
}

/// The id of a request, which an answer carries back.
fn request_id(message: &Value) -> Option<Value> {
    message.get("method")?;
    message.get("id").filter(|id| !id.is_null()).cloned()
}

async fn relay(endpoint: Endpoint) -> ExitCode {
    let mut agent = Agent {
        lines: BufReader::new(tokio::io::stdin()).lines(),
        stdout: tokio::io::stdout(),
    };
    match transport::connect(&endpoint).await {
        Ok((reader, writer)) => relay_to(&mut agent, reader, writer).await,
        Err(error) => {
            let message = format!("Crow Bot's daemon cannot be reached: {error}");
            eprintln!("[crowbotd] the bridge cannot reach {endpoint}: {error}");
            // The first request learns why, and the bridge ends.
            while let Ok(Some(line)) = agent.lines.next_line().await {
                if let Some(id) = serde_json::from_str::<Value>(&line)
                    .ok()
                    .as_ref()
                    .and_then(request_id)
                {
                    agent.refuse(id, UNREACHABLE, &message).await.ok();
                    break;
                }
            }
            ExitCode::FAILURE
        }
    }
}

async fn relay_to(
    agent: &mut Agent,
    mut reader: MessageReader,
    mut writer: MessageWriter,
) -> ExitCode {
    // The requests sent and not answered yet, by their id as text.
    let mut waiting = BTreeMap::<String, Value>::new();
    loop {
        tokio::select! {
            line = agent.lines.next_line() => {
                let Ok(Some(line)) = line else {
                    return ExitCode::SUCCESS;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    if agent.refuse(Value::Null, PARSE_ERROR, "A line is one JSON message.").await.is_err() {
                        return ExitCode::FAILURE;
                    }
                    continue;
                };
                if let Some(id) = request_id(&message) {
                    waiting.insert(id.to_string(), id);
                }
                if writer.write(&message).await.is_err() {
                    break;
                }
            }
            answer = reader.read::<Value>() => {
                let Ok(Some(answer)) = answer else {
                    break;
                };
                if let Some(id) = answer.get("id") {
                    waiting.remove(&id.to_string());
                }
                if agent.write(&answer).await.is_err() {
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    for id in waiting.into_values() {
        agent
            .refuse(
                id,
                UNREACHABLE,
                "Crow Bot's daemon closed the connection of this session.",
            )
            .await
            .ok();
    }
    ExitCode::FAILURE
}
