[PRD]
# PRD: Crow Bot desktop core

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-14 | Arthur Jean | Initial draft |

## Problem Statement

1. A person who pays for an AI plan can run one agent at a time in a terminal or a chat window. Running several agents in parallel, each with one job, means several terminals and no shared view of who works, who waits, and who failed.
2. An agent stops when its window closes. Work that takes an hour needs a process that outlives the window and a way to come back to it.
3. Agent tools either ask for approval on every action or skip approvals for good. Anthropic measured approval fatigue within weeks of the Claude Code launch. A person needs to stay in control without answering the same question all day.
4. The desktop apps that solve this are built on Electron. They carry a browser engine for each window, and their start time and memory use grow with it.
5. Crow Bot has three working prototypes (a daemon that drives Claude through ACP, stores events in SQLite, and a GPUI client that attaches to it). They prove the architecture, but they are not a product: there are no bots, no settings, one bare transcript, and the daemon must be started by hand.

**Why now:** The Agent Client Protocol reached a stable version 1 with session resume in April 2026, so one Rust client can drive Claude, Codex, and Grok. Anthropic's current terms permit a user to sign in to the unmodified Claude Code binary with their own plan inside a third-party product. The three prototypes closed the open technical questions on 2026-11-14: the daemon controls approvals, a conversation lasts and resumes, and GPUI runs the client on Windows.

## Overview

Crow Bot gives one person several AI teammates, called bots. A background daemon (`crowbotd`) owns the bots, their conversations, and every approval. A native desktop client written in Rust with GPUI attaches to the daemon and shows the work as it happens. This PRD turns the prototypes into the desktop core: bots with a name, instructions, a workspace folder and a permission level; conversations that survive crashes; a complete interface; and three providers.

The interface targets the same UI as OpenBot's desktop app: the same regions, sizes, groups, and controls, as a base to iterate on. Where this PRD gives a number for the interface, it is the value read in OpenBot's source. The product name, the daemon, and the data stay Crow Bot's own.

Every story lists references: where OpenBot and Hermes Agent solve the same problem, with file paths and lines. The implementing agent reads them before it designs. It chooses the best approach for this codebase each time, and taking the approach of either codebase is a valid choice. Paths that start with `openbot/` are under `C:\dev\openbot`, `hermes-agent/` under `C:\dev\hermes-agent`, and `zed/` under `C:\dev\zed`.

The work is phased, because it holds 33 stories:

| Phase | Epics | Stories | Outcome |
|-------|-------|---------|---------|
| A. Foundations and shell | EP-001, EP-002, EP-003 | 16 | Several bots, a daemon that starts on its own and survives crashes, the full window layout |
| B. Conversation | EP-004 | 8 | Markdown, steps, approvals, queue, selection |
| C. Setup, settings, providers | EP-005, EP-006 | 9 | First run, settings, search, Codex and Grok, managed runtimes |

Key decisions made before this PRD: the client is GPL-3.0-or-later, the protocol crate is Apache-2.0, the daemon is PolyForm Noncommercial 1.0.0; the Markdown renderer is the client's own; the existing composer stays; the client starts the daemon; the endpoint is reserved to the same user; no model usage is ever sold.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Stories certified `DONE` by `/review-epic` | 16 (Phase A) | 33 (all phases) |
| Bots that one person runs in parallel on one daemon without a failed turn caused by the daemon | 3 | 10 |
| Text lost when the daemon is killed during a turn | 500 ms of stream or less | 500 ms of stream or less |
| Providers that run with daemon-owned approvals | 1 (Claude) | 3 (Claude, Codex, Grok) |
| Manual steps between a fresh install and the first answer | 2 (start the client, create a bot) | 2 |

## Target Users

### Solo builder
- **Role:** A developer or founder who pays for one or more AI plans and wants several bots working in parallel on their own computer.
- **Behaviors:** Runs Claude Code or Codex in terminals, switches between them, copies results by hand.
- **Pain points:** Loses track of which agent waits for an answer; work stops when a terminal closes; approves the same action many times.
- **Current workaround:** Several terminal tabs, or an Electron agent app.
- **Success looks like:** Opens one window, sees every bot's state at a glance, answers approvals in one place, and closes the window without stopping the work.

### Small team owner
- **Role:** The person who will later run a daemon on a server for a team.
- **Behaviors:** Shares prompts and results in a chat tool; each member runs their own agent.
- **Pain points:** No shared bots, no shared history.
- **Current workaround:** A hosted agent product, or one member's computer.
- **Success looks like:** This PRD does not serve the team yet. It must leave the daemon ready for it: one binary with no display, one protocol, no state in the client.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Grok Bot** (`https://x.ai/bot`): AI teammates, each with its own cloud computer, working in Slack and on calls. Crow Bot keeps the teammate model and runs it local first.
- **OpenBot** (`C:\dev\openbot`): the closest product, Electron and SolidJS, with a messaging-style interface. It is the reference for the interface and the runtime.
- **Hermes Agent** (`C:\dev\hermes-agent`): its own agent loop with interrupt, steer, session search, memory, and skills. It is the reference for turn control and search here, and for memory and skills in later PRDs.
- **Codex app, Conductor, T3 Code:** parallel coding agents, one worktree per thread. They show that parallel agents with a status per agent is the minimum.
- **Market gap:** a native client with a daemon that outlives it, on the user's own AI plans.

### Best Practices Applied
- Set boundaries up front instead of asking about every action: approval fatigue is measured (Anthropic, "Claude Code auto mode"). This PRD adds a Full access level next to Ask first.
- A cancelled turn still ends with a stop reason (ACP `session/cancel`). The daemon keeps the prompt request alive after a cancel and records the end.
- Streamed text is written in small batches. OpenBot flushes on a 100 ms debounce or at 8 KiB (`openbot/src/backend/agent/delta-buffer.ts:29-60`).
- A named pipe server claims the first instance and restricts access, and the client connects with identification-level impersonation (tokio `ServerOptions::first_pipe_instance`, `ClientOptions::security_qos_flags`).
- OWASP Top 10 for Agentic Applications 2026, ASI01 (goal hijack through content the agent reads): approval decisions come from a person or from the bot's permission level, never from content.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- The Claude ACP adapter reports whether the user is signed in and can start the provider's own sign-in. Evidence: its package holds `auth-status.js` and `auth-methods.js`. Validated by US-025.
- `codex-acp` and the Grok CLI let the daemon receive every gated tool call as a permission request, as the Claude adapter does. Evidence: Zed runs both through ACP (`zed/crates/agent_servers/src/custom.rs:18-19`). Validated inside US-030, which fails closed.
- GPUI's `list` element holds a conversation of 10,000 events at 60 frames per second. Evidence: Zed's agent panel uses it for long threads. Measured in US-017.
- Static text selection can be built on GPUI text layout. Evidence: Zed's Markdown element does it (`zed/crates/markdown/src/markdown.rs`).
- A provider accepts a system prompt at session creation through ACP. Evidence: `NewSessionRequest` has a `system_prompt` field in `agent-client-protocol-schema` 1.17.0.

### Hard Constraints
- The license boundary in `AGENTS.md`: the daemon and the protocol crate depend on no GPL crate.
- The daemon owns approvals. A bot never inherits what its owner allowed for personal use.
- The database is the source of truth. A stored event is the JSON of the protocol `Event`; a change must still read rows already stored.
- ACP types do not leave the provider adapter.
- The client holds no durable state. Preferences and drafts live in the daemon.
- Model usage belongs to the user: no credentials read or stored, provider binaries unmodified.
- GPUI comes from Zed's repository at the pinned rev in `crates/crowbot-desktop/Cargo.toml`.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting of the root workspace (daemon and protocol)
- `cargo clippy --all-targets --no-deps` - lints of the root workspace, with no warning reported
- `cargo test` - tests of the root workspace
- `cargo fmt --all --check --manifest-path crates/crowbot-desktop/Cargo.toml` - formatting of the client workspace
- `cargo clippy --all-targets --no-deps --manifest-path crates/crowbot-desktop/Cargo.toml` - lints of the client workspace, with no warning reported
- `cargo test --manifest-path crates/crowbot-desktop/Cargo.toml` - tests of the client workspace

For UI stories, additional gates:
- Start `crowbotd` on a test database and a test endpoint, start the client against it, and capture a screenshot of each state the story names.
- Compare each screenshot with the OpenBot reference files of the story: regions, order, sizes, and states must match.

## Epics & User Stories

### EP-001: Daemon reliability and control

The daemon becomes something a person never has to think about: it starts on its own, only its owner can reach it, it loses almost nothing in a crash, and a turn can be stopped or followed by queued messages.

**Definition of Done:** A fresh client start brings up a daemon with no manual step; a second user on the same computer cannot attach; killing the daemon during a turn loses at most 500 ms of streamed text and the turn shows as interrupted; a running turn stops within 2 seconds of a stop request; messages sent during a turn run in order afterward.

#### US-001: The client starts the daemon
**Description:** As a solo builder, I want the client to start the daemon when none is running so that I never start it by hand.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- OpenBot: single instance lock `openbot/src/main/index.ts:336`; shutdown ordering `openbot/src/main/index.ts:1998-2021`.
- Hermes Agent: a child process that dies with the app versus a gateway that starts detached and outlives it, `hermes-agent/gateway/AGENTS.md:85-94`.
- Crow Bot today: the client only retries the connection, `crates/crowbot-desktop/src/connection.rs:7-26`.

**Acceptance Criteria:**
- [ ] Given no daemon is listening, when the client starts, then it starts `crowbotd` as a detached process and attaches within 3 seconds
- [ ] Given the client started the daemon, when the client window closes, then the daemon keeps running and a running turn continues
- [ ] Given a daemon is already listening, when the client starts, then it attaches to that daemon and starts no second process
- [ ] Given two clients start at the same time with no daemon, when both try to start one, then exactly one daemon listens and both clients attach to it
- [ ] Given the `crowbotd` binary is missing or exits at once, when the client starts, then the window shows the reason and a retry control, and does not loop silently

#### US-002: Only the same user reaches the daemon
**Description:** As a solo builder, I want the daemon's endpoint reserved to my account so that no other user or impostor process can read my conversations or answer my approvals.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- OpenBot: the rules of its renderer-to-main trust boundary, `openbot/src/main/AGENTS.md` and `openbot/src/main/ipc/AGENTS.md`.
- Hermes Agent: the JSON-RPC surface a desktop client reaches, `hermes-agent/tui_gateway/hosted_room_server_rpc.py:65-160`.
- Crow Bot today: the listener and connector, `crates/crowbot-protocol/src/transport.rs`; the open decision in `AGENTS.md`.

**Acceptance Criteria:**
- [ ] Given Windows, when the daemon creates its pipe, then it requires the first instance of the name and sets an access list that allows only the current user
- [ ] Given another process already holds the pipe name, when the daemon starts, then it exits with a message that names the conflict and does not serve
- [ ] Given Windows, when the client connects, then it checks that the pipe's server process runs as the current user before it sends anything, and refuses otherwise with a visible message
- [ ] Given Unix, when a process of another user connects to the socket, then the daemon closes the connection after a peer credential check
- [ ] Given a client of the same user, when it connects, then the hello and welcome exchange succeeds as before

#### US-003: A crash loses almost nothing
**Description:** As a solo builder, I want streamed text written as it arrives and an interrupted turn marked at restart so that a daemon crash costs me at most half a second of text and never leaves a turn spinning.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- OpenBot: delta buffer with a 100 ms debounce and an 8 KiB cap, `openbot/src/backend/agent/delta-buffer.ts:29-60`; recovery at boot that closes stale turns, marks streaming messages interrupted, and expires pending prompts, `openbot/src/backend/agent/boot-recovery.ts:290-335`; reconciliation that never replays an action, `openbot/src/backend/agent/boot-recovery.ts:188-251`.
- Hermes Agent: turn leases in the session store, `hermes-agent/hermes_state_common.py:718`.
- Crow Bot today: the recorder merges text in memory until the next event, `crates/crowbotd/src/store.rs:283-310`.

**Acceptance Criteria:**
- [ ] Given a turn is streaming, when text arrives, then the stored row for that text is written or updated at least every 500 ms
- [ ] Given the daemon is killed during a turn, when it restarts, then the conversation holds all text received up to 500 ms before the kill
- [ ] Given a turn was running when the daemon stopped, when the daemon restarts, then it appends an event that ends the turn as interrupted and the client shows no active turn
- [ ] Given an approval was pending when the daemon stopped, when the daemon restarts, then the approval is recorded as expired and no card waits for an answer
- [ ] Given an interrupted turn, when the user sends a new message, then the provider session resumes and the turn runs

#### US-004: Stop a running turn
**Description:** As a solo builder, I want to stop a bot in the middle of a turn so that I can cut off work that goes the wrong way.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**References:**
- OpenBot: stop sends `turn/interrupt`, `openbot/src/backend/agent-service.ts:6618`; the ACP mapping of start, steer, and interrupt, `openbot/src/backend/acp-client.ts:1283-1307`.
- Hermes Agent: `interrupt()` and `hard_interrupt()`, which abort the model request, signal tool workers, and reach child agents, `hermes-agent/agent/interrupt_control.py:163-276`.
- ACP: `session/cancel` is a notification; the agent then answers the prompt with stop reason `cancelled`.

**Acceptance Criteria:**
- [ ] Given a turn is running, when a client sends a stop request, then the daemon sends the ACP cancel and the turn ends as cancelled within 2 seconds
- [ ] Given an approval is pending when the stop arrives, when the turn is cancelled, then the approval is answered as cancelled and recorded
- [ ] Given the agent does not answer the cancel within 10 seconds, when the timeout passes, then the daemon closes the provider process, records the turn as interrupted, and the next message starts a resumed session
- [ ] Given no turn is running, when a stop request arrives, then the daemon answers with no error and changes nothing

#### US-005: Messages sent during a turn wait in a queue
**Description:** As a solo builder, I want a message I send while the bot works to wait and run next so that I can line up follow-ups without watching the turn end.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**References:**
- OpenBot: busy message modes `queue` and `steer`, `openbot/packages/contracts/src/ipc-queue.ts:18-40`; queue tables with a paused flag, `openbot/src/backend/openbot-database-schema.ts:262-288`; the scheduler that starts queued work, `openbot/src/backend/agent/drain-scheduler.ts`; client-side queue edits that survive a reload, `openbot/src/renderer/src/features/conversation/stores/composer-actions.ts:863-904`.
- Hermes Agent: `steer()` and `redirect()`, `hermes-agent/agent/interrupt_control.py:310-349`; the adapter queues messages while a turn runs and catches `/stop` and `/queue` first, `hermes-agent/gateway/AGENTS.md:19-27`.
- Crow Bot today: a prompt during a turn is refused, `crates/crowbotd/src/conversation.rs:102-345`.

**Acceptance Criteria:**
- [ ] Given a turn is running, when the user sends a message, then the daemon stores it in a queue for that conversation and tells every attached client
- [ ] Given queued messages, when the turn ends, then the daemon starts the next queued message within 500 ms, in the order sent
- [ ] Given a queued message, when the user edits or deletes it before it starts, then the queue reflects the change and the deleted message never runs
- [ ] Given a queued message, when the user chooses "Send now", then the daemon stops the running turn and starts that message next
- [ ] Given the daemon restarts with queued messages, when a client attaches, then the queue is still there and nothing started twice
- [ ] Given a turn ends as failed, when messages wait in the queue, then the queue pauses and the client shows that it is paused with a resume control

---

### EP-002: Bots

A conversation becomes the conversation of a bot: a named teammate with instructions, a folder, a model, and a permission level, that keeps its identity over time.

**Definition of Done:** A person can create, edit, duplicate, and delete bots; each bot's instructions, folder, and model reach its provider session; each bot runs on Ask first or Full access; each bot has a current conversation and a history; the daemon reports each bot's state.

#### US-006: Bot records and the protocol to manage them
**Description:** As a solo builder, I want bots stored by the daemon and managed over the protocol so that every client sees the same team.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- OpenBot: the agent summary and its fields, `openbot/packages/contracts/src/ipc-agents.ts:270-339`; the agent row tied to a thread, not to a provider, `openbot/src/backend/openbot-database-schema.ts:128-136`; state ownership rules, `openbot/docs/architecture/structure.md:67-143`.
- Hermes Agent: the session row and its lineage field, `hermes-agent/hermes_state_common.py:468-536`.
- Crow Bot today: conversations with no owner, `crates/crowbotd/src/store.rs:14-30`; the protocol messages, `crates/crowbot-protocol/src/message.rs`.

**Acceptance Criteria:**
- [ ] Given schema version 1, when the new daemon opens the database, then it migrates to version 2 with a `bots` table and a bot reference on each conversation, and every existing conversation belongs to one bot created by the migration
- [ ] Given the migration fails halfway, when the daemon restarts, then the database is still at version 1 with all rows intact
- [ ] Given a client, when it sends create, update, duplicate, or delete for a bot, then the daemon applies it and sends the change to every attached client
- [ ] Given a bot with a running turn, when a client deletes it, then the daemon stops the turn first and then removes the bot and its conversations
- [ ] Given a name that is empty or longer than 80 characters, when a client creates or renames a bot, then the daemon answers with an error that names the field and stores nothing
- [ ] A bot holds: id, name, title, instructions, avatar color and glyph, provider, model, permission level, workspace folder, pinned and hidden flags, and creation time

#### US-007: A bot's instructions, folder, and model reach the agent
**Description:** As a solo builder, I want each bot to work in its own folder with its own instructions and model so that bots have different jobs.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**References:**
- OpenBot: the workspace and added folders as writable roots, `openbot/src/backend/agent/workspace-sandbox.ts:61-66`; the session binding that switches without losing the thread, `openbot/src/backend/database/provider-sessions.ts:117-167`.
- Hermes Agent: how memory guidance and skills enter the system prompt, `hermes-agent/agent/prompt_builder.py:195-237`; the prompt stays stable inside a session for caching, `hermes-agent/agent/AGENTS.md:57-59`.
- Crow Bot today: the Claude profile and its session options, `crates/crowbotd/src/provider/claude.rs`; session creation, `crates/crowbotd/src/provider/acp.rs`.

**Acceptance Criteria:**
- [ ] Given a new bot with no folder chosen, when it is created, then the daemon creates `~/CrowBot/Bots/<slug>` and uses it as the session's working folder
- [ ] Given a bot with instructions, when a session starts, then the instructions reach the agent as the session's system prompt and the answer to "What are your instructions?" reflects them
- [ ] Given instructions or model changed during a conversation, when the next turn starts, then the daemon starts a new provider session with the new values and the conversation history stays in the database
- [ ] Given a workspace folder that no longer exists, when a turn starts, then the turn fails with a message that names the folder, and the bot stays usable after the folder is changed
- [ ] Given a provider that does not accept a system prompt at session creation, when a session starts, then the daemon sends the instructions as the first part of the first prompt

#### US-008: Permission levels Ask first and Full access
**Description:** As a solo builder, I want to choose for each bot whether it asks before gated actions or acts on its own so that I trust some bots more than others.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**References:**
- OpenBot: access modes, `openbot/packages/contracts/src/ipc-agents.ts:34-40`; the three UI levels as access plus an automatic-approval flag, `openbot/docs/architecture/structure.md:97-112` and `openbot/src/main/approval-automation-store.ts:126`; automatic answers and answering requests that already wait after a grant change, `openbot/src/backend/agent/attention-registry.ts:1023-1076`.
- Hermes Agent: approval choices once, session, and always, `hermes-agent/tools/approval_prompt.py:70-79`; an unavailable transport or an error means deny, `hermes-agent/tools/approval_prompt.py:199-236`.
- Crow Bot today: the daemon selects only allow-once or reject-once, `crates/crowbotd/src/provider/acp.rs`.

**Acceptance Criteria:**
- [ ] Given a bot on Ask first, when the agent requests permission, then the request waits for a person as today
- [ ] Given a bot on Full access, when the agent requests permission, then the daemon selects the allow-once option at once and records the approval as allowed by the permission level
- [ ] Given a bot on Full access, when a request offers no allow-once option, then the daemon declines it and records why
- [ ] Given approvals wait for a bot, when its level changes to Full access, then the daemon allows the waiting requests and the turn continues
- [ ] Given a new bot, when it is created with no level, then its level is Ask first
- [ ] The permission level is read from the bot's record only: nothing in a prompt, a tool result, or a file can change it

#### US-009: Conversations of a bot
**Description:** As a solo builder, I want each bot to have a current conversation, a way to start a new one, and a history so that a bot's work stays organized by topic.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**References:**
- OpenBot: new thread in the agent menu and thread tabs in the header, `openbot/packages/ui/src/features/sidebar/SidebarAgentContextMenu.tsx:42-108` and `openbot/packages/ui/src/features/conversation/ConversationHeader.tsx:118-194`.
- Hermes Agent: session title, archived and pinned flags, `hermes-agent/hermes_state_common.py:468-536`.
- Crow Bot today: the title comes from the first message in the client, `crates/crowbot-desktop/src/workspace.rs:737`.

**Acceptance Criteria:**
- [ ] Given a bot, when the user opens it, then the client shows the bot's current conversation
- [ ] Given a bot, when the user chooses "New chat", then the daemon creates a conversation, makes it current, and the previous one stays in the history
- [ ] Given a conversation with a first user message, when the title is unset, then the daemon sets it from the first 80 characters of that message
- [ ] Given a bot with several conversations, when the user opens the history, then conversations are listed by last activity, newest first, and each one opens with its full events
- [ ] Given a conversation with a running turn, when the user starts a new chat, then the running turn continues in its conversation and shows in the bot's state

#### US-010: Bot state, unread, and needs you
**Description:** As a solo builder, I want the daemon to know which bots work, wait for me, failed, or have unread answers so that the sidebar can show it at a glance.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-009

**References:**
- OpenBot: indicator states (working, limited, responded, unread with a count, waiting with a reason), `openbot/packages/ui/src/features/sidebar/SidebarAgentIndicator.tsx:24-55`; jump to unread, then mark read, `openbot/src/renderer/src/features/conversation/stores/scroll-store.ts:396-438`.
- Hermes Agent: per-session runtime state in the desktop app, `hermes-agent/apps/desktop/src/store/session-states.ts`.

**Acceptance Criteria:**
- [ ] Given a bot, when a turn runs, an approval waits, a turn failed, or events arrived after the last read mark, then the bot's summary reports working, waiting, failed, or unread with a count
- [ ] Given a client shows a conversation at its end, when new events arrive, then the client sends a read mark and the unread count stays at zero
- [ ] Given a client marks a bot unread or read, when the request arrives, then the daemon moves the read mark and tells every attached client
- [ ] Given two states at once, when the summary is built, then waiting wins over failed, failed over working, and working over unread
- [ ] Given a read mark for a conversation that does not exist, when the request arrives, then the daemon answers with an error and changes nothing

---

### EP-003: Desktop shell with OpenBot's layout

The window takes the regions, sizes, and controls of OpenBot's desktop app: a resizable sidebar of bots, a conversation pane, a header, and a side panel.

**Definition of Done:** A screenshot of the Crow Bot window next to OpenBot's shows the same regions in the same order with the same sizes, in dark and light; the sidebar lists bots in OpenBot's groups with their states; every connection state has a screen.

#### US-011: Design tokens and theme
**Description:** As a solo builder, I want the client to use one token set in dark and light so that every screen looks the same and follows my system.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- OpenBot: the only place tokens are declared, `openbot/packages/brand/src/tokens.css` (canvas `#1a1a1a`, frame `#0d0d0d`, surface `#212121`, accent `#007cf7`; sizes 12, 13, 14, 16, 18, 24 px; spacing 2, 4, 6, 8, 12, 16, 24, 32 px; radii 4, 6, 8, 12, 14, 20, 22, 30 px; motion 120, 160, 200, 240 ms); light values, `openbot/packages/brand/src/tokens-light.css`; the appearance control, `openbot/packages/ui/src/features/settings/AppearanceSelect.tsx`.
- Hermes Agent: the desktop app's ownership split between shell and presentation, `hermes-agent/apps/desktop/AGENTS.md:16-27`.
- Crow Bot today: `crates/crowbot-desktop/src/theme.rs`.

**Acceptance Criteria:**
- [ ] Given the theme module, when a view needs a color, a size, a space, a radius, or a duration, then it reads a named token and holds no literal value
- [ ] Given dark and light, when the tokens are compared with OpenBot's two token files, then background, text, border, accent, success, warning, and danger have the same values
- [ ] Given the appearance preference System, Light, or Dark, when it changes or the system theme changes, then the window repaints within 200 ms with no restart
- [ ] Given the preference is unset or unreadable, when the client starts, then it uses System
- [ ] The interface font is Inter with the system stack as fallback, and the monospace font is the system monospace

#### US-012: Window frame and resizable sidebar
**Description:** As a solo builder, I want a sidebar I can resize and collapse next to the conversation so that I choose how much room the bot list takes.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**References:**
- OpenBot: frame grid, `openbot/src/renderer/src/styles/app-shell.css:34-45`; left panel default 280 px, minimum 128, maximum 400, compact 88, collapse below 110, expand above 118, conversation minimum 424, `openbot/src/renderer/src/layout-constants.ts:18-24`; the resizer, `openbot/src/renderer/src/WorkspaceLeftPanelResizer.tsx:21-48` and `openbot/packages/ui/src/components/PanelResizer.tsx`; frame state classes, `openbot/src/renderer/src/WorkspaceFrame.tsx:60-100`.
- Hermes Agent: the sidebar chrome, `hermes-agent/apps/desktop/src/app/chat/sidebar/chrome.tsx`.
- Crow Bot today: a fixed 260 px sidebar, `crates/crowbot-desktop/src/workspace.rs:16-17`.

**Acceptance Criteria:**
- [ ] Given a first start, when the window opens, then the sidebar is 280 px wide and the conversation pane takes the rest
- [ ] Given the user drags the sidebar edge, when the width passes below 110 px, then the sidebar snaps to the 88 px compact form, and when it passes above 118 px it expands again
- [ ] Given a drag, when the width would leave the 128 to 400 px range or leave the conversation under 424 px, then the width stops at the limit
- [ ] Given a width or a collapsed state, when the client restarts, then the sidebar opens the same way, read from the daemon's preferences
- [ ] Given the preferences cannot be read, when the client starts, then it uses the defaults and shows no error
- [ ] The conversation header is the window's drag region, and its controls stay clickable

#### US-013: Sidebar structure and bot rows
**Description:** As a solo builder, I want the sidebar to list my bots in groups with their state so that I see who needs me first.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010, US-012

**References:**
- OpenBot: region order (top bar, search, navigation, footer), `openbot/packages/ui/src/features/sidebar/Sidebar.tsx:15-28`; group order Pinned, Needs you, Unread, sections, Hidden, `openbot/packages/ui/src/features/sidebar/SidebarNav.tsx:55-70`; the bot row (avatar with state, name, time or wait chip, one-line preview), `openbot/packages/ui/src/features/sidebar/SidebarAgentRow.tsx:82-193`; indicators, `openbot/packages/ui/src/features/sidebar/SidebarAgentIndicator.tsx:204-250`; the top bar, `openbot/packages/ui/src/features/sidebar/SidebarTopbar.tsx`; the empty state, `openbot/packages/ui/src/features/sidebar/SidebarEmptyState.tsx`.
- Hermes Agent: sidebar list and ordering, `hermes-agent/apps/desktop/src/app/chat/sidebar/index.tsx` and `hermes-agent/apps/desktop/src/app/chat/sidebar/order.ts`.

**Acceptance Criteria:**
- [ ] Given bots, when the sidebar renders, then it shows, top to bottom: the top bar with collapse, the search box, then the groups Pinned, Needs you, Unread, Bots, and Hidden, each shown only when it holds a bot
- [ ] Given a bot row, when it renders, then it shows the avatar with its state indicator, the name, the time of the last activity or a wait chip, and a one-line preview of the last message
- [ ] Given the compact form, when it renders, then each row shows only the avatar with its indicator, and the name appears in a tooltip
- [ ] Given text in the search box, when it changes, then the list shows only bots whose name or title contains it, within 100 ms
- [ ] Given no bot, when the sidebar renders, then it shows the empty state with a control that opens bot creation
- [ ] Given a bot's state changes in the daemon, when the event arrives, then its row moves to the right group within 200 ms

#### US-014: Sidebar actions
**Description:** As a solo builder, I want the right-click actions on a bot and on the empty area so that I manage my team from the list.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**References:**
- OpenBot: the bot menu (mark read or unread, new thread, edit, duplicate, pin, hide or restore), `openbot/packages/ui/src/features/sidebar/SidebarAgentContextMenu.tsx:42-108`; the empty-area menu (create, new section, mark all read), `openbot/packages/ui/src/features/sidebar/SidebarNav.tsx:94-129`; drag to reorder, `openbot/packages/ui/src/features/sidebar/SidebarAgentRow.tsx:170-193`.
- Hermes Agent: the sidebar context menu, `hermes-agent/apps/desktop/src/app/context-menu`.

**Acceptance Criteria:**
- [ ] Given a right-click on a bot, when the menu opens, then it offers Mark read or Mark unread, New chat, Edit, Duplicate, Pin or Unpin, Hide or Restore, and Delete
- [ ] Given Delete, when the user chooses it, then a confirmation names the bot and states that its conversations are removed, and nothing is deleted before the user confirms
- [ ] Given a right-click on the empty area, when the menu opens, then it offers New bot and Mark all read, and Mark all read is disabled when nothing is unread
- [ ] Given an action fails in the daemon, when the error arrives, then the list returns to its previous state and a notice states the reason
- [ ] Given the keyboard, when a row has focus, then the menu key or Shift+F10 opens the same menu

#### US-015: Conversation header
**Description:** As a solo builder, I want a header that shows which bot I talk to and holds its controls so that settings and the model are one click away.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-012

**References:**
- OpenBot: the header (avatar, name, and title button that opens settings; pending model badge; model picker; side panel toggle), `openbot/packages/ui/src/features/conversation/ConversationHeader.tsx:118-194`; the title that shrinks then hides, `openbot/packages/ui/src/features/conversation/ConversationHeader.tsx:52-116`.
- Hermes Agent: the right sidebar, `hermes-agent/apps/desktop/src/app/right-sidebar`.

**Acceptance Criteria:**
- [ ] Given an open bot, when the header renders, then the left side shows the avatar, the name, and the title, and a click on them opens the bot's settings in the side panel
- [ ] Given the right side, when it renders, then it shows the current model, the conversation history control, and the side panel toggle
- [ ] Given a model change that applies at the next turn, when a turn is running, then the header shows a badge that names the model of the next turn
- [ ] Given a narrow window, when the title no longer fits, then the title shrinks and then hides, and the name stays readable
- [ ] Given no bot is open, when the pane renders, then no header controls are shown

#### US-016: Connection and loading states
**Description:** As a solo builder, I want a clear screen for each connection state so that I always know whether the app can work.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001, US-011

**References:**
- OpenBot: the full-window loading cover, the notices for offline or failed servers with retry, the conversation load error, and the incompatible server screen, `openbot/src/renderer/src/WorkspaceShell.tsx:161-168`, `openbot/src/renderer/src/WorkspaceShell.tsx:242-301`, and `openbot/src/renderer/src/WorkspaceShell.tsx:353-369`.
- Hermes Agent: reconnect handling in the desktop store, `hermes-agent/apps/desktop/src/store/session-states-reconnect.test.ts`.
- Crow Bot today: one text line when no daemon listens, `crates/crowbot-desktop/src/workspace.rs`.

**Acceptance Criteria:**
- [ ] Given the client is starting the daemon, when the window opens, then it shows a loading cover, and the cover goes away when the first bot list arrives
- [ ] Given the connection drops, when the client retries, then the panes stay visible but inert, a notice states that the daemon is unreachable, and the notice goes away on reconnect
- [ ] Given a daemon with another protocol version, when the hello exchange fails, then the window shows both versions and states which side to update, with no retry loop
- [ ] Given a conversation fails to load, when the error arrives, then the pane shows the reason and a retry control
- [ ] Given a reconnect, when it succeeds, then the open conversation reloads its snapshot and shows no duplicate event

---

### EP-004: Conversation experience

The transcript becomes the conversation view of OpenBot: message rows, Markdown that streams, steps for thinking and tools, an approval card, selection, and a composer with stop and queue.

**Definition of Done:** A conversation with Markdown, code, tables, tool calls, an approval, a failed step, and a stopped turn renders as OpenBot renders it; text can be selected and copied; the list follows the stream and stops following when the user scrolls up; a 10,000-event conversation scrolls at 60 frames per second.

#### US-017: Message rows and timeline
**Description:** As a solo builder, I want messages laid out as rows with authors, days, and times so that a long conversation stays readable.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**References:**
- OpenBot: the row component (own messages at the end side, bot messages at the start, avatar, name, bubble, action row), `openbot/packages/ui/src/features/conversation/ChatMessageRow.tsx:79-162`; grouping, `openbot/src/renderer/src/features/conversation/chat-grouping.ts`; day markers, `openbot/packages/ui/src/features/conversation/chat-day-markers.ts`; the virtualizer and row estimates, `openbot/packages/ui/src/features/conversation/createChatVirtualizer.ts:8` and `openbot/src/renderer/src/features/conversation/stores/scroll-store.ts:311-348`; the empty chat, `openbot/packages/ui/src/features/conversation/AgentChatEmptyState.tsx`.
- Hermes Agent: the message table with roles and display metadata, `hermes-agent/hermes_state_common.py:538-569`.
- Zed: the list element, `zed/crates/gpui/src/elements/list.rs`.
- Crow Bot today: `crates/crowbot-desktop/src/transcript.rs`.

**Acceptance Criteria:**
- [ ] Given a conversation, when it renders, then the user's messages sit at the end side in a bubble and the bot's messages sit at the start side with the avatar and the name
- [ ] Given consecutive messages of one author within 5 minutes, when they render, then only the first shows the avatar and the name
- [ ] Given messages on different days, when they render, then a day separator sits between them
- [ ] Given a conversation of 10,000 events, when the user scrolls, then the list draws only rows near the viewport and holds 60 frames per second
- [ ] Given a conversation of more than 200 events, when it opens, then the client loads the newest page first and loads older pages as the user reaches the top
- [ ] Given a bot with no message, when the conversation opens, then the pane shows the empty state with the bot's name and title
- [ ] Given a page of older events fails to load, when the error arrives, then the list keeps what it has and shows a retry control at the top

#### US-018: Markdown that streams
**Description:** As a solo builder, I want the bot's answers rendered as Markdown while they arrive so that I read formatted text, code, and tables without waiting for the end.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017

**References:**
- OpenBot: supported elements and extensions, `openbot/packages/ui/src/features/conversation/MarkdownMessageText.tsx:51-122`; partial Markdown while streaming (a block is final once a blank line and another block follow; only later text is parsed again), `openbot/packages/ui/src/features/conversation/MarkdownMessageText.tsx:205-236`; the parse cache, `openbot/packages/ui/src/features/conversation/MarkdownMessageText.tsx:156-198`; code blocks with copy, `openbot/packages/ui/src/features/conversation/CodeBlock.tsx:181-242`.
- Hermes Agent: streaming events `message.delta` and `message.complete`, `hermes-agent/tui_gateway/hosted_room_server_rpc.py:65-160`.
- Zed: the Markdown element and its parser, `zed/crates/markdown/src/markdown.rs` and `zed/crates/markdown/src/parser.rs`.

**Acceptance Criteria:**
- [ ] Given a message, when it renders, then headings, paragraphs, bold, italic, strikethrough, inline code, links, ordered and unordered lists, task lists, block quotes, tables, horizontal rules, and fenced code blocks render as formatted elements
- [ ] Given a fenced code block, when it renders, then it uses the monospace font, shows its language label, and has a Copy control that switches to a copied state for 2 seconds
- [ ] Given a message that streams, when text arrives, then blocks already closed are not parsed again and the frame time stays under 16 ms for a 20,000-character message
- [ ] Given an unclosed block at the end of the stream (a code fence, a table, a list), when it renders, then it shows as the block it is becoming and never as raw markers that flash
- [ ] Given a link, when the user clicks it, then it opens in the system browser, and a link with a scheme other than `http`, `https`, or `mailto` does not open and shows its target
- [ ] Given Markdown that cannot be parsed as intended, when it renders, then the text shows as written and the message never disappears

#### US-019: Select and copy text
**Description:** As a solo builder, I want to select text in a message and copy it so that I can reuse what a bot wrote.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-018

**References:**
- OpenBot: message actions (copy, reply, react), `openbot/src/renderer/src/features/conversation/stores/message-actions.ts:44-144`; the context menu that keeps the native menu for selected text and links, `openbot/packages/ui/src/features/conversation/ChatMessageRow.tsx:163-210`.
- Hermes Agent: the composer and chat context menu, `hermes-agent/apps/desktop/src/app/chat/composer/context-menu.tsx`.
- Zed: selection inside rendered Markdown, `zed/crates/markdown/src/markdown.rs`.

**Acceptance Criteria:**
- [ ] Given a message, when the user drags across its text, then the selection highlights across paragraphs, lists, and code blocks of that message
- [ ] Given a selection, when the user presses Ctrl+C or Cmd+C, then the clipboard holds the selected text with line breaks between blocks
- [ ] Given a double click and a triple click, when they land on text, then they select the word and the block
- [ ] Given a message, when the user uses its Copy action, then the clipboard holds the message's Markdown source
- [ ] Given a selection in one message, when the user clicks elsewhere, then the selection clears
- [ ] Given a message that still streams, when the user selects text in it, then the selection stays on the same characters as more text arrives

#### US-020: Streaming reveal and scroll following
**Description:** As a solo builder, I want new text to appear word by word and the list to follow it until I scroll away so that I can read live or read back.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017

**References:**
- OpenBot: word-by-word reveal whose speed grows with the backlog, with a floor of 120 characters per second, `openbot/packages/ui/src/features/conversation/streamingReveal.ts:3-59`; the fade of each step, `openbot/packages/ui/src/features/conversation/streamingReveal.ts:111-145`; stick to latest, the jump control past 80 px, and the new-message counter, `openbot/src/renderer/src/features/conversation/stores/scroll-store.ts:286-364`; jump to latest, `openbot/src/renderer/src/features/conversation/stores/scroll-store.ts:440-451`.
- Hermes Agent: the composer's busy state, `hermes-agent/apps/desktop/src/app/chat/composer/busy-submit-state.ts`.

**Acceptance Criteria:**
- [ ] Given a streaming message, when text arrives in bursts, then it appears word by word at 120 characters per second or faster, and the shown text is never more than 1 second behind the received text
- [ ] Given the list is at its end, when new content arrives, then the list stays at its end
- [ ] Given the user scrolls up more than 80 px from the end, when new content arrives, then the list does not move, a "Jump to latest" control appears, and it counts the new messages
- [ ] Given "Jump to latest", when the user clicks it, then the list goes to the end, follows again, and the counter clears
- [ ] Given a turn ends, when the last text is revealed, then the full text is shown within 500 ms with nothing left hidden
- [ ] Given the window is hidden or minimized during a stream, when it comes back, then the message shows all received text at once

#### US-021: Steps for thinking and tool calls
**Description:** As a solo builder, I want a bot's thinking and tool calls gathered in a collapsible list of steps so that I see what it did without the noise.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017

**References:**
- OpenBot: the collapsible header with a step count, a failed badge, the turn duration, and a chevron, `openbot/packages/ui/src/features/conversation/AgentSteps.tsx:240-323`; the list capped at 220 px that follows the newest step while live, `openbot/packages/ui/src/features/conversation/AgentSteps.tsx:27` and `openbot/packages/ui/src/features/conversation/AgentSteps.tsx:65-67`; step kinds and the "Show error" toggle, `openbot/packages/ui/src/features/conversation/AgentSteps.tsx:110-189`; where thinking sits relative to the answer, `openbot/src/renderer/src/features/conversation/stores/scroll-store.ts:82-99`.
- Hermes Agent: tool progress in the desktop store, `hermes-agent/apps/desktop/src/store/tool-progress.ts`.
- Crow Bot today: tool events carry only an id, a title, and a status, `crates/crowbot-protocol/src/event.rs`.

**Acceptance Criteria:**
- [ ] Given a tool call, when the daemon records it, then the stored events carry its kind, its title, its input summary, its output or error text capped at 64 KiB, and its start and end times, and events stored before this change still load
- [ ] Given a turn with steps, when it renders, then one header above the answer shows the step count, a "N failed" badge when a step failed, the turn duration, and a chevron
- [ ] Given the header, when the user clicks it, then the step list opens, at most 220 px tall, scrollable, and it closes on a second click
- [ ] Given a running turn, when steps arrive, then the list shows the newest step and the newest line carries a shine animation
- [ ] Given a failed step, when the list is open, then the step has a "Show error" toggle that reveals the error text in a monospace block
- [ ] Given a thinking step, when it renders, then it shows the thinking text with its own icon, and a stopped turn with no answer shows its thinking in a row of its own
- [ ] Given a tool call with no end event (a crash or a stop), when the turn has ended, then the step shows as interrupted and not as running

#### US-022: Approval card
**Description:** As a solo builder, I want a card that states what the bot asks to do and lets me answer so that I approve with the facts in front of me.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008, US-017

**References:**
- OpenBot: the approval card (title, reason, the command with its working folder, the file root, or the permission list; Deny, Always allow with a confirmation, Allow; only Deny when the request text was cut; buttons disabled while the answer is sent; a failure re-enables them), `openbot/packages/ui/src/features/conversation/ConversationPrompts.tsx:19-172`; questions from the agent, `openbot/src/renderer/src/features/conversation/ConversationQuestionPrompt.tsx`.
- Hermes Agent: the question to the human and its choices, `hermes-agent/tools/approval_prompt.py:110-173`; ACP permission mapping, `hermes-agent/acp_adapter/permissions.py:33-128`; the desktop approval mode store, `hermes-agent/apps/desktop/src/store/approval-mode.ts`.
- Crow Bot today: a card with "Allow once" and "Decline", `crates/crowbot-desktop/src/workspace.rs`.

**Acceptance Criteria:**
- [ ] Given a pending approval, when the card renders, then it shows the tool's title, the command or the file path it targets, and the working folder
- [ ] Given the card, when it renders, then it offers Deny, Always allow, and Allow, in that order
- [ ] Given Always allow, when the user chooses it, then a confirmation states that the bot moves to Full access, and on confirm the daemon changes the level and allows the request
- [ ] Given an answer is being sent, when the user clicks again, then the buttons are disabled, and if the send fails they are enabled again and a notice states the reason
- [ ] Given a request whose text was cut by the size cap, when the card renders, then it offers only Deny
- [ ] Given the approval was answered by another client or expired, when the event arrives, then the card shows the outcome and offers no button

#### US-023: Composer controls, drafts, and the queue panel
**Description:** As a solo builder, I want the composer to send, stop, keep my draft, and show queued messages so that writing to a bot never loses text.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-005

**References:**
- OpenBot: the send control states (Stop while the bot works and the box is empty, Send when there is content), `openbot/packages/ui/src/features/conversation/ComposerSendControl.tsx:10-65`; the send shortcut preference (Enter or Ctrl/Cmd+Enter), `openbot/packages/ui/src/features/conversation/send-shortcut.ts:8-47` and `openbot/packages/ui/src/features/conversation/ComposerEditor.tsx:939-969`; the queue panel (edit, Send now, delete with a confirmation, reorder), `openbot/packages/ui/src/features/conversation/QueuePanel.tsx:340-384` and `openbot/packages/ui/src/features/conversation/QueuePanel.tsx:506-621`; drafts, `openbot/src/renderer/src/features/conversation/composer-draft-storage.ts`; the error banner, `openbot/packages/ui/src/features/conversation/ComposerErrorBanner.tsx`.
- Hermes Agent: drafts and the busy submit state, `hermes-agent/apps/desktop/src/app/chat/composer/hooks/use-composer-draft.ts` and `hermes-agent/apps/desktop/src/app/chat/composer/busy-submit-state.ts`.
- Crow Bot today: the hand-written multi-line input, `crates/crowbot-desktop/src/composer.rs`.

**Acceptance Criteria:**
- [ ] Given the composer, when the bot works and the box is empty, then the control is Stop; when the box has text, then it is Send
- [ ] Given the send shortcut preference, when it is Enter, then Enter sends and Shift+Enter adds a line; when it is Ctrl/Cmd+Enter, then Enter adds a line
- [ ] Given text in the composer, when the user switches bot or closes the client, then the draft is stored by the daemon and comes back when the conversation opens again
- [ ] Given queued messages, when the composer renders, then a panel above it lists them in order with Edit, Send now, and Delete for each
- [ ] Given a send fails (the daemon is unreachable or answers with an error), when the error arrives, then the text stays in the composer and a banner above it states the reason
- [ ] Given a message longer than 100,000 characters, when the user sends it, then the client refuses it and states the limit

#### US-024: Error and interrupted states in the timeline
**Description:** As a solo builder, I want a failed, stopped, or interrupted turn to say so in the conversation so that I never mistake silence for work.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003, US-017

**References:**
- OpenBot: an error message with a status label so it does not read like the bot's text, `openbot/packages/ui/src/features/conversation/MessageRendering.tsx:727-734`; interrupted states, `openbot/packages/ui/src/features/conversation/MessageRendering.tsx:979-987`.
- Hermes Agent: interrupted sessions settling in the store, `hermes-agent/apps/desktop/src/store/session-states-interrupted-settle.test.ts`.
- Crow Bot today: a notice per turn end, `crates/crowbot-desktop/src/transcript.rs:217`.

**Acceptance Criteria:**
- [ ] Given a turn that failed, when it renders, then a labeled error row states the reason in plain words and is styled apart from the bot's text
- [ ] Given a turn stopped by the user, when it renders, then the partial answer stays and a "Stopped" label follows it
- [ ] Given a turn interrupted by a daemon restart, when it renders, then the partial answer stays and a label states that the daemon restarted
- [ ] Given a provider error that asks the user to act (not signed in, usage limit reached), when it renders, then the row names the action and the provider
- [ ] Given a turn that ended normally, when it renders, then no status label is shown

---

### EP-005: Setup and settings

A new user reaches a first answer, and every bot and the app can be configured, with the screens of OpenBot.

**Definition of Done:** A fresh install leads to bot creation, then to a first answer, with the provider's sign-in state shown; every field of a bot is editable in the side panel and saves on its own; the settings dialog holds appearance, send shortcut, and providers; Ctrl+K finds bots, conversations, and messages.

#### US-025: Validate assumption: provider sign-in status and sign-in through ACP
**Description:** As a product owner, I want to know how the daemon can read a provider's sign-in state and start its sign-in so that the setup stories rest on facts.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**References:**
- OpenBot: the driver interface with `signIn`, `authState`, and `validateAccount`, `openbot/src/backend/provider-drivers.ts:753-797`; Claude's sign-in through its CLI and how status is shown, `openbot/docs/provider-terms.md:15-28` and `openbot/src/backend/provider-drivers.ts:987-1026`; Codex login, `openbot/src/backend/agent/codex-login.ts:162-326`.
- Hermes Agent: Hermes as an ACP agent, `hermes-agent/acp_adapter/entry.py:163-213` and `hermes-agent/acp_adapter/server.py`.
- ACP: `InitializeResponse.auth_methods` and `AuthenticateRequest` in `agent-client-protocol-schema` 1.17.0.

**Acceptance Criteria:**
- [ ] A written finding in `docs/` states, for Claude, how the daemon learns "signed in", "signed out", and "runtime missing", with the ACP messages or commands used, each observed in a real run
- [ ] The finding states how the daemon starts the provider's own sign-in without reading or storing a credential, and what the user sees
- [ ] Given the adapter cannot report the state, when the spike concludes, then the finding names the fallback (for example the provider CLI's own status command) and its limits
- [ ] The finding states whether the same method works for Codex and Grok, or what differs, and marks what was not run

#### US-026: First run and bot creation
**Description:** As a solo builder, I want a single screen to create a bot, shown on first run, so that I get from install to a first answer in two steps.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-012, US-025

**References:**
- OpenBot: the single-screen form (title for a first or an additional agent, color picker, face picker, required name, purpose, model picker, Create with a creating state, Cancel, suggestion templates), `openbot/packages/ui/src/features/agents/FirstAgentSetup.tsx:428-626`; draft defaults, `openbot/packages/ui/src/features/agents/FirstAgentSetup.tsx:100-137`; where it mounts, `openbot/src/renderer/src/features/agents/WorkspaceAgentSetup.tsx`.
- Hermes Agent: the setup and profile flows of the desktop app, `hermes-agent/apps/desktop/src/app/profiles`.

**Acceptance Criteria:**
- [ ] Given no bot exists, when the client attaches, then the conversation pane shows the creation screen in its first-bot form
- [ ] Given the screen, when it renders, then it holds a color picker, a glyph picker, a required name, a purpose field, a model picker, Create, and a row of suggestion templates that fill the name and the purpose
- [ ] Given a valid form, when the user clicks Create, then the button shows a creating state, the bot appears in the sidebar, and its conversation opens with the composer focused
- [ ] Given an empty name, when the user clicks Create, then the name field shows an error and no bot is created
- [ ] Given the provider is signed out or its runtime is missing, when the screen renders, then it states so above Create with the control that fixes it, and Create stays available
- [ ] Given a bot already exists, when the user chooses New bot, then the same screen shows in its additional-bot form with Cancel

#### US-027: Bot settings panel
**Description:** As a solo builder, I want a side panel where every field of a bot can be changed and saves on its own so that tuning a bot takes no dialog and no save button.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007, US-008, US-015

**References:**
- OpenBot: the panel's draft store and sub-pages with back and close, `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:322` and `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:1044-1059`; avatar, name, title, and instructions with a length limit, `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:1079-1350`; the "Brain" group (provider, model, reasoning effort), `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:1351-1387`; permissions and the advanced group, `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:1414-1667`; debounced saves and save on blur, `openbot/packages/ui/src/features/conversation/AgentSettingsPanel.tsx:702-721`; folders, `openbot/packages/ui/src/features/conversation/AgentFolders.tsx`.
- Hermes Agent: the settings area of the desktop app, `hermes-agent/apps/desktop/src/app/settings`.

**Acceptance Criteria:**
- [ ] Given the panel, when it opens, then it shows, in order: avatar (color and glyph), name, title, instructions, a Brain group with provider and model, a Permissions group with the level, the workspace folder, and an Advanced group with New chat, Delete history, and Delete bot
- [ ] Given a text field, when the user stops typing for 600 ms or leaves the field, then the value is saved and no save button exists
- [ ] Given an empty name on save, when the field loses focus, then the name returns to its last saved value
- [ ] Given a save fails, when the error arrives, then the field shows a "save failed" state with the reason and keeps the typed text
- [ ] Given instructions longer than 20,000 characters, when the user types past the limit, then the field stops accepting text and shows the limit
- [ ] Given Delete history or Delete bot, when the user chooses it, then a confirmation states what is removed and nothing is removed before the user confirms
- [ ] Given a change to the level, the model, or the instructions, when it is saved, then the header and the sidebar reflect it within 200 ms

#### US-028: Application settings
**Description:** As a solo builder, I want one settings dialog for appearance, the send shortcut, and my providers so that app-wide choices live in one place.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011, US-025

**References:**
- OpenBot: the dialog shell, `openbot/packages/ui/src/features/settings/SettingsDialogShell.tsx`; appearance, `openbot/packages/ui/src/features/settings/AppearanceSelect.tsx`; the send shortcut select, `openbot/packages/ui/src/features/settings/SendShortcutSelect.tsx`; the providers page, `openbot/packages/ui/src/features/settings/ProviderSettingsPage.tsx`.
- Hermes Agent: the settings area and the command palette, `hermes-agent/apps/desktop/src/app/settings` and `hermes-agent/apps/desktop/src/app/command-palette`.

**Acceptance Criteria:**
- [ ] Given the dialog, when it opens, then it has the sections General (appearance, send shortcut), Providers, and About
- [ ] Given Providers, when it renders, then each provider shows its state (ready, signed out, runtime missing) and the control that fixes it
- [ ] Given a provider's sign-in control, when the user uses it, then the provider's own sign-in starts and the state updates within 5 seconds of its end, with no credential shown or stored by Crow Bot
- [ ] Given About, when it renders, then it shows the client version, the daemon version, the protocol version, and the database path
- [ ] Given a preference changes, when it is saved, then it applies at once and every other attached client applies it too
- [ ] Given the daemon is unreachable, when the dialog opens, then the fields are disabled and a notice states why

#### US-029: Global search
**Description:** As a solo builder, I want one search that finds bots, conversations, and messages so that I get back to something a bot said last week.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009, US-013

**References:**
- OpenBot: the Ctrl/Cmd+K shortcut, `openbot/src/renderer/src/global-search-shortcut.ts:1-9`; the search surface, `openbot/packages/ui/src/components/GlobalSearch.tsx`; search inside a conversation, `openbot/packages/ui/src/features/conversation/ChatSearch.tsx:15-78`.
- Hermes Agent: the FTS5 table over message content kept in sync by triggers, and the trigram table for word fragments, `hermes-agent/hermes_state_common.py:984-1009` and `hermes-agent/hermes_state_common.py:1104`; search queries, `hermes-agent/hermes_state_search.py`; the agent-facing tool, `hermes-agent/tools/session_search_tool.py`.

**Acceptance Criteria:**
- [ ] Given Ctrl+K or Cmd+K, when the user presses it, then the search surface opens with the field focused, and the same keys or Escape close it
- [ ] Given a query, when the user types, then results show bots, conversations by title, and messages by content, grouped in that order, within 200 ms for a database of 100,000 events
- [ ] Given a message result, when the user selects it, then its conversation opens scrolled to that message with the match highlighted
- [ ] Given a query with no match, when results render, then the surface states that nothing matches
- [ ] Given a query with FTS operators or quotes that do not parse, when it runs, then the daemon searches it as plain text and returns no error
- [ ] Given events stored before the index existed, when the daemon migrates, then they are indexed and searchable

---

### EP-006: More providers and managed runtimes

Crow Bot runs Codex and Grok next to Claude, chooses models from what each provider offers, and installs the agent runtimes itself.

**Definition of Done:** A bot runs on Claude, Codex, or Grok with approvals owned by the daemon; the model list of each provider comes from the provider; a fresh computer with none of Bun, Node.js, or the adapters installed reaches a first answer after the provider's sign-in.

#### US-030: Provider profiles for Codex and Grok
**Description:** As a solo builder, I want bots on ChatGPT and Grok so that I use the plans I pay for.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007, US-008

**References:**
- OpenBot: the driver registry and the Codex and Grok drivers (start, sign-in, accounts), `openbot/src/backend/provider-drivers.ts:862` and `openbot/src/backend/provider-drivers.ts:951-1084`; Codex sandbox mode and policy per turn, `openbot/src/backend/agent/workspace-sandbox.ts:20-22`; Claude's permission mode, `openbot/src/backend/claude-client.ts:1490`; the ACP client used for Grok, `openbot/src/backend/acp-client.ts:1283-1307`.
- Hermes Agent: approvals mapped to ACP permission requests, `hermes-agent/acp_adapter/permissions.py:33-128`.
- Zed: the two agents it declares, `zed/crates/agent_servers/src/custom.rs:18-19`.
- Crow Bot today: one profile, `crates/crowbotd/src/provider/claude.rs`.

**Acceptance Criteria:**
- [ ] Given a bot on Codex or Grok, when the user sends a message, then the daemon starts that provider's ACP agent and the answer streams as with Claude
- [ ] Given each provider profile, when a session starts, then the profile sets what makes every gated tool call reach the daemon, and a test with a real file write proves that a decline leaves no file
- [ ] Given a provider whose session cannot be put in a mode where the daemon receives approvals, when a session starts, then the turn fails with a message that names the provider and the missing mode, and no tool runs
- [ ] Given the user's own settings for that provider (allowed commands, bypass modes), when a bot session starts, then they do not apply to the bot
- [ ] Given a provider is signed out, when a turn starts, then the turn fails with the provider's name and the sign-in control, and the bot stays usable
- [ ] Given a provider whose agent offers several sign-in methods, when the user starts a sign-in, then the client lists the methods by name and starts the one the user picks (EP-005 starts the first `terminal` method without asking)
- [ ] No ACP type appears outside `crates/crowbotd/src/provider`

#### US-031: Models and reasoning effort from the provider
**Description:** As a solo builder, I want to pick each bot's model from what my plan offers so that I match the model to the job.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**References:**
- OpenBot: the model picker, `openbot/packages/ui/src/components/ProviderModelPicker.tsx`; the pending "model next turn" badge, `openbot/packages/ui/src/features/conversation/ConversationHeader.tsx:157-168`; model lists per driver, `openbot/src/backend/provider-drivers.ts:951-1084`; `model/list` for Claude, `openbot/src/backend/claude-client.ts:955`.
- Hermes Agent: the model stored on the session, `hermes-agent/hermes_state_common.py:468-536`.
- ACP: session config options with the categories model and thought level in `agent-client-protocol-schema` 1.17.0.

**Acceptance Criteria:**
- [ ] Given a provider, when the daemon has an open session or asks for one, then it reads the models and the reasoning levels the provider reports and offers them to clients
- [ ] Given the model picker, when it opens, then it lists the provider's models with the current one marked, and the reasoning levels when the provider has them
- [ ] Given a model chosen during a turn, when the turn is running, then the change applies at the next turn and the header shows the pending badge
- [ ] Given the provider reports no model list, when the picker opens, then it shows the provider's default and states that the list is not available
- [ ] Given a stored model that the provider no longer offers, when a turn starts, then the daemon uses the provider's default, records the change in the conversation, and tells the client

#### US-032: Managed agent runtimes
**Description:** As a solo builder, I want Crow Bot to install the agent runtimes it needs so that I do not install Bun, Node.js, or adapters myself.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-025

**References:**
- OpenBot: it downloads the provider's own binary from the provider's package, checks the integrity hash, does not change the executable, and prefers a path the user set, `openbot/docs/provider-terms.md:15-28`; CLI resolution per driver, `openbot/src/backend/provider-drivers.ts:753-797`.
- Hermes Agent: execution environments behind one base, `hermes-agent/tools/environments/base.py`.
- Zed: how it launches a registry agent, `zed/crates/agent_servers/src/acp.rs`.
- Crow Bot today: the adapter runs through `bunx`, `crates/crowbotd/src/provider/claude.rs`.

**Acceptance Criteria:**
- [ ] Given a provider with no runtime installed, when a bot's first turn starts, then the daemon downloads the pinned version of that provider's ACP agent from its publisher, checks its hash against the pinned value, and stores it under the daemon's data folder
- [ ] Given a download whose hash does not match, when the check runs, then the file is removed, the turn fails with a message that states the mismatch, and nothing is executed
- [ ] Given a download in progress, when a client is attached, then it shows the progress and the turn starts when the download ends
- [ ] Given no network, when a runtime is missing, then the turn fails with a message that names the runtime and the bot stays usable once the network is back
- [ ] Given the user set a path to their own provider executable, when a session starts, then the daemon uses that executable unchanged
- [ ] Given a fresh computer with none of Bun or Node.js installed, when the provider is signed in, then a Claude bot answers

#### US-033: A provider switch keeps the bot
**Description:** As a solo builder, I want to move a bot to another provider and keep its conversation so that a usage limit on one plan does not end the work.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-030

**References:**
- OpenBot: the agent row points to a thread and provider sessions are child rows, so a switch adds a session and keeps the agent, `openbot/src/backend/openbot-database-schema.ts:199-214`; binding a new session and turning off the previous one in one dispatch, `openbot/src/backend/database/provider-sessions.ts:117-167`.
- Hermes Agent: compaction into a structured summary by a side model, `hermes-agent/agent/AGENTS.md:77-92`; session lineage, `hermes-agent/hermes_state_common.py:468-536`.

**Acceptance Criteria:**
- [ ] Given a bot with a conversation, when its provider changes, then the conversation's events stay in place and the next turn runs on the new provider
- [ ] Given the first turn after a switch, when it starts, then the new session receives a handoff built from the conversation's stored events, capped at 50,000 characters, and the answer shows that the bot knows the earlier exchange
- [ ] Given a switch during a running turn, when it is saved, then the running turn ends on the old provider and the change applies at the next turn
- [ ] Given the new provider is signed out, when the switch is saved, then the bot keeps the new provider, the next turn fails with the sign-in control, and switching back restores the old session
- [ ] Given a switch back to a provider used before, when the next turn starts, then the daemon resumes that provider's earlier session when it still exists

## Functional Requirements

- FR-01: The client must start the daemon when none listens, and the daemon must outlive the client.
- FR-02: The daemon must accept connections only from the user that runs it, and the client must refuse a daemon run by another user.
- FR-03: The daemon must write streamed text at least every 500 ms and must end, at restart, every turn that was running.
- FR-04: The daemon must store bots, their conversations, their queue, their read marks, and client preferences, and must send every change to all attached clients.
- FR-05: The daemon must pass a bot's instructions, working folder, and model to its provider session.
- FR-06: When a bot is on Ask first, the daemon must hold each permission request until a person answers. When it is on Full access, the daemon must select the allow-once option itself.
- FR-07: The daemon must NOT let a prompt, a tool result, or a file change a bot's permission level or answer an approval.
- FR-08: The daemon must stop a running turn on request and must record how the turn ended.
- FR-09: When a user sends a message during a turn, the daemon must queue it and run it after the turn.
- FR-10: The client must render the window with the regions, sizes, and groups this PRD reads in OpenBot's source.
- FR-11: The client must render Markdown while it streams, show steps for thinking and tools, and let the user select and copy text.
- FR-12: The client must NOT keep durable state: preferences and drafts are stored by the daemon.
- FR-13: The daemon must run Claude, Codex, and Grok through ACP, and must fail a turn when it cannot own that provider's approvals.
- FR-14: The daemon must install the agent runtimes it needs, verify each download, and run provider binaries unchanged.
- FR-15: The daemon must NOT read, copy, or store a provider credential.

## Non-Functional Requirements

- **Performance:** the client shows its first frame within 800 ms of launch on the reference computer (Windows 11, 8 cores); a conversation of 10,000 events scrolls at 60 frames per second; a frame during streaming takes under 16 ms for a 20,000-character message; opening a conversation of 5,000 events takes under 300 ms; search answers within 200 ms on 100,000 events.
- **Security:** the endpoint accepts only the same user (access list on Windows, mode 0600 and a peer credential check on Unix); zero provider credentials in the database, the logs, or the protocol; every runtime download is checked against a pinned SHA-256; the risks ASI01 and ASI02 of the OWASP Top 10 for Agentic Applications 2026 are covered by FR-06 and FR-07.
- **Accessibility:** every control is reachable with the keyboard; focus is visible on every control; text and its background keep a contrast ratio of 4.5:1 or more in dark and light; the interface font size is 13 px or more.
- **Scalability:** one daemon runs 10 bots with 10 turns at once with no dropped event; the database holds 1,000,000 events with conversation open and search inside the targets above.
- **Reliability:** a daemon killed during a turn loses at most 500 ms of streamed text; zero turns shown as running after a restart; a stop request ends a turn within 2 seconds, or within 10 seconds when the provider does not cooperate; the client reconnects within 2 seconds of the daemon coming back.
- **Resource use:** the daemon idles under 60 MB of memory with 10 bots and no turn; the client stays under 300 MB with a 10,000-event conversation open.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Empty state | First run, no bot | Show the bot creation screen | "Create your first bot" |
| 2 | Empty conversation | A bot with no message | Show the bot's name and title, composer focused | None |
| 3 | Loading state | The client starts the daemon | Loading cover until the bot list arrives | None |
| 4 | Daemon unreachable | The daemon stops while the client is open | Panes inert, notice, automatic reconnect | "The daemon is not reachable. Reconnecting." |
| 5 | Version mismatch | Client and daemon differ in protocol version | Screen with both versions, no retry loop | "This client speaks protocol N, the daemon speaks M. Update the older one." |
| 6 | Provider signed out | A turn starts with no sign-in | The turn fails, the bot stays usable | "Claude is signed out. Sign in to continue." |
| 7 | Runtime missing | No agent runtime, no network | The turn fails with the runtime's name | "The Claude runtime is not installed and cannot be downloaded offline." |
| 8 | Crash during a turn | The daemon is killed while text streams | Text kept up to 500 ms before the kill; the turn ends as interrupted | "The daemon restarted during this turn." |
| 9 | Approval on restart | The daemon stops while a card waits | The approval is expired; no card waits | "This request expired when the daemon restarted." |
| 10 | Concurrent answers | Two clients answer the same approval | The first answer wins; the second sees the outcome | "Already answered." |
| 11 | Boundary value | A message over 100,000 characters | The client refuses to send | "A message can hold 100,000 characters at most." |
| 12 | Boundary value | Sidebar dragged past its limits | The width stops at 128 or 400 px | None |
| 13 | Interrupted flow | The client closes with a draft and a queue | Both are stored by the daemon and come back | None |
| 14 | Undo and reversal | Delete a bot | A confirmation names what is removed; no undo after confirm | "Delete Atlas and its 4 conversations? This cannot be undone." |
| 15 | External dependency | The provider reaches a usage limit | The turn fails with the provider's message; the queue pauses | "Claude reached its usage limit. It resets at 14:00." |
| 16 | Endpoint taken | Another process holds the pipe name | The daemon refuses to serve; the client refuses to attach | "Another program is using Crow Bot's endpoint." |
| 17 | Missing folder | A bot's workspace folder was removed | The turn fails and names the folder | "The folder C:\\Work\\atlas does not exist. Choose another one in the bot's settings." |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | The ACP Rust SDK changes often (three releases between 2026-11-02 and 2026-11-10, one major) | High | Med | Pin the exact version; keep ACP types inside `crates/crowbotd/src/provider`; run a real-agent smoke test on each bump |
| 2 | An adapter update changes how settings or modes work, and a bot inherits the owner's permissions | Med | High | Pin adapter versions; fail the turn when the wanted mode is missing; a test with a real declined write for each provider |
| 3 | Text selection and the Markdown renderer on GPUI take more work than sized | Med | Med | Two separate L stories; Zed's Markdown element as reference; ship rendering before selection |
| 4 | GPUI changes before 1.0 and breaks the client on a rev bump | Med | Med | Pin the rev; bump only on purpose, with the patch entries copied from Zed at that rev |
| 5 | A provider changes its terms on plan sign-in in third-party products | Med | High | Run provider binaries unmodified and never touch credentials, which is the permitted case today; keep API key sign-in working through the same CLI |
| 6 | Approval fatigue pushes users to Full access for every bot | High | Med | Ask first stays the default; Always allow needs a confirmation; a later PRD adds the sandboxed level |
| 7 | Content read by a bot tries to steer it (prompt injection) | High | High | Approvals come only from a person or the bot's level (FR-07); the level cannot change from content |
| 8 | Named pipe squatting or impersonation on Windows | Low | High | First-instance flag, access list, and the client's check of the server's user (US-002) |
| 9 | OpenBot's interface is a moving target (2,198 commits in 30 days) | High | Low | Parity is measured against the source at the time each story is built, as a base to iterate on, not as a tracking goal |
| 10 | The noncommercial daemon license slows adoption by teams | Med | Med | Personal use stays free; the paid license is part of the pricing model in `docs/product.md` |

## Non-Goals

What this version explicitly does NOT include:

- **The bot's own computer** (an isolated browser and workspace, visible live). It is the next PRD, after its own prototype. References for it: `openbot/src/backend/browser-tools.ts`, `hermes-agent/tools/environments/`.
- **Memory, skills, and the learning loop.** A later PRD. References: `hermes-agent/tools/memory_tool_store.py:177`, `hermes-agent/agent/background_review.py`, `hermes-agent/skills/AGENTS.md:16-81`, `hermes-agent/agent/curator.py`.
- **Scheduled work and messaging channels.** A later PRD. References: `hermes-agent/cron/scheduler.py`, `hermes-agent/cron/jobs.py`, `hermes-agent/gateway/AGENTS.md`.
- **Servers and teams** (a daemon on a server, several members, the hosted offer, the remote transport with encryption and sign-in). A later PRD. Reference: `openbot/docs/architecture/structure.md`.
- **The sandboxed permission level** ("Workspace only"). It needs process confinement and comes with the bot's computer. Reference: `openbot/src/backend/agent/workspace-sandbox.ts:61-99`.
- **Steering a running turn.** ACP version 1 has no steer; the queue and "Send now" cover the need. Reference: `hermes-agent/agent/interrupt_control.py:310-349`.
- **Attachments, images, voice, mentions, reactions, replies, and pinned messages** in the composer and the timeline. Reference for attachments: `openbot/src/backend/agent/delivery-content.ts:394-402`.
- **Math and diagram rendering.** Math and Mermaid blocks show as code in this version. Reference: `openbot/packages/ui/src/features/conversation/MathFormula.tsx`.
- **Bot-to-bot messages and subagents shown as bots.** Reference: `hermes-agent/tools/delegate_tool.py:55`.
- **Translations.** The interface is in English.
- **Installers, signing, and automatic updates.** A distribution PRD follows this one.
- **macOS and Linux clients as a release target.** The code stays portable, and the Unix socket path is built, but this PRD is verified on Windows.

## Files NOT to Modify

- `crates/crowbot-desktop/LICENSE`, `crates/crowbot-protocol/LICENSE`, `crates/crowbotd/LICENSE.md`, `crates/crowbotd/NOTICE`: the license texts and the required notice.
- The `SCHEMA_V1` text in `crates/crowbotd/src/store.rs`: a released schema is never edited; a change is a new migration.
- The existing variants and field names of `Event` in `crates/crowbot-protocol/src/event.rs`: stored rows must keep loading; add fields with a default and add variants.
- The `[patch.crates-io]` entries and the GPUI rev in `crates/crowbot-desktop/Cargo.toml`: they change only together, when the rev is bumped on purpose.
- `docs/product.md`: product decisions change with the product owner, not inside a story.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** Should the client embed the path of `crowbotd`, or find it next to its own executable? Recommended: next to its own executable, with an override flag. Engineering to confirm how a detached process is started on Windows without a console window.
- **Data Model:** Version 2 adds `bots`, a bot reference and a title on conversations, `queue`, `read_marks`, and `preferences`. Should streamed text update one row in place or append chunk rows merged on read? Trade-off: an in-place update needs no merge on read and rewrites a growing row; chunk rows are append-only and need a merge. Recommended: update in place with a cap, then start a new row.
- **Protocol:** New messages for bots, queue, stop, read marks, preferences, drafts, paging, search, and provider state. Should `PROTOCOL_VERSION` go to 2 with a refusal of version 1, or stay compatible? Recommended: go to 2; no version 1 client is in use. Paging: by event id cursor, newest first.
- **Events:** Tool detail (kind, input, output, times) extends the tool events. Should large tool output live in the event or in a side table? Recommended: in the event up to 64 KiB, cut beyond with a marker.
- **Dependencies:** `pulldown-cmark` for Markdown (it has no incremental input, so the renderer freezes closed blocks and parses only the open tail); `windows-sys` for the pipe's security descriptor and the server process check. Code highlighting: tree-sitter grammars, `syntect`, or none in this version? Recommended: none at first, behind the code block element.
- **Rendering:** Should the timeline use GPUI's `list` with measured rows, or `uniform_list` with fixed heights? Recommended: `list`, as rows vary. Engineering to measure with 10,000 events.
- **Preferences:** One key-value table scoped to the daemon. Should a preference be per client device or shared? Recommended: shared, with the sidebar width as the one per-device value.
- **Migration:** Version 1 databases exist only on the author's computer. Backward compatibility: yes, the migration keeps every row. Rollback plan: the migration runs in one transaction; a failure leaves version 1.
- **ACP version 2:** The draft removes `session/load` and changes resume. Recommended: stay on version 1 and gate version 2 behind negotiation when it is stable.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Stories certified `DONE` | 0 of 33 | 16 | Month-1 | `tasks/prd-crow-bot-desktop-core-status.json` |
| Stories certified `DONE` | 0 of 33 | 33 | Month-6 | `tasks/prd-crow-bot-desktop-core-status.json` |
| Manual steps from install to first answer | 3 (start the daemon, start the client, create a conversation) | 2 | Month-1 | A scripted first run on a clean profile |
| Streamed text lost on a daemon kill | All text since the last non-text event | 500 ms or less | Month-1 | A test that kills the daemon during a stream and compares stored and received text |
| Turns shown as running after a restart | Not measured | 0 | Month-1 | A test that restarts the daemon during a turn |
| Frame rate while scrolling 10,000 events | Not measured | 60 frames per second | Month-6 | GPUI frame timing on the reference computer |
| Providers with daemon-owned approvals | 1 | 3 | Month-6 | One declined-write test per provider |
| Bots run in parallel by the author in daily use | 1 | 5 | Month-6 | A count of bots with a turn in the last 7 days, read from the database |

## Open Questions

- What do bot avatars look like beyond a color and a glyph? The product owner decides before US-026; it shapes the creation screen and the sidebar rows.
- Do OpenAI and xAI permit plan sign-in inside a third-party product, as Anthropic does for the unmodified Claude Code binary? The product owner checks their terms before US-030 ships; it decides whether those providers are offered with plan sign-in or with an API key only.
- Where does the daemon keep bot folders by default on a server, where `~` belongs to a service account? Engineering answers it in the servers PRD; US-007 uses `~/CrowBot/Bots`.
- Should a bot's Claude configuration be separate from the owner's `~/.claude`? Open in `AGENTS.md`; it waits for the bot's computer PRD.
- Which code highlighting does the client ship, and when? Engineering proposes after US-018, from frame time measurements.
- Does the client need a tray icon or a start-at-login option so that the daemon runs after a reboot? The product owner decides with the distribution PRD.
[/PRD]
