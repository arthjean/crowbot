[PRD]
# PRD: Crow Bot harness

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-23 | Arthur Jean | Initial draft, from the audit of 2026-11-23 |

## Problem Statement

1. A bot learns nothing. `docs/product.md:13` promises that a bot "keeps what it learns from conversations, builds skills from the tasks it completes, and improves them with use". No code does any of it: memory, skills, learning, scheduled work, delegation, and usage accounting have no line in `crates/crowbotd` (audit, section "Comparaison sous-système par sous-système").
2. The daemon has no channel to give a bot a tool. Each session opens with no MCP server: `crates/crowbotd/src/provider/acp.rs:921` builds `NewSessionRequest::new(cwd)`, whose `mcp_servers` defaults to an empty list (`agent-client-protocol-schema-1.17.0/src/v1/agent.rs:1302`). MCP (Model Context Protocol) is the protocol through which a program offers tools to an agent.
3. The daemon watches a turn but does not guard it. It sees each tool call (`acp.rs:1158-1197`) and has no check for a bot that repeats the same call, no budget, and no watchdog for a turn that went silent. The usage figures the agents send are dropped (`acp.rs:1196`).
4. Approvals have two settings and nothing between them. A bot either asks for every gated call or is allowed everything (`crates/crowbot-protocol/src/bot.rs:21-29`). There is no rule that refuses a catastrophic command on Full access, and no rule that allows a known safe one on Ask first.
5. A bot works only while someone talks to it. There is no timer besides the save, the stop, and the sign-in polling (audit, same section), so a bot cannot run a routine.

The reference for all five is Hermes Agent (`/home/arthur/dev/hermes-agent`, commit `bb89ad1`, MIT license), which the audit mapped subsystem by subsystem.

**Why now:** The desktop core and the Linux port are done (`tasks/prd-crow-bot-desktop-core-status.json`, `tasks/prd-crow-bot-linux-port-status.json`, both `DONE`). `docs/product.md:58-59` names memory and skills as the next step, then scheduled work. The audit of 2026-11-23 settled what to take from Hermes Agent and where the daemon can do better, so the work can be specified from evidence.

## Overview

This PRD gives the daemon a harness: the layer that makes a bot more than a provider session. The daemon stays above the provider agents. It does not call a model API and does not run the agent loop; it serves tools to the agent, guards the turn from outside, and starts turns of its own under rules the user sets.

**The audit** is the document "Crow Bot face à Hermes Agent : audit du harness" (https://claude.ai/artifact/RYcNSg8XjwQNwtxcqWTuSS). It is not in the repository, so each story repeats the Hermes Agent paths it needs and can be built without opening the audit. Every story cites the audit section it comes from and the Hermes Agent code it adapts. Paths that start with `hermes-agent/` are under `/home/arthur/dev/hermes-agent` at commit `bb89ad1`. The implementing agent reads them before it designs. Hermes Agent is a reference for ideas (`AGENTS.md`, "Reference projects"): the design is written for this codebase, in Rust, and no Python is translated line by line.

What is taken from Hermes Agent is what acts above the model call. What acts around the model call (API error recovery, context compression, prompt cache markers, tool call repair) stays with the provider agent (audit, sections "À reprendre : boucle d'agent" and "À reprendre : prompt et contexte").

Key decisions. Each one settles a decision the audit left open (audit, section "Conflits avec AGENTS.md : décisions à prendre"); the options and the evidence are in "Technical Decisions".

- **D1. Daemon tools declare an effect, and the daemon gates them where it executes them.** A read or a write to the bot's own versioned state runs without a person. A tool that reaches outside that state asks.
- **D2. An unattended approval waits.** A routine that needs an answer shows as waiting; its next occurrence is recorded as skipped. No approval is granted or refused by a timer.
- **D3. Approval rules are typed, per bot, and written by a person only.** Refusal floors are built in. Allow rules match a parsed command, never a pattern over text.
- **D4. A turn the daemon starts has an origin, a switch, and a daily limit.** Every origin but `Retry` is off until the user turns it on for a bot or approves the action that needs it.
- **D5. The database holds memory and skills.** Skill files on disk are a projection the daemon owns and checks by hash.
- **D6. Skills reach every provider through MCP only.**
- **D7. Memory and the skills index are frozen when a session opens and stay out of the session fingerprint.** Changes travel in a note the daemon puts before the user's message.
- **D8. Delegation and goals are in scope**, after memory, skills, and routines.
- **D9. No provider runs in a wider mode to get daemon tools.** A provider that cannot call them in its ask-first mode runs without them, and the client says so.
- **D10. The harness does not depend on who owns the agent loop.** A native loop is a later PRD and plugs in as one more ACP agent.

The work is phased, because it holds 46 stories: EP-011 keeps the client reading the protocol, so it runs in phase A although it is numbered late. US-028 (the curator) spares the skills a routine names, so it waits for US-031. An epic's priority in the status file is the highest priority among its stories.

| Phase | Epics | Stories | Outcome |
|-------|-------|---------|---------|
| A. Channel and guard | EP-001, EP-011, EP-002, EP-003 | 17 | A bot can call daemon tools on each provider; the client shows every new event; the daemon records usage, stops a turn that makes no progress, and applies approval rules |
| B. Memory and skills | EP-004, EP-005, EP-006 without US-028 | 12 | A bot remembers, searches its past conversations, writes and reuses skills, and reviews its work when the user allows it |
| C. Work without the user | EP-007, EP-008, US-028 | 9 | A bot runs routines on a schedule, and every file it edits can be put back |
| D. Several conversations at once | EP-009, EP-010 | 5 | A bot delegates to a child conversation and pursues a goal over several turns |
| E. Desktop surfaces | EP-012 | 3 | The client manages what phases A to D store |

## Goals

| Goal | Target at the end of phase B | Target at the end of phase E |
|------|------------------------------|------------------------------|
| Stories certified `DONE` by `/review-epic` | 29 | 46 |
| Providers on which a bot calls a daemon tool in a real turn | 3, or each exception recorded by US-001 | same |
| Kinds of durable state a bot keeps across sessions (memory, skills, routines, checkpoints, goals) | 2 | 5 |
| Daemon-started turns that run without a matching switch turned on | 0 | 0 |

## Target Users

### Solo builder
- **Role:** A developer or founder who pays for one or more AI plans and runs several bots on their own computer.
- **Pain points:** Tells each bot the same facts in every conversation. Repeats the same procedure by hand. Cannot leave a bot a task to do every morning.
- **Success looks like:** A bot recalls a preference stated a week ago and names the conversation it came from. A procedure done twice becomes a skill the bot loads the third time. A routine reports at 9:00 without a window open.

### Small team owner
- **Role:** Runs the daemon on a Linux server for a team.
- **Pain points:** Cannot give a bot Full access, because nothing refuses a destructive command. Cannot see what the bots consume.
- **Success looks like:** Full access with refusal rules that no bot can change. A usage figure per bot and per origin.

## Research Findings

All findings come from the audit and from the code it cites. No web research was run for this PRD: the feature is specified from two codebases, and the three facts that only a live provider can settle are the subject of US-001.

- **The loop belongs to the provider.** The daemon sends one prompt and waits (`crates/crowbotd/src/provider/acp.rs:1009-1044`). Hermes Agent owns its loop (`hermes-agent/agent/conversation_loop.py:1724-1726`).
- **ACP already carries MCP servers.** `NewSessionRequest` and `ResumeSessionRequest` hold `mcp_servers`, and stdio is the transport every agent must support (`agent-client-protocol-schema-1.17.0/src/v1/agent.rs:1255`, `:1465`, `:4118-4120`).
- **ACP already carries usage.** `UsageUpdate{used, size, cost}` (`agent-client-protocol-schema-1.17.0/src/v1/client.rs:1892-1901`).
- **Hermes Agent keeps its prompt fixed for a session** and sends later changes in a user message or a tool result (`hermes-agent/agent/AGENTS.md:57-63`).
- **Hermes Agent's safety rests on patterns inside the agent's process**, with a documented hole: a headless local session approves a whole script (`hermes-agent/tools/approval.py:1480-1490`). The daemon's approvals are outside the agent's process (`crates/crowbotd/src/provider/acp.rs:847-884`).
- **Hermes Agent spreads state over many files** and needs locks, leases, and heartbeats to share them (`hermes-agent/cron/jobs.py:99`, `hermes-agent/cron/scheduler.py:4058-4070`, `hermes-agent/cron/constants.py:9`). The daemon is the only writer of one database (`crates/crowbotd/src/instance.rs:60`).
- **Hermes Agent's defaults contradict its own documents** in three places the audit checked (audit, section "Où Crow Bot peut faire mieux", row "Constantes typées"). No default is copied without its code line.

## Assumptions & Constraints

### Assumptions (to validate)

- **A1 (high risk).** Each provider agent accepts a stdio MCP server through `mcp_servers` and lets the model call its tools in the session mode that asks first. Not observed. Validated by US-001.
- **A2 (high risk).** Each provider agent reports a call to a daemon tool in a way that names the daemon's server, in the permission request or in the tool call fields. Not observed. Validated by US-001.
- **A3.** Each provider agent asks for permission, with the file path as the target, before it writes a file. Nothing in this PRD rests on it any longer: checkpoints snapshot the folder around a turn (EP-008). Recorded by US-001.
- **A4.** The `jiff` crate gives time zone arithmetic with the system time zone database on Linux and a bundled one on Windows, under a license the daemon may link (not GPL). Not checked in its documentation. US-030 checks it before it adds the dependency and names the fallback.
- **A5.** A model loads a skill when the index tells it to. Hermes Agent relies on the same instruction (`hermes-agent/agent/prompt_builder.py:1349-1362`). Measured by US-029.
- **A6.** Each provider agent sends the output of a tool call in its updates. The no-progress guard (US-008) compares outputs; where an agent sends none, the guard compares titles and inputs only, which US-008 states. Recorded by US-001.
- **A7.** Some provider agents report tokens per turn (`PromptResponse.usage`) or a session cost. The daily limit does not depend on it (D4). Recorded by US-001.
- **A8.** A process that runs as the user's account can connect to a session's bridge endpoint, as it can to the daemon's own endpoint. This is the boundary `AGENTS.md` already states for runtimes: the approvals are the boundary, not the endpoint.
- **A9.** A provider's permission request for a command carries the folder the command runs in. An agent may keep its own current folder between calls, so the daemon cannot assume the bot's folder. Recorded by US-001; where a provider does not send it, no allow rule applies to that provider's commands (D3).
- **A10.** The `ignore` crate reads `.gitignore` files under a license the daemon may link (not GPL). Not checked in its documentation. US-036 checks it before it adds the dependency; the fallback is the daemon's own reader of `.gitignore` patterns.
- **A11.** The context size an agent reports (`UsageUpdate.used`) falls when the agent compacts its context. Inferred from the field's definition, not observed. Recorded by US-001; US-020 rests on it.

### Hard Constraints

- The hard rules of `AGENTS.md`, as amended on 2026-11-23 for decisions D1 to D10.
- The daemon and the protocol crate must not depend on any GPL crate (`AGENTS.md`, "License boundary").
- A stored event is the JSON of the protocol `Event`: fields are added with `#[serde(default)]`, and no variant is renamed or removed (`AGENTS.md`, "The database is the source of truth").
- Schema changes go through `PRAGMA user_version` and keep all user data.
- Provider binaries run unmodified, and the daemon never reads provider credentials (`AGENTS.md`, "Model usage belongs to the user").
- No bundled skill, catalog, or download of skills: distribution waits for the product (`AGENTS.md`, "Product").

## Technical Decisions

### D1. Approvals of daemon tools

- **Options:** (a) every daemon tool asks like any other tool; (b) the daemon allows its read tools; (c) the daemon also allows writes to the bot's own state.
- **Choice:** (c), made strict by types. A daemon tool declares one `Effect`: `Read`, `BotState` (a write to state the daemon versions and can put back: memory, skills), or `External` (anything else: a routine, a delegation, a script, a restore). When an agent asks permission to call a daemon tool, the daemon selects the allow-once option and records `ApprovalReason::DaemonTool`. The real gate is inside the tool call, where the daemon executes it: an `External` tool raises an ordinary approval request and waits for it; the bot's permission level and the approval rules apply to that request. Every call needs a permit of the turn that runs, which the conversation gives while a turn runs and no stop is asked, so no daemon tool runs between turns or after a stop. A `BotState` tool returns its change as a `Mutation`, which the conversation applies in its own loop, in one transaction with its event, only while that turn still runs: the check and the write cannot be split by a stop.
- **Evidence:** Under Grok every MCP call asks (`crates/crowbotd/src/provider/grok.rs:18`), so option (a) makes a person approve each memory read. Hermes Agent runs these tools inside the loop with no prompt (`hermes-agent/model_tools.py:632`). Gating at the point of execution does not depend on how an agent labels a call, which A2 has not confirmed.
- **Fallback:** where US-001 finds that a provider does not name the daemon's server in its permission request, that provider's daemon tool calls ask (option a), and `docs/harness-providers.md` says so.

### D2. Approvals with nobody there

- **Options:** (a) wait; (b) refuse after a delay; (c) a permission level of the routine.
- **Choice:** (a). The request waits, the bot shows as waiting, and the next occurrence of that routine is recorded as skipped while the run waits.
- **Evidence:** Hermes Agent refuses (`hermes-agent/tools/approval_context.py:369-381`) because it has no durable place to wait and no client to reach. The daemon has both: a pending approval and the `Waiting` state (`crates/crowbot-protocol/src/bot.rs:205-217`). Option (b) needs a delay no evidence supplies. Option (c) adds a second source for the permission level, which `AGENTS.md` forbids.
- **What the daemon still declines on its own:** only what it cannot let run, each with its own `ApprovalReason`: a request with no allow-once option (today's `NoAllowOnce`), a request whose record could not be written (US-016), and a tool that is not the daemon's in a review session (US-027).

### D3. Durable approval rules

- **Options:** (a) stay with two levels; (b) add refusals only; (c) add refusals and allow rules.
- **Choice:** (c). Built-in refusal floors apply to every bot. Per-bot rules, refuse or allow, live in the database and are written through a client message only: no daemon tool, prompt, or file can add one. A refuse rule wins over an allow rule and over Full access. An allow rule matches a parsed simple command: the same program, and an argument list that starts with the rule's arguments. A request matches only when it carries the folder the command runs in and that folder is inside the bot's folder (A9); a command the parser cannot reduce to one simple command, or a target that was cut, never matches. The daemon still answers with the allow-once or the reject-once option, and records the rule as the reason.
- **Evidence:** Hermes Agent's floors and scopes (`hermes-agent/tools/approval.py:1310-1333`, `:504-517`). Its allowlist skips commands with compound operators for the same reason (`hermes-agent/tools/approval_floors.py:173-243`). Its detection needs a normalization pass against obfuscation because it matches text (`hermes-agent/tools/approval_detection.py:938-961`); matching a parsed command removes that class of bypass for allow rules.

### D4. Turns the daemon starts

- **Options:** (a) off by default, turned on per bot; (b) on by default with a visible budget; (c) on request only.
- **Choice:** (a), with a ledger. Each turn records its origin: `User`, `Routine`, `Review`, `Goal`, `Delegation`, `Retry`, or `Budget`. Each bot has a switch per origin and a daily limit on daemon-started turns, counted in turns over the daemon's local day. A turn whose origin is off, or whose limit is reached, does not start, and the refusal is recorded. A switch other than `Retry` can be turned on only when the daily limit is set, so no default limit is invented.
- **Why turns and not tokens:** the usage ACP reports is not a spend. `UsageUpdate.used` is "Tokens currently in context" and `cost` is cumulative for the session (`agent-client-protocol-schema-1.17.0/src/v1/client.rs:1892-1901`); tokens per turn exist only in `PromptResponse.usage`, behind the unstable feature `unstable_end_turn_token_usage` (`agent-client-protocol-schema-1.17.0/src/v1/agent.rs:4687-4691`, `:4780-4786`). A turn is the one unit every provider gives. A daily token limit is offered beside it and applies only to turns whose provider reported tokens (A7).
- **Evidence:** The usage is the user's (`AGENTS.md`, "Model usage belongs to the user"). Hermes Agent's own note puts one review at about 30,000 tokens (`hermes-agent/cron/scheduler.py:2472`).
- **Defaults:** `Retry` is on: it repeats a turn the user asked for, as Hermes Agent retries by default (`hermes-agent/agent/agent_init.py:1275`). `Review` and `Budget` are off. `Routine`, `Goal`, and `Delegation` are off, and each is turned on by the user's own act: the approval of a `schedule` or `delegate` call, or the client message that creates a routine or sets a goal. That act asks for the daily limit when none is set.

### D5. Where memory and skills live

- **Options:** (a) the database, with files written on demand; (b) files in the bot's folder; (c) files in a folder the daemon owns.
- **Choice:** (a), with the projection of (c). Memory entries and skill versions are rows; skill content is stored by SHA-256. Before each session the daemon writes the bot's active skills under `<data>/skills/<bot>/<skill>/`, read-only, and checks every file against its hash, the way `runtime.rs` checks an installed runtime.
- **Evidence:** `AGENTS.md`, "The database is the source of truth". A bot can write any file in its folder, so option (b) lets a bot rewrite a skill outside the scan and the history. Hermes Agent keeps files and needs a ledger beside them to get history back (`hermes-agent/tools/skill_ledger.py:1-8`).

### D6. How skills reach an agent

- **Options:** (a) MCP only; (b) the daemon also writes skills into each provider's own skills folder.
- **Choice:** (a). One path for three providers, and the rule that a provider home holds no skills stays whole (`crates/crowbotd/src/provider/codex.rs:27-43`, `crates/crowbotd/src/provider/grok.rs:32-50`).
- **Evidence:** Hermes Agent itself serves `skill_view` and `skills_list` over MCP when it drives Codex (`hermes-agent/agent/transports/hermes_tools_mcp_server.py:57-67`). A5 is the risk; US-029 measures it.

### D7. Memory, skills, and the session fingerprint

- **Options:** (a) frozen at session open, outside the fingerprint; (b) inside the fingerprint; (c) out of the system prompt entirely.
- **Choice:** (a). The versions of memory and of the skills index that a session saw are stored with the provider session. Each later prompt carries, in the daemon's note, what changed since those versions.
- **Evidence:** Hermes Agent's frozen snapshot (`hermes-agent/tools/memory_tool_store.py:655-658`) and its per-turn difference channel (`hermes-agent/agent/skills_index_delta.py:1-14`). Option (b) closes the session on every memory write (`crates/crowbotd/src/store/sessions.rs:31-48`). Option (c) loses the standing instruction to load a skill.

### D8. Delegation and goals

- **Choice:** In scope, as phase D. A delegated task is a child conversation, so its approvals reach the client like any other (audit, section "À reprendre : délégation et sous-agents").
- **Evidence:** They were non-goals of the desktop core only (`tasks/prd-crow-bot-desktop-core.md:877-880`). `docs/product.md:58-59` orders memory and skills first, which the phases follow.

### D9. Providers that cannot call daemon tools in their ask-first mode

- **Choice:** Fail closed. The bot runs on that provider without daemon tools; the provider's entry names the cause; no session mode is widened. The open decision on Codex's mode (`AGENTS.md`, "Open decisions") stays separate.
- **Evidence:** `AGENTS.md`, "The daemon owns approvals": a turn already fails when an agent does not offer the mode that asks first.

### D10. Who owns the agent loop

- **Choice:** The harness is written against the daemon's own events and tools, never against a provider. A loop written in Rust in this repository is a later PRD; it would be one more ACP agent, for API keys only.
- **Evidence:** `AGENTS.md`, "Model usage belongs to the user": a subscription can only be used through the provider's unmodified binary. What that costs is listed in the audit (section "Où Crow Bot peut faire mieux", last paragraph).

### Other decisions

- **The MCP server is written in this repository**, with no MCP library: JSON-RPC 2.0 over stdio with `initialize`, `tools/list`, and `tools/call`. The daemon already writes its own protocol code where the surface is small (`systemd.rs`, `AGENTS.md`, "Supervision on Linux"). MCP types stay in that module, as ACP types stay in the adapter.
- **The bridge is the daemon's own program** started with `--mcp-bridge`, as `--guard-agent` and the sign-in page are (`AGENTS.md`, "Daemon" and "Providers"). It holds no logic: it relays each line to the daemon over an endpoint the daemon creates for that session. The session's identity comes from the endpoint.
- **A checkpoint is a snapshot of the bot's folder around a turn, taken by the daemon with no git program.** Before and after each turn the daemon stores a manifest (path, size, SHA-256 of each file) and keeps each new content once, by hash. Hermes Agent snapshots the folder with git before a file-mutating tool (`hermes-agent/agent/checkpoints/manager.py:80-83`, `hermes-agent/agent/tool_executor.py:106-125`). A copy taken when an edit is approved was considered and dropped: it rests on A3, and it misses every file a command changes. A snapshot that fails does not stop the turn: a checkpoint is a way back, not a gate.
- **The handoff between providers keeps its present form.** A summary written by the provider a bot leaves would need a turn on a provider that is often the reason for leaving (signed out, usage limit). Memory and skills now follow the bot across providers instead.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting of the root workspace (daemon and protocol)
- `cargo clippy --locked --all-targets --no-deps -- -D warnings` - lints of the root workspace, with no warning
- `cargo test --locked` - tests of the root workspace
- `cargo fmt --all --check --manifest-path crates/crowbot-desktop/Cargo.toml` - formatting of the client workspace
- `cargo clippy --locked --all-targets --no-deps --manifest-path crates/crowbot-desktop/Cargo.toml -- -D warnings` - lints of the client workspace, with no warning
- `cargo test --locked --manifest-path crates/crowbot-desktop/Cargo.toml` - tests of the client workspace

For the stories of EP-012, additional gates:
- Start `crowbotd` on a test database and a test endpoint, start the client against it, and capture a screenshot of each state the story names with the `/agent-desktop` skill.
- Compare each screenshot with `DESIGN.md`: regions, sizes, radii, and states must match. A change that departs from `DESIGN.md` updates it in the same commit.

Unless a story says otherwise, "a test" means a test of `crates/crowbotd/tests/daemon` that starts the real daemon, attaches through the protocol, and drives `examples/fake_acp_agent.rs`.

A story that adds an `Event` variant or field also adds its row in the client's transcript (`crates/crowbot-desktop/src/transcript.rs`) and a client test that opens a conversation holding it. US-043 sets the pattern.

## Epics & User Stories

### EP-001: The daemon serves tools to a bot

A bot on any provider can call a tool the daemon executes, and the daemon gates that call itself.

**Definition of Done:** In a test, the fake agent lists and calls a daemon tool through the bridge; an `External` tool waits for an approval a client answers; and `docs/harness-providers.md` records, for Claude, ChatGPT, and Grok, the answers to the three questions of US-001.

#### US-001: Record how each provider treats a daemon MCP server
**Description:** As the product owner, I want the behavior of each real agent written down before the design rests on it, so that assumptions A1 and A2 are facts and A3, A6, A7, A9, and A11 are recorded.

**Priority:** P0
**Dependencies:** None

**References:** Audit, section "Non confirmé", last three rows. `agent-client-protocol-schema-1.17.0/src/v1/agent.rs:1255`, `:4099-4121`. `crates/crowbotd/src/provider/grok.rs:18`.

**Acceptance Criteria:**
- [ ] Given a signed-in Claude, ChatGPT, and Grok, when each runs a turn with a throwaway stdio MCP server (a script kept under `scripts/`, not the bridge of US-002) that offers one read tool and one write tool, then `docs/harness-providers.md` states for each provider whether the model could call each tool in the ask-first mode
- [ ] Given the same turns, when a tool is called, then the document quotes the fields of the permission request and of the tool call update that name the server and the tool, or states that none does
- [ ] Given a turn that edits a file on each provider, when the edit is requested, then the document states whether a permission request arrived before the write and what its target held
- [ ] Given a provider that could not be signed in on the test computer, when the document is written, then that provider's rows say "not observed" with the reason, and the document gives it the degraded mode of D9 until it is observed
- [ ] Given the same turns, when they end, then the document states for each provider whether tool call updates carried an output, whether `UsageUpdate` arrived, and whether the prompt response carried tokens per turn
- [ ] Given a turn that runs a command after a `cd` on each provider, when the request arrives, then the document states whether the request carried the folder the command runs in
- [ ] Given a long turn on each provider that makes the agent compact its context, when it does, then the document states whether `UsageUpdate.used` fell and by how much
- [ ] Given the document, when a provider fails A1, then the document names the degraded mode of D9 as that provider's behavior

#### US-002: The bridge and the per-session endpoint
**Description:** As the daemon, I want a relay process an agent can start as an MCP server, so that a tool call reaches me with the identity of its session.

**Priority:** P0
**Dependencies:** None

**References:** Audit, section "À reprendre : outils et approbations", first two paragraphs. `crates/crowbotd/src/sign_in_page.rs` (the same pattern). `hermes-agent/agent/transports/hermes_tools_mcp_server.py:1-5`.

**Acceptance Criteria:**
- [ ] Given a session start, when the daemon builds `NewSessionRequest` or `ResumeSessionRequest`, then `mcp_servers` holds one stdio server whose command is the daemon's own program with `--mcp-bridge`
- [ ] Given a running bridge, when the agent sends `initialize`, `tools/list`, or `tools/call`, then the agent receives the daemon's answer to that request
- [ ] Given two sessions of two bots, when each bridge calls a tool, then each call is attributed to its own bot and conversation
- [ ] Given a process that runs as another account, when it connects to a session endpoint, then the connection is dropped before any request is read
- [ ] Given a session that ended, when its bridge sends a request, then the request fails with a JSON-RPC error and no tool runs
- [ ] Given the daemon is killed, when a bridge is running, then the bridge exits

#### US-003: The tool registry and the effect of a tool
**Description:** As a developer, I want each daemon tool declared with a typed input and one effect, so that what a tool may do is decided at compile time.

**Priority:** P0
**Dependencies:** Blocked by US-002

**References:** Audit, section "À reprendre : outils et approbations", rows "Registre d'outils" and "Outils réservés à la boucle". `hermes-agent/tools/registry.py:840-906`. `hermes-agent/model_tools.py:632`.

**Acceptance Criteria:**
- [ ] Given a session, when `tools/list` is answered, then each tool has a name, a description, and a JSON schema generated from its Rust input type
- [ ] Given a tool call, when its `ToolCallStarted` event is stored, then the event names the tool's effect: `Read`, `BotState`, or `External`
- [ ] Given a session kind (`Conversation`, `Routine`, `Review`, `Delegated`), when `tools/list` is answered, then it holds only the tools that kind may call
- [ ] Given a `tools/call` whose arguments do not match the schema, when it arrives, then the tool does not run and the answer names the first invalid field
- [ ] Given a `tools/call` for a tool the session kind may not call, when it arrives, then it fails with an error and an event records the refusal
- [ ] Given a tool result larger than `TOOL_OUTPUT_BYTES` (64 KiB, `crates/crowbot-protocol/src/event.rs:91`), when it is returned, then the agent receives a preview and a cursor to read the rest

#### US-004: Approvals of daemon tools
**Description:** As a user, I want a daemon tool to ask only when it reaches outside the bot's own state, so that a bot reads its memory without a prompt and cannot schedule work without me.

**Priority:** P0
**Dependencies:** Blocked by US-001, US-003

**References:** Decision D1. `crates/crowbotd/src/provider/acp.rs:847-884`. `crates/crowbotd/src/conversation.rs:857-907`. `crates/crowbot-protocol/src/event.rs:150-155`.

**Acceptance Criteria:**
- [ ] Given an agent that asks permission to call a daemon tool, when the request names the daemon's server, then the daemon selects the allow-once option and records `ApprovalResolved` with the reason `DaemonTool`
- [ ] Given a `Read` or `BotState` tool, when it is called, then it runs with no approval request
- [ ] Given an `External` tool on an Ask first bot, when it is called, then an `ApprovalRequested` event is recorded and the tool call does not return before a client answers
- [ ] Given an `External` tool on a Full access bot, when it is called, then it runs and `ApprovalResolved` carries the reason `PermissionLevel`
- [ ] Given a client that declines the request, when the tool call returns, then the agent receives an error that says a person declined, and nothing was changed
- [ ] Given a turn that is stopped while an `External` tool waits, when the stop is applied, then the request closes as `Cancelled` and the tool does not run
- [ ] Given a provider recorded by US-001 as not naming the server, when its agent asks permission for a daemon tool, then the request waits for a person like any other
- [ ] Given a signed-in provider that US-001 recorded as passing A1 and the environment flag that enables real-provider checks, when a real turn asks for a daemon tool, then the tool runs through the bridge and `docs/harness-providers.md` records the run

#### US-005: The system prompt in tiers and the daemon's note
**Description:** As a bot, I want what the daemon knows placed where the provider's prompt cache keeps it, so that a change in memory does not cost a new session.

**Priority:** P0
**Dependencies:** None

**References:** Decision D7. Audit, section "À reprendre : prompt et contexte". `hermes-agent/agent/system_prompt.py:770-845`. `hermes-agent/agent/skills_index_delta.py:1-14`. `hermes-agent/agent/memory_context.py:170-186`. `crates/crowbotd/src/provider/acp.rs:895-905`, `:1002-1008`.

**Acceptance Criteria:**
- [ ] Given a new session and two registered sections, when the system prompt is built, then it holds the bot's instructions first and the sections after them in the order of their registration rank
- [ ] Given a bot whose registered sections change, when the session fingerprint is computed, then it is the same as before the change
- [ ] Given a provider session, when it opens, then the version of each registered section it saw is stored with it
- [ ] Given a later prompt on that session, when a registered section has a newer version, then the prompt carries one note block before the user's text that holds the difference
- [ ] Given a note block, when the `UserText` event is stored, then the note is stored in a field of that event added with `#[serde(default)]`
- [ ] Given an event stored before this story, when it is read, then it deserializes with an empty note
- [ ] Given an agent that takes no system prompt, when a new session starts, then the sections follow the instructions in the preface of the first prompt

---

### EP-002: The daemon guards a turn

The daemon measures what a turn consumes and ends a turn that stopped making progress.

**Definition of Done:** Tests show a turn stopped for no progress, a turn stopped for silence, a failed turn retried on the ladder, a paused daemon that starts no turn, and the usage of a turn stored and sent to a client.

#### US-006: Store the usage an agent reports
**Description:** As a user, I want the context size, the tokens, and the cost of each turn stored when the agent reports them, so that I can see what a bot consumes.

**Priority:** P0
**Dependencies:** None

**References:** Audit, section "À reprendre : mémoire et recherche de sessions", row "Comptabilité d'usage". `agent-client-protocol-schema-1.17.0/src/v1/client.rs:1892-1901`. `agent-client-protocol-schema-1.17.0/src/v1/agent.rs:4687-4691`, `:4780-4786` (feature `unstable_end_turn_token_usage`, which this story enables in `crates/crowbotd/Cargo.toml`). `crates/crowbotd/src/provider/acp.rs:1196`. `hermes-agent/persistence/sqlite/_schema_ddl.py:230-250`.

**Acceptance Criteria:**
- [ ] Given an agent that sends `UsageUpdate`, when the turn ends, then one row holds the conversation, the turn, the provider, the last context size (`used` of `size`), and the session cost when one was sent
- [ ] Given a prompt response that carries tokens per turn, when the turn ends, then the row also holds the input, output, and total tokens of that turn
- [ ] Given a session cost that grew during the turn, when the turn ends, then the row holds the growth as the turn's cost
- [ ] Given a stored usage, when a client opens the conversation, then the snapshot carries the usage of its last turn
- [ ] Given an agent that never sends `UsageUpdate`, when the turn ends, then no row is written and the snapshot says the usage is unknown
- [ ] Given a schema older than this story, when the daemon opens it, then the migration adds the table and keeps every row

#### US-007: The origin of a turn
**Description:** As a user, I want each turn to say who started it, so that I can tell my requests from the daemon's.

**Priority:** P0
**Dependencies:** None

**References:** Decision D4.

**Acceptance Criteria:**
- [ ] Given a message a client sends, when its `UserText` event is stored, then its origin is `User`
- [ ] Given a `UserText` stored before this story, when it is read, then its origin is `User`
- [ ] Given a turn the daemon starts, when the event is stored, then its origin is one of `Routine`, `Review`, `Goal`, `Delegation`, `Retry`, `Budget`
- [ ] Given a client message that names an origin other than `User`, when it arrives, then the daemon ignores the field and stores `User`

#### US-008: Stop a turn that makes no progress
**Description:** As a user, I want a bot that repeats the same call stopped, so that it does not spend my plan in a loop.

**Priority:** P1
**Dependencies:** None

**References:** Audit, section "À reprendre : boucle d'agent", row "Garde contre le sur-place". `hermes-agent/agent/tool_guardrails.py:28-39`, `:121`, `:471-485`.

**Acceptance Criteria:**
- [ ] Given a turn, when 5 consecutive tool calls have the same title, input, and output, then the daemon stops the turn and records `TurnInterrupted` with the cause `NoProgress` (5 is `no_progress_block_after`, `tool_guardrails.py:121`)
- [ ] Given a turn that repeats a cycle of 2 to 4 calls with identical inputs and outputs, when the cycle has run 5 times, then the turn is stopped the same way (`tool_guardrails.py:36-37`)
- [ ] Given calls that share a title and an input and differ in output, when they repeat, then the turn is not stopped
- [ ] Given a turn stopped for no progress, when the queue holds messages, then the queue is paused
- [ ] Given an agent that sends no output for its tool calls, when 5 consecutive calls have the same title and input, then the turn is stopped the same way

#### US-009: Limits of a turn
**Description:** As a user, I want a turn that went silent stopped and a turn that used its budget ended, so that a stuck agent does not hold a bot for hours.

**Priority:** P0
**Dependencies:** Blocked by US-007

**References:** Audit, section "À reprendre : boucle d'agent", row "Budget de tour"; section "À reprendre : tâches planifiées", row "Garde d'inactivité". `hermes-agent/cron/scheduler.py:1190-1201`. `hermes-agent/agent/turn_finalizer.py:138-164`.

**Acceptance Criteria:**
- [ ] Given a turn of origin other than `User`, when no agent event arrives for 600 seconds, then the daemon stops the turn and records `TurnInterrupted` with the cause `Inactive` (600 s is `_cron_inactivity_seconds`, `scheduler.py:1196`)
- [ ] Given a turn that waits for an approval, when the 600 seconds pass, then the turn is not stopped
- [ ] Given a client message that sets a bot's tool call limit per turn, when it is applied, then every attached client receives the limit
- [ ] Given a bot with a tool call limit per turn, when a turn reaches it, then the daemon stops the turn and records the cause `BudgetExhausted`
- [ ] Given a bot with no limit set, when a turn runs, then no tool call limit applies

#### US-010: Retry a turn that failed before it said anything
**Description:** As a user, I want a turn that failed on a passing outage tried again, so that I do not resend the same message.

**Priority:** P1
**Dependencies:** Blocked by US-007

**References:** Audit, section "À reprendre : boucle d'agent", last paragraph. `hermes-agent/agent/turn_recovery_autorecover.py:5-8`, `:22-26`. `hermes-agent/agent/agent_init.py:1275`.

**Acceptance Criteria:**
- [ ] Given a turn that ends in `TurnFailed` with the cause `Other` and no `MessageText` and no tool call, when it ends, then the daemon sends the same prompt again after 15 seconds, as a turn of origin `Retry`
- [ ] Given repeated failures of that kind, when retries follow, then they wait 15, 30, then 60 seconds, and stop after 3 attempts (`turn_recovery_autorecover.py:23-24`, `agent_init.py:1275`)
- [ ] Given a failed turn that recorded a `MessageText` or a tool call, when it ends, then no retry is sent
- [ ] Given a failure with the cause `SignInRequired` or `UsageLimit`, when it ends, then no retry is sent
- [ ] Given a retry that is waiting, when the user stops the conversation or sends a message, then the retry is dropped

#### US-011: Pause all new work
**Description:** As a user, I want one switch that stops every new turn, so that I can halt my bots without killing what runs.

**Priority:** P1
**Dependencies:** None

**References:** Audit, section "À reprendre : boucle d'agent", row "Arrêt d'urgence". `hermes-agent/agent/estop.py:1-8`.

**Acceptance Criteria:**
- [ ] Given a client message that pauses the daemon, when it is applied, then `DaemonInfo` reports the daemon as paused
- [ ] Given a paused daemon, when a turn of any origin other than `User` is due, then it does not start
- [ ] Given a paused daemon, when a turn is already running, then it runs to its end
- [ ] Given a file named `PAUSED` in the data folder at start, when the daemon starts, then it is paused
- [ ] Given a paused daemon that restarts, when it is ready, then it is still paused
- [ ] Given a client message that resumes the daemon, when it is applied, then the `PAUSED` file is removed if it exists
- [ ] Given a `PAUSED` file that cannot be removed, when a client resumes the daemon, then the daemon stays paused and the answer names the file

---

### EP-003: Approval rules

A person can refuse some commands for good and allow some known ones, and no bot can change either list.

**Definition of Done:** Tests show a built-in floor refused on a Full access bot, an allow rule answered without a person on an Ask first bot, a compound command that no allow rule matches, and a rule that no daemon tool can create.

#### US-012: A cut request is never allowed
**Description:** As a user, I want a request nobody could read in full never allowed, by the daemon or by a person, so that Full access does not approve what nobody could see.

**Priority:** P0
**Dependencies:** None

**References:** Audit, section "Conflits avec AGENTS.md", item "Demande coupée en accès complet". `crates/crowbotd/src/conversation.rs:875-886`, `:443-448`, `:943-945`.

**Acceptance Criteria:**
- [ ] Given a Full access bot, when a request arrives whose target was cut, then the daemon does not allow it and records `ApprovalRequested`
- [ ] Given that request, when a client answers, then only a decline is accepted
- [ ] Given a Full access bot, when a request arrives whose target was not cut, then the daemon allows it as before

#### US-013: Parse a command
**Description:** As the daemon, I want the target of a command request reduced to a simple command or marked opaque, so that rules match structure and not text.

**Priority:** P0
**Dependencies:** None

**References:** Decision D3. `hermes-agent/tools/approval_detection.py:938-961`, `:1067-1077`. `hermes-agent/tools/approval_floors.py:173-243`.

**Acceptance Criteria:**
- [ ] Given a target such as `cargo test --locked`, when it is parsed, then the result is one simple command with a program and a list of arguments
- [ ] Given a target with a pipe, a list operator, a redirection, a substitution, a subshell, a here-document, or an `eval`, when it is parsed, then the result is `Opaque`
- [ ] Given a target with ANSI-C quoting, a line continuation, or an escaped character in the program name, when it is parsed, then the program is the decoded name
- [ ] Given a target that was cut at `APPROVAL_TARGET_BYTES` (16 KiB, `crates/crowbot-protocol/src/event.rs:93`), when it is parsed, then the result is `Opaque`
- [ ] Given any byte string, when it is parsed in a property test, then the parser returns and never panics

#### US-014: Built-in refusals
**Description:** As a user, I want commands that destroy the computer refused whatever the level, so that Full access has a floor.

**Priority:** P0
**Dependencies:** Blocked by US-013

**References:** Audit, section "À reprendre : outils et approbations", row "Planchers infranchissables". `hermes-agent/tools/approval.py:1310-1333`. `hermes-agent/tools/approval_detection.py:176-218` (root, drive root, system folders, home, `mkfs`, `dd`, shutdown). The list of protected folders is this PRD's own.

**Acceptance Criteria:**
- [ ] Given a request to delete recursively one of the protected folders, when it arrives on a Full access bot, then the daemon selects the reject-once option and records `ApprovalResolved` as declined with the reason `Floor`
- [ ] Given the protected folders, when the list is read by the test, then it holds `/`, `/etc`, `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/boot`, `/var`, `/sys`, `/proc`, `/dev`, and the user's home folder on Unix, and a drive root, the Windows folder, the program folders, and the user's profile folder on Windows
- [ ] Given a request to format a device (`mkfs`), when it arrives, then it is declined the same way
- [ ] Given a request to write to a raw block device (`dd` with a device as output), when it arrives, then it is declined the same way
- [ ] Given a request to shut down or reboot the computer, when it arrives, then it is declined the same way
- [ ] Given a request to delete the daemon's data folder or its own program, when it arrives, then it is declined the same way
- [ ] Given an `Opaque` target that contains a floor's program name, when it arrives on a Full access bot, then the daemon does not allow it and it waits for a person
- [ ] Given a floor, when a per-bot allow rule matches the same command, then the floor wins

#### US-015: Per-bot rules
**Description:** As a user, I want to refuse or allow a command for one bot, so that I approve `cargo test` once and never a push.

**Priority:** P1
**Dependencies:** Blocked by US-001, US-013, US-014

**References:** Decision D3. Assumption A9. `hermes-agent/tools/approval.py:504-517`. `hermes-agent/tools/approval_floors.py:47-53`.

**Acceptance Criteria:**
- [ ] Given a client message that creates a rule, when it is applied, then the rule is stored with its bot, its kind (refuse or allow), its program, and its leading arguments
- [ ] Given a refuse rule, when a matching request arrives on a Full access bot, then it is declined with the reason `Rule` and the rule's id
- [ ] Given an allow rule, when a simple command arrives on an Ask first bot with the rule's program and an argument list that starts with the rule's arguments, then the daemon selects the allow-once option with the reason `Rule` and the rule's id
- [ ] Given an allow rule, when the request is `Opaque` or was cut, then the rule does not match and the request waits
- [ ] Given an allow rule for `git status`, when the request is `git push`, then the rule does not match
- [ ] Given a request that carries no folder for its command, when it arrives, then no allow rule matches and the request waits
- [ ] Given a request whose command runs in a folder outside the bot's folder, when it arrives, then no allow rule matches
- [ ] Given a request that carries no folder, when a refuse rule matches its command, then it is declined
- [ ] Given a refuse rule and an allow rule that both match, when the request arrives, then it is declined
- [ ] Given a daemon tool, a prompt, or a file in the bot's folder, when any of them asks for a rule, then no rule is created: only the client message creates one
- [ ] Given a rule that is deleted while a request waits, when the request is answered, then the answer follows the rules as they are at that moment

#### US-016: An answer is stored before it is given
**Description:** As the daemon, I want an approval answered only after its record is written, so that the history never misses an action that ran.

**Priority:** P1
**Dependencies:** None

**References:** Audit, section "À reprendre : boucle d'agent", row "Persister avant d'exécuter". `hermes-agent/agent/turn_tool_round.py:133-156`. `crates/crowbotd/src/conversation.rs:1194-1202`.

**Acceptance Criteria:**
- [ ] Given a request the daemon or a person allows, when the answer is sent to the agent, then the `ApprovalResolved` event is already committed
- [ ] Given a database write that fails, when an approval would be allowed, then the daemon declines the request with the reason `NotRecorded`
- [ ] Given that decline, when the turn ends, then it ends as `TurnFailed` with a message that names storage

---

### EP-004: Memory and the search of past conversations

A bot keeps facts across sessions, each tied to the event that grounds it, and can search its own history.

**Definition of Done:** In a test, a fact saved in one conversation appears in the system prompt of the next session with its source event; a poisoned entry is replaced in the prompt and kept visible; and the fake agent finds a past message through `session_search`.

#### US-017: The memory store
**Description:** As the daemon, I want memory as rows with a scope and a source, so that every fact can be traced and withdrawn.

**Priority:** P0
**Dependencies:** None

**References:** Decision D5. Audit, section "À reprendre : mémoire et recherche de sessions", rows "Deux magasins distincts" and "Refus des doublons". `hermes-agent/tools/memory_tool_store.py:23-26`, `:412-413`.

**Acceptance Criteria:**
- [ ] Given the schema, when it is migrated, then a memory entry holds an id, a scope (`Bot` or `User`), a bot id for the `Bot` scope, a text, a `core` flag, its author (bot, review, or user), the id of the event that grounds it, and the times it was created and retired
- [ ] Given an entry whose normalized text equals an active entry of the same scope, when it is added, then nothing is added and the existing id is returned
- [ ] Given an entry that is retired, when memory is read, then it is absent and its row remains
- [ ] Given a bot that is deleted, when its memory is read, then its `Bot` entries are gone and the `User` entries remain
- [ ] Given a source event id that does not belong to a conversation of the bot, when an entry is added, then the write is refused

#### US-018: Scan what the daemon injects
**Description:** As a user, I want text that tries to steer a bot kept out of its prompt, so that a poisoned memory does not last across sessions.

**Priority:** P0
**Dependencies:** None

**References:** Audit, section "À reprendre : mémoire", row "Balayage d'injection". `hermes-agent/tools/memory_tool_store.py:29-32`, `:224-241`. `hermes-agent/cron/prompt_scan.py:30-57`. `hermes-agent/tools/skills_guard.py:172-556`, `:1076` (two levels of verdict). The mapping of categories to severities is this PRD's own.

**Acceptance Criteria:**
- [ ] Given a text, when it is scanned, then the result names each finding with a category among instruction override, concealment from the user, secret reading, exfiltration, persistence, and destruction
- [ ] Given a text with no finding, when it is scanned, then the result is empty
- [ ] Given a text that hides a directive behind Unicode compatibility forms or zero-width characters, when it is scanned, then the finding is reported
- [ ] Given a finding, when it is reported, then it has a severity: `Dangerous` for secret reading, exfiltration, persistence, and destruction; `Caution` for instruction override and concealment
- [ ] Given the fixture file of labelled texts, which holds for each category at least one positive and one harmless near-miss (a sentence that quotes or describes the pattern), when the test scans it, then every text gets its label
- [ ] Given the fixture, when it is read, then it holds at least one positive text for each pattern family of `hermes-agent/cron/prompt_scan.py:30-57` and of `hermes-agent/tools/memory_tool_store.py:224-241`
- [ ] Given a text that documents an attack inside a quotation, when it is scanned, then the finding is `Caution` at most

#### US-019: The `memory` tool and the client messages
**Description:** As a bot, I want to add, correct, and retire facts in one call, so that my memory stays small and true.

**Priority:** P0
**Dependencies:** Blocked by US-004, US-017, US-018

**References:** Audit, section "À reprendre : mémoire", rows "Outil unique, lot atomique" and "Consigne quand mémoriser". `hermes-agent/tools/memory_tool.py:384-396`, `:418`. `hermes-agent/tools/memory_tool_store.py:568-573`, `:624-635`.

**Acceptance Criteria:**
- [ ] Given a call with several operations (add, correct by id, retire by id), when all are valid, then all are applied in one transaction and one `MemoryChanged` event lists them
- [ ] Given a call in which one operation is invalid, when it is applied, then none is applied and the answer names the operation
- [ ] Given an operation whose text has a scan finding, when the call arrives, then the call is refused and the answer names the category
- [ ] Given a call that would leave the entries flagged `core` of a scope over their limit, when it arrives, then it is refused and the answer lists the current entries (limits: 2,200 characters for `Bot`, 1,375 for `User`, `memory_tool_store.py:188`)
- [ ] Given a session of kind `Delegated`, when it lists its tools, then `memory` is absent
- [ ] Given the tool's description, when the agent reads it, then it says that memory holds facts true for every session and that what is learned while doing a task belongs in a skill
- [ ] Given an add operation, when it sets `core`, then the entry enters the system prompt of later sessions; when it does not, the entry is recalled by turn only
- [ ] Given a correct operation, when it sets or clears `core` on an entry, then the entry's flag follows, under the same limit
- [ ] Given a client message that lists a bot's memory, when it is answered, then each entry has its text, scope, `core` flag, author, and source event
- [ ] Given a client message that adds or retires an entry, when it is applied, then the entry's author is the user and a `MemoryChanged` event is recorded

#### US-020: Memory in the prompt and recalled by turn
**Description:** As a bot, I want my core memory in my system prompt and the rest recalled when it matters, so that memory can grow past what fits in every session.

**Priority:** P0
**Dependencies:** Blocked by US-005, US-017, US-018, US-006

**References:** Decision D7. Audit, section "À reprendre : mémoire", rows "Rappel par tour" and "Balayage d'injection". `hermes-agent/agent/turn_context.py:1076-1109`. `hermes-agent/agent/memory_context.py:170-186`.

**Acceptance Criteria:**
- [ ] Given a bot with entries flagged `core`, when a session opens, then the system prompt holds one section for the `Bot` scope and one for the `User` scope
- [ ] Given an entry that gains a scan finding at load, when the section is built, then the entry is replaced by a placeholder that names the category, and the stored text is unchanged
- [ ] Given entries not flagged `core`, when a prompt is sent, then the entries whose text matches the user's message by full-text search are placed in the note, inside a fence that says they are recalled memory and not a message of the user
- [ ] Given no matching entry, when a prompt is sent, then the note holds no memory fence
- [ ] Given an entry added during a session, when the next prompt is sent, then the note names it once and later prompts do not repeat it
- [ ] Given a session whose reported context size fell below its previous report, when the next prompt is sent, then the note carries the two core sections again (A11)
- [ ] Given an agent that reports no context size, when prompts are sent, then the core sections are sent at session open only

#### US-021: The `session_search` tool
**Description:** As a bot, I want to search my past conversations, so that I can find what was said or run before.

**Priority:** P1
**Dependencies:** Blocked by US-003

**References:** Audit, section "À reprendre : mémoire", rows "Recherche de sessions" and "Index des appels d'outils". `hermes-agent/tools/session_search_tool.py:8`, `:47-50`, `:449-550`. `crates/crowbotd/src/store.rs:103-130`. `crates/crowbotd/src/store/search.rs:77-170`.

**Acceptance Criteria:**
- [ ] Given a query, when the tool is called, then the answer groups hits by conversation and gives for each a title, a time, a snippet, and the id of the matching event
- [ ] Given a conversation id and an event id, when the tool is called, then the answer holds the events around that event
- [ ] Given a hit in the conversation that is running, when results are built, then it is left out
- [ ] Given a hit in a conversation of another bot, when results are built, then it is left out
- [ ] Given a tool call stored as `tool_call_started`, when a query matches its title or input, then it is found
- [ ] Given a query shorter than `SEARCH_MESSAGE_TERM_LENGTH`, when the tool is called, then the answer is empty and says why
- [ ] Given the tool, when it runs, then it starts no model call

---

### EP-005: Skills

A bot writes down how it did a task and loads that note the next time, on any provider.

**Definition of Done:** In a test, the fake agent creates a skill, a new session lists it in its system prompt, the agent loads it, corrects it against the version it read, and a client rolls it back; a skill with a dangerous finding waits for an approval.

#### US-022: The skill store and its projection
**Description:** As the daemon, I want each version of a skill stored by content, so that every change can be put back and no file on disk can lie.

**Priority:** P0
**Dependencies:** None

**References:** Decision D5. Audit, section "À reprendre : skills", rows "Format du skill" and "Journal d'audit". `hermes-agent/tools/skill_manager_tool.py:106-111`, `:144-168`. `hermes-agent/tools/skill_ledger.py:1-8`. `crates/crowbotd/src/runtime.rs` (the hash check of an installed runtime).

**Acceptance Criteria:**
- [ ] Given a skill, when a version is stored, then it holds a `SKILL.md` with a front matter that has a name and a description, and files only under `references`, `templates`, `scripts`, and `assets`
- [ ] Given a name longer than 64 characters or outside `[a-z0-9][a-z0-9._-]*`, when a version is stored, then it is refused with the limit named (`skill_manager_tool.py:106`, `:110`)
- [ ] Given a `SKILL.md` over 100,000 characters, when a version is stored, then it is refused with the limit named (`skill_manager_tool.py:108`)
- [ ] Given a file over 1 MiB, when a version is stored, then it is refused with the limit named (`skill_manager_tool.py:109`)
- [ ] Given two versions with the same content, when they are stored, then they share one hash and one stored copy
- [ ] Given a session start, when the projection is written, then `<data>/skills/<bot>/<skill>/` holds the active version, read-only, and every file matches its stored hash
- [ ] Given a projected file that was changed, when the next session starts, then the folder is written again from the database before the agent starts
- [ ] Given a skill path that would leave its folder, when a version is stored, then it is refused

#### US-023: The skills index and the read tools
**Description:** As a bot, I want a short index of my skills in my prompt and the full text on demand, so that I pay for a skill only when I use it.

**Priority:** P0
**Dependencies:** Blocked by US-003, US-005, US-022

**References:** Audit, section "À reprendre : skills", rows "Divulgation progressive" and "Index stable". `hermes-agent/agent/prompt_builder.py:1327-1369`. `hermes-agent/agent/skill_utils.py:846`. `hermes-agent/tools/skills_tool.py:634-734`.

**Acceptance Criteria:**
- [ ] Given a bot with skills, when a session opens, then the system prompt holds one line per active skill with its name and its description cut at 60 characters (`skill_utils.py:846`)
- [ ] Given that section, when it is built, then it opens with the instruction to load a matching skill with `skill_view` before acting
- [ ] Given `skills_list`, when it is called, then the answer holds each active skill's name, full description, and version hash
- [ ] Given `skill_view` with a name, when it is called, then the answer holds the `SKILL.md`, the list of its files, the path of its projection, and the hash of the version
- [ ] Given `skill_view` with a name and a file, when it is called, then the answer holds that file
- [ ] Given a skill created or archived during a session, when the next prompt is sent, then the note says so once
- [ ] Given a name that does not exist, when `skill_view` is called, then the answer is an error that lists the names that do
- [ ] Given a bot with no skill, when a session opens, then the system prompt holds no skills section

#### US-024: The `skill_manage` tool
**Description:** As a bot, I want to create and correct skills through one checked path, so that a bad edit never replaces a good skill.

**Priority:** P0
**Dependencies:** Blocked by US-004, US-018, US-022

**References:** Audit, section "À reprendre : skills", rows "Outil d'écriture unique", "Lire avant d'écrire", and "Balayage de sécurité". `hermes-agent/tools/skill_manager_tool.py:406-431`, `:847-849`, `:601-614`. `hermes-agent/agent/background_review.py:584-591`. `hermes-agent/tools/skills_guard.py:26-42`.

**Acceptance Criteria:**
- [ ] Given a call with several operations (create, correct, write a file, remove a file, archive), when all are valid, then all are applied in one transaction and one `SkillChanged` event names each skill with its old and new hash
- [ ] Given a call in which one operation fails, when it is applied, then no skill changes
- [ ] Given a correction, when it names a version hash that is not the active one, then it is refused and the answer says to read the skill again
- [ ] Given a version with no finding or with `Caution` findings only, when it is written by a bot, then it becomes active
- [ ] Given a version with a `Dangerous` finding, when it is written by a bot, then an approval request shows the finding and the difference, and the version becomes active only if a person allows it
- [ ] Given a session of kind `Review`, when it archives a skill, then the skill is archived
- [ ] Given any session, when it calls the tool, then no operation removes a stored version
- [ ] Given a skill that is pinned, when a bot archives it, then the call is refused

#### US-025: Skill history, rollback, and import
**Description:** As a user, I want to put a skill back to an earlier version and to give a bot a skill I already have, so that I stay in charge of what a bot knows how to do.

**Priority:** P1
**Dependencies:** Blocked by US-022

**References:** Audit, section "À reprendre : skills", rows "Journal d'audit" and "Synchronisation des skills fournis". `hermes-agent/tools/skill_ledger.py:1-8`. `hermes-agent/tools/skills_guard.py:35-42`.

**Acceptance Criteria:**
- [ ] Given a client message that asks for a skill's history, when it is answered, then each version has its hash, its time, its author (bot, review, or user), and the conversation that caused it
- [ ] Given a client message that rolls a skill back to a version, when it is applied, then that version is active and a `SkillChanged` event records the user as the author
- [ ] Given a client message that imports a folder, when the folder holds a valid skill, then the skill is stored for that bot with the user as the author
- [ ] Given an imported folder with a `Dangerous` finding, when it is imported, then the import is refused unless the message confirms it, and the answer lists the findings
- [ ] Given a client message that pins, unpins, archives, or restores a skill, when it is applied, then the skill's state follows and a client that lists the skills sees it

---

### EP-006: Learning from use

When the user allows it, a bot reviews what it did and keeps what is worth keeping, in the open.

**Definition of Done:** In a test, a bot with the `Review` origin on runs a review conversation after 10 tool calls, the review writes one skill, and a bot with the origin off runs none; the ledger refuses a turn over budget; a stale skill is archived by the curator.

#### US-026: Switches and limits for daemon-started turns
**Description:** As a user, I want to choose which turns the daemon may start for a bot and how many a day, so that nothing spends my plan without my consent.

**Priority:** P0
**Dependencies:** Blocked by US-006, US-007, US-009

**References:** Decision D4. `hermes-agent/cron/scheduler.py:2472`. `hermes-agent/agent/turn_finalizer.py:138-164` (the summary after an exhausted budget).

**Acceptance Criteria:**
- [ ] Given a new bot, when its settings are read, then `Review` and `Budget` are off, `Retry` is on, and `Routine`, `Goal`, and `Delegation` are off
- [ ] Given a client message that sets a bot's harness settings (the switches, the daily limit of daemon-started turns, the optional daily token limit, the verification reminder), when it is applied, then every attached client receives the new settings
- [ ] Given a message that turns on a switch other than `Retry` while no daily limit is set, when it arrives, then it is refused and the answer asks for the limit
- [ ] Given a turn of an origin that is off, when it is due, then it does not start and a record names the origin and the cause
- [ ] Given a daily limit of turns, when the daemon-started turns of the daemon's local day reach it, then no further daemon-started turn starts that day and a record says so
- [ ] Given a daily token limit and turns whose provider reported tokens, when their total for the day reaches it, then no further daemon-started turn starts that day
- [ ] Given a provider that reports no tokens, when the token limit is checked, then that provider's turns count toward the limit of turns only
- [ ] Given a client message that asks for a bot's consumption, when it is answered, then it gives, per origin and for the current day, the number of turns and the tokens and cost that were reported
- [ ] Given a turn ended for `BudgetExhausted` and the `Budget` origin turned on, when the turn has ended, then the daemon queues one message of origin `Budget` that asks for a summary of the work done
- [ ] Given the `Budget` origin turned off, when a turn ends for `BudgetExhausted`, then no message is queued
- [ ] Given a turn of origin `User`, when it starts, then neither the switches nor the limits apply
- [ ] Given every daemon tool, when the list of tools is read by the test, then none takes a switch or a limit as input

#### US-027: The review conversation
**Description:** As a user, I want a bot to review its recent work in a conversation I can read, so that I see what it learned and can undo it.

**Priority:** P1
**Dependencies:** Blocked by US-019, US-021, US-023, US-024, US-026

**References:** Audit, section "À reprendre : skills", rows "Revue en arrière-plan" and "Ce que la revue n'a pas le droit de faire". `hermes-agent/agent/review_signals.py:33-50`. `hermes-agent/agent/agent_init.py:1177`, `:1219-1221`. `hermes-agent/agent/background_review.py:532-603`, `:1212-1252`.

**Acceptance Criteria:**
- [ ] Given a bot with `Review` on, when 10 tool calls have run since the last review or the last `skill_manage` call and no turn of the bot is running or queued, then a review conversation starts with origin `Review` (10 is `creation_nudge_interval`, `agent_init.py:1221`)
- [ ] Given a bot with `Review` on, when 10 user turns have run since the last review or the last `memory` call and no turn of the bot is running or queued, then a review starts that may also write memory (`agent_init.py:1177`)
- [ ] Given a review conversation, when it lists its tools, then it holds `memory`, `skills_list`, `skill_view`, `skill_manage`, and `session_search`, and no tool with the `External` effect
- [ ] Given a review, when its agent asks for permission to use a tool that is not a daemon tool, then the daemon declines with the reason `ReviewSession`
- [ ] Given a review that ends, when it changed memory or a skill, then the conversation it reviewed receives one event that lists the changes and links the review conversation
- [ ] Given a user message sent to the bot while a review runs, when it arrives, then the review is stopped and the user's turn starts
- [ ] Given a bot with `Review` off, when the counters pass 10, then no review starts

#### US-028: The curator
**Description:** As a user, I want skills nobody uses set aside, so that the index stays short.

**Priority:** P2
**Dependencies:** Blocked by US-025, US-029, US-031

**References:** Audit, section "À reprendre : skills", row "Curateur". `hermes-agent/agent/curator.py:42-46`, `:210-264`, `:318-326`.

**Acceptance Criteria:**
- [ ] Given a skill written by a bot or a review with no use for 14 days, when the curator runs, then it is marked stale (`curator.py:43`)
- [ ] Given such a skill with no use for 30 days, when the curator runs, then it is archived and leaves the index (`curator.py:43`)
- [ ] Given a skill that is pinned, imported by the user, or named by a routine, when the curator runs, then it is not changed
- [ ] Given the curator, when it runs, then it starts no model call and deletes no version
- [ ] Given a daemon, when 7 days have passed since the last pass and no turn ran for 2 hours, then the curator runs once (`DEFAULT_INTERVAL_HOURS` and `DEFAULT_MIN_IDLE_HOURS`, `curator.py:42`)
- [ ] Given an archived skill, when a client restores it (US-025), then it is active again and its versions are unchanged

#### US-029: Measure what skills change
**Description:** As the product owner, I want the outcome of turns that used a skill recorded, so that learning is judged by figures.

**Priority:** P1
**Dependencies:** Blocked by US-006, US-023

**References:** Assumption A5. Audit, section "Où Crow Bot peut faire mieux", row "Apprentissage visible et mesurable". `hermes-agent/tools/skill_usage.py:1-6`, `:348-353`, `:506-517`.

**Acceptance Criteria:**
- [ ] Given a turn in which `skill_view` was called, when the turn ends, then a row ties the skill, its version, the turn, the turn's end (completed, failed, interrupted, stopped), and the turn's usage
- [ ] Given a client message that asks for a skill's figures, when it is answered, then it gives per version the number of turns, the share that completed, and the mean usage
- [ ] Given a client message that asks for a bot's figures, when it is answered, then it gives the share of turns in which a skill listed in the index was loaded
- [ ] Given a skill never loaded, when its figures are asked, then the answer holds zero turns and no share
- [ ] Given a name that is not a skill of the bot, when its figures are asked, then the answer is an error

---

### EP-007: Routines

A bot does a task on a schedule, whether or not a client is attached.

**Definition of Done:** In tests with a controlled clock, a routine runs once at its time in its own conversation; a daemon stopped across three occurrences records the misses and runs at most once; a routine that needs an approval waits and skips its next occurrence; and a bot cannot create a routine without a person.

#### US-030: Schedules and their next occurrence
**Description:** As the daemon, I want a typed schedule and one function that gives its next occurrence, so that time zones and clock changes are handled in one tested place.

**Priority:** P0
**Dependencies:** None

**References:** Assumption A4. Audit, section "À reprendre : tâches planifiées", rows "Modèle de tâche" and "Fuseaux et changement d'heure". `hermes-agent/cron/jobs.py:792-855`, `:1218`, `:1243-1259`.

**Acceptance Criteria:**
- [ ] Given a schedule, when it is stored, then it is one of `Once` (an instant), `Every` (a duration), or `Cron` (five fields and a time zone)
- [ ] Given a `Cron` schedule in a zone with a clock change, when the local time does not exist that day, then the next occurrence is the first instant after the gap
- [ ] Given a `Cron` schedule, when the local time occurs twice that day, then the routine is due once, at the first of the two
- [ ] Given an `Every` schedule, when the next occurrence is computed, then it is the last occurrence plus the duration in UTC
- [ ] Given a cron expression with a field out of range, when it is parsed, then it is refused and the error names the field
- [ ] Given any valid schedule and instant, when a property test asks for the next occurrence, then it is later than the instant
- [ ] Given the time zone dependency, when it is added, then the story records its license and its source of zone data, or the fallback it chose if A4 fails

#### US-031: Routines and the scheduler
**Description:** As a user, I want a routine to run at its time in its own conversation, so that I can read each run like any other.

**Priority:** P0
**Dependencies:** Blocked by US-003, US-007, US-026, US-030

**References:** Audit, section "À reprendre : tâches planifiées", rows "Avancer avant d'exécuter", "Propriétaire unique du tic", and "Session propre". `hermes-agent/cron/jobs.py:2672-2673`, `:2538-2540`. `hermes-agent/cron/scheduler_tick.py:160-163`. `crates/crowbotd/src/store.rs:560-577`.

**Acceptance Criteria:**
- [ ] Given a routine, when it is stored, then it holds its bot, its schedule, its prompt, its skills, its state (active or paused), and its next occurrence
- [ ] Given a due routine, when the scheduler fires, then one transaction moves the next occurrence forward, records the run, and queues the prompt with origin `Routine` in the routine's conversation
- [ ] Given a daemon killed after that transaction, when it restarts, then that occurrence does not run a second time
- [ ] Given a `Once` routine that ran, when the run is recorded, then the routine is finished and never due again
- [ ] Given a routine due in one hour and a controlled clock, when the clock stands one second before the occurrence, then no run is recorded; when it reaches the occurrence, then the run is recorded
- [ ] Given a routine session, when it lists its tools, then `schedule` and `delegate` are absent
- [ ] Given a bot whose `Routine` origin is off, when a routine is due, then it does not run and the miss is recorded with that cause

#### US-032: Missed occurrences
**Description:** As a user, I want occurrences missed while the daemon was down counted and run at most once, so that a restart does not start a burst.

**Priority:** P1
**Dependencies:** Blocked by US-011, US-031

**References:** Audit, section "À reprendre : tâches planifiées", row "Exécutions manquées". `hermes-agent/cron/jobs.py:947-964`. `hermes-agent/cron/missed_runs.py:25-41`, `:96-114`. `crates/crowbotd/src/store.rs:322-355`.

**Acceptance Criteria:**
- [ ] Given a repeating routine and a daemon that starts late by less than half the period, clamped between 120 seconds and 2 hours, when it starts, then the occurrence runs (`jobs.py:947-964`)
- [ ] Given a daemon that starts later than that window after several occurrences, when it starts, then one run is queued and one `RoutineMissed` event gives the number of occurrences missed
- [ ] Given a routine set to skip misses, when the daemon starts past the window, then no run is queued and the event is recorded
- [ ] Given a `Once` routine missed by more than 120 seconds, when the daemon starts, then it does not run and is marked missed (`jobs.py:124`)
- [ ] Given a routine that was paused when it came due, when it is resumed, then no missed occurrence runs
- [ ] Given a daemon that was paused (US-011) while occurrences came due, when it resumes, then the same window decides whether one run is queued

#### US-033: The `schedule` tool and the client messages
**Description:** As a user, I want a bot to propose a routine and me to accept it, so that scheduling takes a sentence and nothing is scheduled behind my back.

**Priority:** P0
**Dependencies:** Blocked by US-004, US-031

**References:** Audit, section "À reprendre : tâches planifiées", row "Suggestions". `hermes-agent/tools/cronjob_tools.py:1019-1035`. `hermes-agent/cron/suggestions.py:1-9`.

**Acceptance Criteria:**
- [ ] Given a bot that calls `schedule` to create or change a routine, when the call arrives, then an approval request shows the schedule's kind and fields, its time zone, its next three occurrences, and the prompt
- [ ] Given a person who allows it, when the call returns, then the routine is stored
- [ ] Given that approval on a bot whose `Routine` switch was off and whose daily limit is set, when the call returns, then the switch is on
- [ ] Given that approval on a bot with no daily limit, when the call returns, then the routine is stored as paused and the answer says the limit is missing
- [ ] Given a person who declines, when the call returns, then no routine is stored
- [ ] Given a bot that calls `schedule` to list its routines, when the call arrives, then it runs with no approval
- [ ] Given a client message that lists a bot's routines, when it is answered, then each has its schedule, state, next occurrence, and the end of its last run
- [ ] Given a client message that creates, pauses, resumes, or deletes a routine, when it is applied, then every attached client receives the routine's new state
- [ ] Given a client message that creates a routine on a bot whose `Routine` switch was off and whose daily limit is set, when it is applied, then the switch is on
- [ ] Given that message on a bot with no daily limit, when it arrives, then it is refused and the answer asks for the limit
- [ ] Given a client message that runs a routine now, when it is applied, then one run starts and the stored next occurrence is unchanged
- [ ] Given a routine prompt with a scan finding, when a bot proposes it, then the approval request shows the finding

#### US-034: A routine with nobody there
**Description:** As a user, I want a routine that needs me to wait for me, and a routine that fails to tell me once, so that an absent user is never taken for a yes.

**Priority:** P0
**Dependencies:** Blocked by US-003, US-009, US-031

**References:** Decision D2. Audit, section "À reprendre : tâches planifiées", rows "Approbations sans personne", "Réponse silencieuse", and "Échecs". `hermes-agent/cron/scheduler.py:543`, `:563-575`. `hermes-agent/cron/unreachable_retry.py:33`, `:58-65`. `hermes-agent/cron/quota_hold.py:34-57`. `hermes-agent/cron/incidents.py:1-10`.

**Acceptance Criteria:**
- [ ] Given a routine run that requests an approval with no client attached, when time passes, then the request stays open and the bot's state is waiting
- [ ] Given a routine whose run is waiting or still running, when its next occurrence comes due, then that occurrence is recorded as skipped and does not start
- [ ] Given a run that calls `routine_result` with `silent`, when the turn ends, then the conversation is not marked unread
- [ ] Given a run that ends without calling `routine_result`, when the turn ends, then the conversation is marked unread
- [ ] Given a run that fails with `SignInRequired`, when it ends, then the routine is paused and one event says why
- [ ] Given a run that fails with `UsageLimit`, when it ends, then the routine stays active and the failure is recorded once per identical cause until a run succeeds
- [ ] Given a run that fails before any agent event, when it ends, then it is retried after 5, 15, then 30 minutes and no further (`unreachable_retry.py:33`)

#### US-035: A guard script before a run
**Description:** As a user, I want a script to decide whether a routine needs the bot, so that a run with nothing new costs nothing.

**Priority:** P2
**Dependencies:** Blocked by US-018, US-033

**References:** Audit, section "À reprendre : tâches planifiées", row "Script de garde". `hermes-agent/cron/scheduler_prompt.py:21-35`, `:342-350`. `hermes-agent/cron/job_args.py:417-439`. `hermes-agent/cron/scheduler.py:1384`.

**Acceptance Criteria:**
- [ ] Given a routine with a script, when it is created or its script changes, then the approval request shows the script's path and content hash
- [ ] Given a due routine with a script, when the script prints nothing or its last line is `{"wakeAgent": false}`, then no turn starts and the run is recorded as silent
- [ ] Given a script that prints other output, when it ends, then the output is placed before the routine's prompt inside a fence that says it is script output
- [ ] Given script output with a scan finding, when it ends, then no turn starts and the run is recorded as blocked with the category
- [ ] Given a script whose content hash differs from the approved one, when the routine is due, then the script does not run and the routine is paused
- [ ] Given a script that runs longer than 3,600 seconds, when the limit passes, then the daemon ends its process group and records the failure (`scheduler.py:1384`)
- [ ] Given a script path outside the bot's folder, when a routine is created, then it is refused

---

### EP-008: Checkpoints

The bot's folder can be put back as it was before any turn, whatever changed it: an edit or a command.

**Definition of Done:** In a test, the fake agent changes one file through an edit and another through a command, and a client restores both to their content before the turn; a file changed by another hand since is not overwritten without a confirmation.

#### US-036: Snapshot the bot's folder around a turn
**Description:** As a user, I want the daemon to record the bot's folder before and after each turn, so that any change a turn made can be seen and undone.

**Priority:** P1
**Dependencies:** None

**References:** Assumption A10. Audit, section "À reprendre : boucle d'agent", row "Points de restauration". `hermes-agent/agent/checkpoints/manager.py:80-83` (a snapshot of the whole folder, there with git). `hermes-agent/agent/tool_executor.py:106-125`. `crates/crowbotd/src/runtime.rs` (the daemon's existing hash of a folder).

**Acceptance Criteria:**
- [ ] Given a turn about to start, when the prompt is sent to the agent, then a manifest of the bot's folder is already stored and tied to the turn's `UserText` event
- [ ] Given a manifest, when it is read, then it holds for each file its path relative to the folder, its size, and its SHA-256
- [ ] Given a file under `.git` or matched by a `.gitignore` of the folder, when a manifest is taken, then the file is absent from it
- [ ] Given a file whose content is not stored yet, when a manifest is taken, then the content is stored once under the data folder, named by its hash
- [ ] Given a second turn before which no file changed, when its manifest is taken, then no new content is stored
- [ ] Given a manifest, when it is stored, then the time it took is stored with it
- [ ] Given a file larger than 1 MiB, when a manifest is taken, then the manifest holds its hash and marks that no content was kept (1 MiB is the per-file limit of US-022)
- [ ] Given a symbolic link, when a manifest is taken, then it is recorded as a link with its target and is not followed
- [ ] Given a turn that ends, when its end is recorded, then a second manifest is stored and tied to that event
- [ ] Given a client message that asks what a turn changed, when it is answered, then it lists the files added, changed, and removed between the turn's two manifests
- [ ] Given a file the agent changed through a command and not through an edit, when that message is answered, then the file is in the list
- [ ] Given a storage failure or an unreadable file, when a manifest cannot be completed, then the turn starts and a `CheckpointSkipped` event names the cause
- [ ] Given a conversation whose history is deleted, when stored contents are no longer named by any manifest, then they are removed

#### US-037: Put the folder back
**Description:** As a user, I want to return files to their state before a given turn, so that one bad turn does not cost the whole conversation.

**Priority:** P1
**Dependencies:** Blocked by US-036

**References:** Audit, section "Où Crow Bot peut faire mieux", row "Points de restauration reliés aux événements". `hermes-agent/agent/checkpoints/manager.py:1242` (signature only; the body was not read by the audit).

**Acceptance Criteria:**
- [ ] Given a client message that names a turn and a list of paths, when it is applied, then each named file holds the content of the manifest taken before that turn and a `CheckpointRestored` event lists the files
- [ ] Given that message with no list of paths, when it is applied, then it covers every file that differs from that manifest
- [ ] Given a file that was absent from that manifest, when it is restored, then the file is removed
- [ ] Given a restore, when it is applied, then a manifest of the folder as it stood just before the restore is stored first
- [ ] Given a file whose content differs from the last manifest taken, when a restore covers it, then the restore is refused unless the message confirms it
- [ ] Given a turn running in a conversation of that bot, when a restore is asked, then it is refused
- [ ] Given a file whose manifest entry kept no content, when a restore covers it, then that file is left as it is and the answer names it

---

### EP-009: Delegation

A bot hands part of a task to a conversation of its own, or of another bot, and gets the result back as a new turn.

**Definition of Done:** In a test, a parent conversation delegates, the child's approval request reaches the client, the child returns a value that fits the schema, and the parent receives it as a queued message after its own turn ended.

#### US-038: Child conversations and the `delegate` tool
**Description:** As a bot, I want to give a goal to a fresh conversation, so that a side task does not fill my context.

**Priority:** P2
**Dependencies:** Blocked by US-004, US-007, US-026

**References:** Decision D8. Audit, section "À reprendre : délégation et sous-agents". `hermes-agent/tools/delegate_tool.py:319-337`, `:366`. `hermes-agent/tools/delegate_tool_toolsets.py:14-22`. `hermes-agent/tools/delegate_tool_config.py:19-23`.

**Acceptance Criteria:**
- [ ] Given a bot that calls `delegate` with a goal and a context, when a person allows it, then a conversation is created with the calling conversation as its parent and the goal queued with origin `Delegation`
- [ ] Given that approval on a bot whose `Delegation` switch was off and whose daily limit is set, when the call returns, then the switch is on
- [ ] Given a bot with no daily limit, when `delegate` is allowed, then no child is created and the answer says the limit is missing
- [ ] Given `delegate` with another bot's name, when it is allowed, then the child belongs to that bot and runs under that bot's permission level
- [ ] Given a child session, when it lists its tools, then `delegate`, `schedule`, and `memory` are absent
- [ ] Given a conversation that is itself a child, when it calls `delegate`, then the call fails: the depth limit is 1 (`delegate_tool_config.py:23`)
- [ ] Given a parent with 10 children running, when it calls `delegate`, then the call fails and names the limit (`delegate_tool_config.py:19`)
- [ ] Given a child that requests an approval, when it is recorded, then it reaches the clients as a request of the child's conversation
- [ ] Given `delegate` naming a bot that does not exist, when it is called, then it fails before any approval request

#### US-039: The result of a delegation
**Description:** As a bot, I want the child's result as a new turn and in the shape I asked for, so that I can use it without parsing prose.

**Priority:** P2
**Dependencies:** Blocked by US-038

**References:** Audit, section "À reprendre : délégation", rows "Résultat en tâche de fond" and "Sortie structurée". `hermes-agent/tools/async_delegation.py:1-9`. `hermes-agent/tools/delegate_tool_child_run.py:560-608`, `:674-681`.

**Acceptance Criteria:**
- [ ] Given a child that calls `delegation_result` with a value, when the child's turn ends, then the parent's queue receives one message of origin `Delegation` with the value and the child's conversation id
- [ ] Given a parent whose turn is running, when the result arrives, then it waits in the queue and starts no turn before the running one ends
- [ ] Given a `delegate` call with a JSON schema, when the child's value does not fit it, then the child receives one message that lists the errors and may answer once more
- [ ] Given a second value that does not fit, when the child's turn ends, then the parent receives the value marked invalid with the errors
- [ ] Given a child that ends without calling `delegation_result`, when its turn ends, then the parent receives its last message text marked as unstructured
- [ ] Given a child that fails or is stopped, when it ends, then the parent receives a message that names the end

#### US-040: Stop and restart with children
**Description:** As a user, I want one action that stops a conversation and its children, so that a delegation I regret ends everywhere.

**Priority:** P2
**Dependencies:** Blocked by US-038

**References:** Audit, section "À reprendre : délégation", rows "Propagation de l'arrêt" and "Reprise après redémarrage". `hermes-agent/tools/delegate_tool_child_run.py:143-162`. `crates/crowbotd/src/store.rs:322-355`.

**Acceptance Criteria:**
- [ ] Given a parent with running children, when the client stops the parent's turn, then the children keep running
- [ ] Given the same, when the client sends the message that stops the tree, then the parent and every child are stopped
- [ ] Given that stop, when a child had a request waiting, then the request closes as `Cancelled`
- [ ] Given a daemon that restarts while a child ran, when it is ready, then the child's turn is recorded as interrupted and the parent receives a message that names that end
- [ ] Given a parent that is deleted, when its children are read, then they remain with no parent

---

### EP-010: Goals

A bot keeps working toward an objective over several turns and stops when it is met or when it cannot be.

**Definition of Done:** In a test, a goal with a check command continues for two turns and ends when the check passes; a goal that reaches its turn limit pauses.

#### US-041: A goal that lasts across turns
**Description:** As a user, I want to give a bot an objective and have it continue until it is done, so that I do not type "continue".

**Priority:** P2
**Dependencies:** Blocked by US-004, US-007, US-026

**References:** Audit, section "À reprendre : boucle d'agent", row "Objectif persistant". `hermes-agent/hermes_cli/goals.py:1-8`, `:32-52`.

**Acceptance Criteria:**
- [ ] Given a client message that sets a goal on a conversation, when it is applied, then the goal is stored with its text, its check commands, and its turn count
- [ ] Given that message on a bot whose `Goal` switch was off and whose daily limit is set, when it is applied, then the switch is on
- [ ] Given that message on a bot with no daily limit, when it arrives, then it is refused and the answer asks for the limit
- [ ] Given a goal, when a turn completes, then the daemon runs each check command in the bot's folder as an approval request of that conversation
- [ ] Given a check that fails, when it ends, then the daemon queues a message of origin `Goal` that holds the check's output
- [ ] Given all checks passing, when the bot calls `goal_verdict` with `met`, then the goal ends and an event says so
- [ ] Given all checks passing and the bot calling `goal_verdict` with `not_met` and a reason, when the turn ends, then the daemon queues a message of origin `Goal` with the reason
- [ ] Given a goal, when it has continued 20 turns, then it pauses and an event names the limit (`goals.py:32`)
- [ ] Given a turn that fails, is stopped, or is interrupted, when it ends, then the goal pauses
- [ ] Given a goal with no check command, when a turn completes without a `goal_verdict` call, then the goal pauses

#### US-042: A reminder to verify an edit
**Description:** As a user, I want a bot that edited code and ran nothing reminded once, so that fewer turns end on unverified work.

**Priority:** P2
**Dependencies:** Blocked by US-007, US-026

**References:** Audit, section "À reprendre : boucle d'agent", row "Porte de vérification avant de finir". `hermes-agent/agent/turn_stop_gates.py:134-175`. `hermes-agent/agent/verification_stop.py:1-3`, `:27-35`.

**Acceptance Criteria:**
- [ ] Given a bot whose settings (US-026) turn the verification reminder on, when a turn completes with an edit call and no execute call after it, then the daemon queues one message of origin `Goal` that asks for verification
- [ ] Given that follow-up turn, when it completes, then no second reminder is queued
- [ ] Given a turn whose edits touch only `.md` or `.txt` files, when it completes, then no reminder is queued (`verification_stop.py:27-35`)
- [ ] Given a new bot, when its settings are read, then the setting is off
- [ ] Given the reminder on and the `Goal` switch off, when a turn completes with an unverified edit, then no reminder is queued

---

### EP-011: The client keeps working

The desktop client reads the new protocol and shows the new events.

**Definition of Done:** The client's tests show a message with an origin and a note rendered, an unknown event and an unknown approval reason rendered as unsupported rows, and a client on protocol version 9 refused with the existing incompatibility screen.

#### US-043: The transcript pattern for new events
**Description:** As a user, I want every new kind of event shown in the transcript, so that nothing the daemon did is hidden.

**Priority:** P0
**Dependencies:** Blocked by US-005, US-007

**References:** `crates/crowbot-protocol/src/message.rs:9`. `crates/crowbot-desktop/src/transcript.rs`. `DESIGN.md`.

**Acceptance Criteria:**
- [ ] Given this story, when it lands, then `PROTOCOL_VERSION` is 10 and the transcript has one function that every later story extends with its own row
- [ ] Given a `UserText` whose origin is not `User`, when it is shown, then the row names the origin
- [ ] Given a `UserText` with a note, when it is shown, then the note is collapsed under the message and can be opened
- [ ] Given a daemon on `PROTOCOL_VERSION` 10, when a client built on version 9 attaches, then it shows the incompatibility screen it has today
- [ ] Given an approval reason this client does not know, when it is shown, then the row shows the approval with the reason's name as text
- [ ] Given an event the client cannot read, when a conversation holds it, then the conversation opens and that row says the event is not supported by this client

---

### EP-012: Desktop surfaces for the harness

The user reads and manages memory, skills, routines, rules, and consumption from the client.

**Definition of Done:** Screenshots of each panel in its empty, filled, and refused-action states match `DESIGN.md`, and each action named below changes the daemon's state in a test.

#### US-044: Memory and skills of a bot
**Description:** As a user, I want to see what a bot remembers and knows how to do, so that I can correct it.

**Priority:** P1
**Dependencies:** Blocked by US-019, US-025, US-043

**References:** Audit, section "Où Crow Bot peut faire mieux", row "Mémoire avec provenance". `crates/crowbot-desktop/src/workspace/bot_panel.rs`. `DESIGN.md`.

**Acceptance Criteria:**
- [ ] Given a bot with memory, when its panel opens, then each entry shows its text, its scope, and a link that opens the conversation at the event that grounds it
- [ ] Given an entry, when the user retires it, then it leaves the list and the daemon's memory
- [ ] Given an entry replaced by a scan placeholder in the prompt, when the list shows it, then it is marked with the category and its stored text is shown
- [ ] Given a bot with skills, when the list opens, then each skill shows its name, its description, its state, and whether it is pinned
- [ ] Given a skill, when the user opens it, then its versions are listed and one action rolls back to a version
- [ ] Given a bot with no memory and no skill, when the panel opens, then each list shows its empty state
- [ ] Given an action the daemon refuses, when the answer arrives, then the panel shows the daemon's message and the list is unchanged

#### US-045: Routines
**Description:** As a user, I want to see and control a bot's routines, so that I know what will run and when.

**Priority:** P1
**Dependencies:** Blocked by US-011, US-033, US-043

**References:** `DESIGN.md`.

**Acceptance Criteria:**
- [ ] Given a bot with routines, when the list opens, then each shows its schedule's kind and fields, its next occurrence in the user's time zone, its state, and the end of its last run
- [ ] Given a routine, when the user pauses, resumes, runs, or deletes it, then the daemon's state changes and the list follows
- [ ] Given a routine whose run is waiting for an approval, when the list shows it, then it is marked as waiting and one action opens that conversation
- [ ] Given a daemon that is paused, when the list opens, then a notice says no routine will run and offers to resume
- [ ] Given a bot with no routine, when the list opens, then it shows its empty state
- [ ] Given an action the daemon refuses, when the answer arrives, then the list shows the daemon's message and is unchanged

#### US-046: Rules, switches, and consumption
**Description:** As a user, I want to set what a bot may do without me and see what it consumed, so that autonomy is a choice I can read.

**Priority:** P1
**Dependencies:** Blocked by US-014, US-015, US-026, US-043

**References:** Decisions D3 and D4. `crates/crowbot-desktop/src/workspace/settings.rs`. `DESIGN.md`.

**Acceptance Criteria:**
- [ ] Given a bot, when its rules open, then the built-in refusals are listed as read-only and the bot's rules as editable
- [ ] Given a bot with no rule of its own, when its rules open, then the editable list shows its empty state
- [ ] Given a request that waits, when the user chooses to always allow or always refuse that command for this bot, then the rule is created
- [ ] Given that choice, when the rule is created, then the waiting request is answered by it
- [ ] Given a request that is opaque or cut, when it is shown, then the action that creates an allow rule is absent
- [ ] Given a bot, when its autonomy settings open, then each origin has a switch and the daily limit of turns has a field
- [ ] Given a switch turned on with no daily limit, when the daemon refuses, then the field for the limit is marked and the switch stays off
- [ ] Given a bot, when the consumption view opens, then it shows the day's turns per origin, with tokens and cost where they were reported
- [ ] Given a provider that reports no tokens, when the view opens, then its rows show turns only and say the tokens are unknown

## Functional Requirements

| ID | Requirement | Stories |
|----|-------------|---------|
| FR-01 | Every session carries the daemon's MCP server | US-002 |
| FR-02 | A daemon tool has a typed input and one effect; an `External` tool asks | US-003, US-004 |
| FR-03 | The system prompt is fixed for a session; changes travel in a recorded note | US-005 |
| FR-04 | Usage reported by an agent is stored per turn | US-006 |
| FR-05 | Every turn records its origin; a daemon-started turn needs its switch and room under its daily limit | US-007, US-026 |
| FR-06 | A turn with no progress, no activity, or no budget left is stopped by the daemon | US-008, US-009 |
| FR-07 | Refusals are built in and per bot; allow rules match parsed commands; only a person writes a rule | US-012 to US-015 |
| FR-08 | Memory entries have a scope, a source event, and a history | US-017, US-019 |
| FR-09 | Text the daemon injects is scanned by one module | US-018 |
| FR-10 | A bot searches its own conversations with no model call | US-021 |
| FR-11 | Skill versions are stored by content and projected read-only | US-022 to US-025 |
| FR-12 | A review is a recorded conversation with daemon tools only | US-027 |
| FR-13 | A routine runs at most once per occurrence, in its own conversation | US-030 to US-032 |
| FR-14 | An unattended approval waits | US-034 |
| FR-15 | The bot's folder is recorded before and after each turn and can be put back | US-036, US-037 |
| FR-16 | A delegation is a child conversation whose result returns as a queued message | US-038, US-039 |
| FR-17 | A goal continues a conversation up to a limit | US-041 |

## Non-Functional Requirements

- **Compatibility of stored data:** a database written before this PRD opens after each migration with every conversation, event, and bot intact (`AGENTS.md`, "The database is the source of truth").
- **Privacy on disk:** every folder and file the daemon creates for skills, checkpoints, and bridge endpoints is 0700 or 0600 on Unix (`AGENTS.md`, "Folders and files").
- **Limits are constants:** each limit named in a criterion is one named constant read by the code and by its test.
- **Systems:** every story passes on the systems of `.github/workflows/ci.yml` (Fedora, Ubuntu, Windows Server).

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior |
|----------|---------|-------------------|
| The bridge cannot reach the daemon | The session endpoint is gone | The bridge answers each request with a JSON-RPC error and exits; the turn continues without daemon tools |
| A provider does not support MCP in its ask-first mode | Result of US-001 | The bot runs without daemon tools; the provider's entry says so (D9) |
| The bot is deleted during a daemon tool call | Client message | The call fails; nothing is written |
| The permission level changes while a request waits | Client message | Each waiting request is checked again against the four sources in order: one that was cut, or that is opaque and names a floor's program (US-014), keeps waiting; the others are allowed |
| Memory changes during a session | `memory` call | The system prompt is unchanged; the next note carries the difference |
| The system clock moves backwards | System time change | A routine never runs twice for one stored occurrence |
| Two routines of one bot are due together | Same instant | Each runs in its own conversation |
| The daemon is paused when a routine comes due | US-011 | The occurrence does not start; when the daemon resumes, the rule for missed occurrences (US-032) decides whether one runs |
| A skill's projection folder is not writable | Disk or permission error | The session starts without that skill and an event names the cause |
| A restore is asked while a turn of the bot runs | Client message during a turn | Refused: a turn is running (US-037) |
| A child conversation's bot is on a provider that is signed out | `delegate` | The child's turn fails with `SignInRequired` and the parent receives that end |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|-------------|--------|------------|
| 1 | A provider agent does not accept or use the daemon's MCP server in its ask-first mode (A1) | Medium | High | US-001 runs first; D9 gives the degraded behavior; no later story assumes all three providers |
| 2 | A provider does not label daemon tool calls (A2) | Medium | Medium | The gate is at execution (D1); that provider's calls ask |
| 3 | A model does not load skills from an index (A5) | Medium | Medium | US-029 measures the share of turns that load a skill; D6 is revisited with that figure |
| 4 | Injected text steers a bot through memory, a skill, or script output | High | High | US-018 scans all three and is measured on a labelled fixture; memory keeps its source event; rules and switches cannot be changed by a bot (US-015, US-026) |
| 5 | Daemon-started turns consume the user's plan unseen | Medium | High | Switches off by default, a daily limit of turns required before a switch turns on, an origin on every turn (D4) |
| 6 | A provider compacts its context and loses the memory section | Medium | Medium | A fall in the reported context size makes the daemon send the core sections again (US-020, A11); the per-turn recall covers a provider that reports no size |
| 7 | The scan refuses harmless text | Medium | Low | The answer names the category so the bot can rewrite; the user can write memory and import skills with a confirmation |
| 8 | The hand-written MCP server falls behind the protocol | Low | Medium | Three methods only; US-004 runs it against each real agent that passed A1 |
| 9 | A snapshot of a large folder delays the start of a turn | Medium | Medium | Files ignored by git are left out; the time a snapshot takes is recorded with the manifest |
| 11 | A turn changes a file outside the bot's folder | Medium | Medium | Stated as a limit: a checkpoint covers the bot's folder only (US-036) |
| 10 | A process of the user's account calls a session's tools through its bridge endpoint (A8) | Low | Medium | The same boundary as the daemon's endpoint; an `External` tool still asks a person |

## Non-Goals

- **Calling a model API from the daemon.** No error recovery, context compression, prompt cache markers, tool call repair, or mixture of agents. They belong to whoever owns the loop (audit, sections "À reprendre : boucle d'agent" and "À reprendre : prompt et contexte"; `hermes-agent/agent/error_classifier.py:35-69`, `hermes-agent/agent/context_compressor.py:1073-1074`, `hermes-agent/agent/moa_loop.py:1-6`).
- **An agent loop written in Rust.** A later PRD (D10).
- **A model that decides an approval.** `hermes-agent/tools/approval_smart.py:18-33` is not taken: a person or a typed rule decides.
- **Lazy tool search.** The daemon serves fewer than twenty tools (`hermes-agent/tools/tool_search.py:153-165`).
- **Third-party MCP servers attached to a bot.** A later PRD; trust and sampling limits are designed then (`hermes-agent/tools/mcp_tool_handlers.py:190-219`).
- **External memory services.** One store, in the database (`hermes-agent/agent/memory_provider.py:109-252`).
- **Bundled skills, a skills catalog, or skill downloads.** Distribution waits for the product (`AGENTS.md`, "Product").
- **A summary written by the provider a bot leaves.** See "Other decisions".
- **Messaging gateways, inbound webhooks, and the task board.** `docs/product.md:59` places channels later (`hermes-agent/gateway/platforms/webhook.py:1-6`).
- **Skills as slash commands and the `/learn` command.** A client feature for a later PRD (`hermes-agent/agent/skill_commands.py:381-415`).
- **A consolidation of skills by a model.** Hermes Agent ships it off (`hermes-agent/agent/curator.py:46`); the deterministic pass of US-028 is enough to start.
- **Retention and pruning of old conversations.** The audit did not establish Hermes Agent's schedule (audit, section "Non confirmé").
- **Steering a running turn.** ACP version 1 has no steer (`tasks/prd-crow-bot-desktop-core.md:877`).
- **A client that attaches to a daemon on another computer.** The remote transport is an open decision of `AGENTS.md`; the team owner of "Target Users" is served once it exists.

## Files NOT to Modify

- `crates/crowbot-desktop/LICENSE`, `crates/crowbot-protocol/LICENSE`, `crates/crowbotd/LICENSE.md`, `crates/crowbotd/NOTICE`: the license texts and the required notice.
- The GPUI rev and the `[patch.crates-io]` entries in `crates/crowbot-desktop/Cargo.toml`, and `crates/crowbot-desktop/patches/`: this PRD needs no GPUI change.
- The existing variants and field names of `Event`, `ClientMessage`, and `ServerMessage` in `crates/crowbot-protocol/src`: this PRD adds variants and adds fields with `#[serde(default)]`; it renames and removes nothing.
- The schema texts `SCHEMA_V1` to `SCHEMA_V8` in `crates/crowbotd/src/store.rs`: a change is a new step.
- The `REMOVED` lists and the `CONFIG` texts of `crates/crowbotd/src/provider/codex.rs` and `crates/crowbotd/src/provider/grok.rs`, and `settingSources` in `crates/crowbotd/src/provider/claude.rs`: skills and memory reach a provider through MCP only (D6).
- The session modes of the three provider profiles: no mode is widened (D9).
- `crates/crowbotd/src/runtime/pins.rs`: this PRD installs nothing.
- `docs/product.md`: product decisions change with the product owner, not inside a story.
- `tasks/prd-crow-bot-desktop-core.md`, `tasks/prd-crow-bot-linux-port.md`, and their status files: they are certified.

## Open Questions

- **Does any provider announce that it compacted its context?** No variant of `SessionUpdate` in ACP version 1 names it (`agent-client-protocol-schema-1.17.0/src/v1/client.rs:107-144`). US-020 infers it from a fall in the reported context size (A11); if US-001 finds that a provider's size does not fall, that provider keeps the per-turn recall only.
- **Which time zone crate.** A4 names `jiff`; US-030 confirms it or chooses another.
- **The Codex session mode** stays an open decision of `AGENTS.md`. US-001 records what `read-only` allows for MCP tools; this PRD does not change the mode.
- **The size of the core memory.** US-019 takes Hermes Agent's limits (2,200 and 1,375 characters). With recall by turn (US-020) a different split may serve better; US-029's figures are the place to judge it.
[/PRD]
