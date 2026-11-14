# Crow Bot: repository guidance

## Product

Crow Bot gives a person or a team AI teammates, called bots. Each bot has its own computer, works without supervision, and learns from use. Personal use on the user's own computer is free. Revenue comes from hosted servers and from licenses for use at work.

## Architecture

- **Daemon (`crowbotd`)**: a headless Rust binary. It owns bots, conversations, memory, skills, and storage. It runs on the user's computer or on a Linux server, as one binary with no display.
- **Desktop client**: Rust with GPUI. It is a thin client: it attaches to a daemon through one protocol, local or remote, and holds no durable state.
- **Protocol crate**: written in this repository and shared by the daemon and the client.
- **Providers**: Claude, ChatGPT (Codex), and Grok. The daemon starts each one through the Agent Client Protocol (ACP): `claude-agent-acp`, `codex-acp`, and the `grok` CLI. Build and test Claude first.
- **Memory, skills, and learning** live in the daemon. The daemon serves them to provider CLIs through MCP.

## Hard rules

- **License boundary.** The desktop client is GPL-3.0-or-later and may depend on Zed's GPL crates. The daemon and the protocol crate must not depend on any GPL crate. The daemon will carry a noncommercial license; the exact license is not chosen yet.
- **Model usage belongs to the user.** A bot runs on the user's own subscription or API key. Never pay for, resell, or proxy model usage. Never read, copy, or store provider credentials: sign-in happens in the provider's own CLI. Run provider binaries unmodified.
- **The daemon owns approvals.** A bot must not inherit what its owner allowed for personal use. The Claude profile (`provider/claude.rs`) starts each session with `settingSources: []`, refuses bypass mode, and sets the session mode to `default`, so every gated tool call reaches the daemon as an approval request. A turn fails when the agent does not offer that mode.
- **ACP stays at the edge.** ACP types do not leave the provider adapter. Inside, the daemon records its own events (message, tool call, approval request).
- **GPUI source.** GPUI comes from Zed's git repository at a pinned rev, with Zed's `[patch.crates-io]` entries. Never use the crates.io `gpui`.
- **Reference projects.** OpenBot, Hermes Agent, and Zed are references for ideas: read them to learn how a problem was solved, then design the answer for this codebase. Never copy or translate OpenBot code or text; its license is noncommercial.

## Open decisions

- Full isolation of a bot from the user's `~/.claude`. The Claude profile already ignores the user's settings and instructions, but the bot still signs in with the user's credentials and writes its transcripts there. A bot-owned `CLAUDE_CONFIG_DIR` would isolate both, probably at the price of a separate sign-in for each bot (not verified).

- The exact license of the daemon.
- Whether a native Rust client for the Claude CLI replaces `claude-agent-acp` later. If it does, it plugs in as an ACP agent, so the daemon does not change.
