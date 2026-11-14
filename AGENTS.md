# Crow Bot: repository guidance

## Product

Crow Bot gives a person or a team AI teammates, called bots. Each bot has its own computer, works without supervision, and learns from use. Personal use on the user's own computer is free. Revenue comes from hosted servers and from licenses for use at work.

## Architecture

- **Daemon (`crowbotd`)**: a headless Rust binary. It owns bots, conversations, memory, skills, and storage. It runs on the user's computer or on a Linux server, as one binary with no display. It listens on a local endpoint: a named pipe on Windows, a Unix socket elsewhere. Each conversation is one task (`conversation.rs`) that records events, sends them to the attached clients, and holds the pending approvals.
- **Desktop client (`crates/crowbot-desktop`, binary `crowbot`)**: Rust with GPUI. It is a thin client: it attaches to a daemon through one protocol, local or remote, and holds no durable state. It rebuilds what it shows from the snapshot the daemon sends when a conversation opens.
- **Protocol crate (`crates/crowbot-protocol`)**: written in this repository and shared by the daemon and the client. One JSON message per line, after a `hello` that carries `PROTOCOL_VERSION`. It owns the `Event` type, and its `transport` feature holds the local listener and connector.
- **Two Cargo workspaces.** The root workspace holds `crowbotd` and `crowbot-protocol`. `crates/crowbot-desktop` is its own workspace, with its own `Cargo.lock`, and takes the protocol crate by path. Run Cargo for the client from `crates/crowbot-desktop` or with `--manifest-path`. The split keeps Zed's dependency patches and GPUI's dependency versions out of the daemon.
- **Providers**: Claude, ChatGPT (Codex), and Grok. The daemon starts each one through the Agent Client Protocol (ACP): `claude-agent-acp`, `codex-acp`, and the `grok` CLI. Build and test Claude first.
- **Memory, skills, and learning** live in the daemon. The daemon serves them to provider CLIs through MCP.

## Hard rules

- **License boundary.** The desktop client is GPL-3.0-or-later and may depend on Zed's GPL crates. The daemon and the protocol crate must not depend on any GPL crate. The daemon will carry a noncommercial license; the exact license is not chosen yet.
- **Model usage belongs to the user.** A bot runs on the user's own subscription or API key. Never pay for, resell, or proxy model usage. Never read, copy, or store provider credentials: sign-in happens in the provider's own CLI. Run provider binaries unmodified.
- **The daemon owns approvals.** A bot must not inherit what its owner allowed for personal use. The Claude profile (`provider/claude.rs`) starts each session with `settingSources: []`, refuses bypass mode, and sets the session mode to `default`, so every gated tool call reaches the daemon as an approval request. A turn fails when the agent does not offer that mode. A request waits until a client answers it, and the daemon only ever selects the allow-once or the reject-once option: it declines a request that offers no allow-once option.
- **The database is the source of truth.** A conversation is the ordered list of its events in SQLite (`store.rs`). The provider session is only a handle to resume the agent; the daemon never reads history back from a provider. Schema changes go through `PRAGMA user_version`, keep all user data, and a daemon refuses a database that a newer daemon wrote. A stored event is the JSON of the protocol `Event`, so a change to `Event` must still read the rows already stored: add fields with `#[serde(default)]`, never rename or remove a variant.
- **ACP stays at the edge.** ACP types do not leave the provider adapter. Inside, the daemon records its own events (message, tool call, approval request).
- **GPUI source.** GPUI comes from Zed's git repository at a pinned rev, with Zed's `[patch.crates-io]` entries. Never use the crates.io `gpui`. The rev and the patches live in `crates/crowbot-desktop/Cargo.toml`; when the rev changes, copy the patch entries from Zed's `Cargo.toml` at that rev.
- **Reference projects.** OpenBot, Hermes Agent, and Zed are references for ideas: read them to learn how a problem was solved, then design the answer for this codebase. Never copy or translate OpenBot code or text; its license is noncommercial.

## Open decisions

- Full isolation of a bot from the user's `~/.claude`. The Claude profile already ignores the user's settings and instructions, but the bot still signs in with the user's credentials and writes its transcripts there. A bot-owned `CLAUDE_CONFIG_DIR` would isolate both, probably at the price of a separate sign-in for each bot (not verified).

- The remote transport. The protocol runs over any byte stream, but only the local endpoint exists: encryption and client sign-in for a daemon on a server are not designed.
- Who may reach the local endpoint. The Windows pipe keeps the default access rules and refuses remote clients; the Unix socket is readable and writable by its owner only, and the daemon drops a connection from another user. The Windows daemon does not check who the client is, and no client checks who the daemon is.
- The exact license of the daemon.
- Whether a native Rust client for the Claude CLI replaces `claude-agent-acp` later. If it does, it plugs in as an ACP agent, so the daemon does not change.
