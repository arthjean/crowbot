# How each provider treats the daemon's MCP server

This page records what the three provider agents did with a stdio MCP server
in the session mode that asks first, so that the harness rests on observed
behavior (US-001 of `tasks/prd-crow-bot-harness.md`, assumptions A1, A2, A3,
A6, A7, A9, and A11). The daemon reads nothing from this page: it is the
evidence behind `provider/acp.rs` and `mcp.rs`, and the place a later run
updates.

## How it was observed

`scripts/mcp-probe.py` is a throwaway ACP client and MCP server, not the
daemon's bridge. It starts each agent as the daemon's profile of that
provider starts it (the arguments, the environment, the session `_meta`, and
the session mode that asks first), opens one session with the probe as its
only MCP server, named `crowbot_probe`, and sends five prompts in one
session: call the read tool `probe_read` and the write tool `probe_write`,
create a file with the editing tool, run `cd sub` then `ls` as two commands,
`/compact`, and one short message after it. It allows each permission
request once and logs every message.

Run on 2026-11-24, on Fedora 45 (x86_64):

| Provider | Agent | Model | Sign-in |
|----------|-------|-------|---------|
| Claude | `claude-agent-acp` 0.107.0 (Claude Code 2.1.323), the pinned runtime | `claude-opus-5-5` | the user's own, as the daemon's Claude profile |
| ChatGPT | `codex-acp` 2.6.0, Codex 0.166.0, the pinned runtime | `gpt-6.2-sol` | the daemon's Codex home |
| Grok | `grok` 1.0.77, the pinned runtime | `grok-4.7` | the daemon's Grok home |

Each provider was signed in, so no row below is "not observed" for that
reason. A provider that cannot be signed in on the computer of a later run
gets "not observed" with the reason in each of its rows, and the degraded
mode of decision D9 until it is observed: its sessions run without daemon
tools, and its entry says so.

## A1: the model calls daemon tools in the mode that asks first

| Provider | Mode | `probe_read` | `probe_write` |
|----------|------|--------------|---------------|
| Claude | `default` | called, after a permission request | called, after a permission request |
| ChatGPT | `read-only` | called with no permission request (the tool declares `readOnlyHint: true`) | called, after a permission request |
| Grok | `--permission-mode default` | called, after a permission request (every MCP call asks: `MCPTool` is in the `ask` list of `provider/grok.rs`) | called, after a permission request |

All three pass A1. No provider takes the degraded mode of D9.

Claude loads the tools of a dynamic MCP server lazily: the model first ran
`ToolSearch` with `select:mcp__crowbot_probe__probe_read,mcp__crowbot_probe__probe_write`.
Grok did the same with its own `search_tool`. Neither asked for that search.

## A2: how a call names the server and the tool

The ACP tool call id reaches the MCP server with Claude and ChatGPT, so the
daemon can join the agent's tool call and the call it executes.

**Claude.** The permission request names both, in fields the adapter sets
and the model does not write:

```json
"toolCall": { "toolCallId": "toolu_01LEi7J39Uny1HeF4HrY6Y2q",
  "title": "mcp__crowbot_probe__probe_read", "kind": "other", "rawInput": {},
  "_meta": { "claudeCode": { "toolName": "mcp__crowbot_probe__probe_read",
    "mcpServer": { "name": "crowbot_probe", "source": "dynamic" } } } }
```

The tool call update carries the same `title` and `_meta.claudeCode.toolName`,
and the MCP `tools/call` carries `"_meta": { "claudecode/toolUseId": "toolu_01LEi7J39Uny1HeF4HrY6Y2q" }`.

**ChatGPT.** The permission request names neither: it holds the tool call id
and a flag.

```json
"toolCall": { "toolCallId": "exec-2209c9a9-1ce1-4feb-b506-c3060f85e8ec",
  "kind": "execute", "status": "pending" },
"_meta": { "is_mcp_tool_approval": true }
```

The tool call update with the same id, sent before the request, names both:

```json
{ "sessionUpdate": "tool_call", "toolCallId": "exec-2209c9a9-1ce1-4feb-b506-c3060f85e8ec",
  "kind": "execute", "title": "mcp.crowbot_probe.probe_write", "status": "in_progress",
  "rawInput": { "server": "crowbot_probe", "tool": "probe_write", "arguments": { "text": "caw" } },
  "_meta": { "is_mcp_tool_call": true } }
```

The MCP `tools/call` carries `"_meta": { "callId": "exec-2209c9a9-1ce1-4feb-b506-c3060f85e8ec" }`.
So the daemon names a ChatGPT request as a daemon tool through the tool call
it already received under that id.

**Grok.** The permission request names both, as one name `<server>__<tool>`
inside the input of its `use_tool`:

```json
"toolCall": { "toolCallId": "call-26a55dce-9e2d-4229-ad1b-845dc48f1f99-1",
  "kind": "other", "title": "crowbot_probe__probe_read",
  "rawInput": { "variant": "UseTool", "tool_name": "crowbot_probe__probe_read", "tool_input": {} },
  "_meta": { "x.ai/tool": { "name": "use_tool", "kind": "use_tool", "namespace": "grok_build" } } }
```

The last update names them apart: `"rawOutput": { "type": "MCP", "tool_name": "probe_read", "server_name": "crowbot_probe" }`.
The MCP `tools/call` carries only a `progressToken`.

A title alone never names a daemon tool: Claude and Grok title a shell
command with the command's text, which the model writes.

## A3: a file edit asks before the write

| Provider | Request before the write | What the request's target held |
|----------|--------------------------|--------------------------------|
| Claude | yes, tool `Write` | `rawInput.file_path` and `locations[0].path`: the file's absolute path; `content` held the diff |
| ChatGPT | yes, title `Edit files` | `locations[0].path`: the file's absolute path; the earlier tool call held the diff |
| Grok | yes, tool `search_replace` | `rawInput.file_path`: the file's absolute path, and the title ``Edit `<path>` ``; no `locations` in the request |

## A6: tool call updates carry an output

| Provider | Output in the updates |
|----------|-----------------------|
| Claude | yes: `content` text and `rawOutput` for MCP tools, files, and shell commands |
| ChatGPT | MCP tools: `rawOutput.result.content`. Shell commands: no `content` and no text in `rawOutput` (`{"exit_code": 0}`); the text is in `_meta.terminal_output_delta.data` |
| Grok | yes: `content` text and `rawOutput` (`output_for_prompt` for shell commands, `output.OkayOutput` for MCP tools) |

## A7: usage

| Provider | `UsageUpdate` | Tokens per turn in the prompt response | Cost |
|----------|---------------|----------------------------------------|------|
| Claude | yes, several per turn (`used`, `size` 1,000,000) | yes, `usage` (`inputTokens`, `outputTokens`, `cachedReadTokens`, `cachedWriteTokens`, `totalTokens`) | yes, cumulative `cost` in USD on the last `UsageUpdate` of a turn |
| ChatGPT | yes, several per turn (`used`, `size` 475,000) | yes, `usage` (same fields and `thoughtTokens`); `null` for `/compact` | no |
| Grok | no | not in `usage`: the counts are in `_meta` (`totalTokens`, `inputTokens`, `outputTokens`, `cachedReadTokens`, `reasoningTokens`) | no |

## A9: the folder of a command after a `cd`

| Provider | Request for `cd sub`, then `ls` | Folder in the request | Where `ls` ran |
|----------|---------------------------------|-----------------------|----------------|
| Claude | no request: the mode `default` allows both commands | none (`rawInput` holds `command` and `description`) | `sub`: the shell keeps its folder between commands |
| ChatGPT | no request: the `read-only` sandbox allows both | the tool call's `rawInput.cwd` names the session folder | the session folder: each command runs in a new shell |
| Grok | a request for each | none in the request; the result's `rawOutput.current_dir` names the session folder | the session folder: each command runs in a new shell |

So no provider puts the folder of a command in its permission request, and
Claude runs a command in a folder an earlier command chose. Under decision D3
no allow rule applies to a command on any of the three until that changes.

## A11: the context size after a compaction

`/compact` compacted the context on each provider.

| Provider | `UsageUpdate.used` before, after | Fall |
|----------|----------------------------------|------|
| Claude | 25,730, 6,870 (the compaction's `rawOutput` says `"preTokens": 25730, "postTokens": 6870`) | 18,860 tokens, 73% |
| ChatGPT | 28,321, 24,208 | 4,113 tokens, 15% |
| Grok | not observed: Grok sends no `UsageUpdate`. Its prompt response's `_meta.inputTokens` went from 21,888 to 21,013 and `cachedReadTokens` from 21,760 to 2,176 | not observed |

Claude and ChatGPT report the compaction itself as a tool call of kind
`think` titled `Compact conversation`.

## What the daemon does with this

- Every session carries the daemon's MCP server, on all three providers (A1).
- The daemon treats a permission request as one for its own tool when the
  request, or the tool call the agent sent under the same id, names the
  daemon's server in a field the adapter sets: Claude's
  `_meta.claudeCode.mcpServer.name`, ChatGPT's `rawInput.server` with
  `_meta.is_mcp_tool_call`, Grok's `rawInput.tool_name` of a `UseTool`, and
  the tool named is one the daemon serves (Claude and Grok join the server
  and the tool with `__`, so a server whose name starts with `crowbot__`
  could otherwise pass for the daemon's). It then selects the allow-once option and records `DaemonTool` (D1); the
  real gate is where it executes the tool.
- A daemon tool whose effect is `Read` declares `readOnlyHint`, so ChatGPT
  calls it without a request.
- Grok's usage is not read from `UsageUpdate`; its turns count as turns only
  (D4).

## Runs through the daemon

The ignored test `tools::real_providers_call_a_daemon_tool_through_the_bridge`
of `crates/crowbotd/tests/daemon` runs a real turn on each signed-in
provider through the daemon and its bridge (US-004). It runs only on request,
with `CROWBOT_REAL_PROVIDERS` naming the data folder of the user's daemon,
whose runtimes and sign-ins it uses with a database of its own, and
`CROWBOT_CLAUDE_EXE` naming the Claude agent when that folder holds no Claude
runtime (`CROWBOT_REAL_PROVIDER` keeps one provider):

```sh
CROWBOT_REAL_PROVIDERS=~/.local/share/crowbot \
  cargo test -p crowbotd --test daemon real_providers -- --ignored --nocapture
```

The prompt asks the bot to call the test tool `probe_read` (effect `Read`) of
the server `crowbot` and to quote what it returned.

Run on 2026-11-24, same versions as above:

| Provider | `ToolCallStarted.effect` | Permission request | A person asked | Answer |
|----------|--------------------------|--------------------|----------------|--------|
| Claude | `read` | allowed by the daemon, recorded as `DaemonTool` | no | quoted `bot=1 conversation=1` |
| ChatGPT | `read` | none (the tool declares `readOnlyHint`) | no | quoted the result `bot=2 conversation=2` |
| Grok | `read` | allowed by the daemon, recorded as `DaemonTool` | no | quoted `bot=1 conversation=1` |

Grok runs a session only once a sign-in the daemon started ended well, so the
test starts one: with the sign-in Grok's home holds, it ends at once.
