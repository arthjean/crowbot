# Crow Bot

AI teammates that finish the work. Each bot has its own computer, keeps working when your laptop is closed, and learns how you work.

Crows learn by watching, use tools, and remember. So do the bots.

## Status

Prototype. The daemon listens on a local endpoint, runs each bot on Claude, ChatGPT, or Grok through the Agent Client Protocol, and records every event in SQLite. The desktop client attaches to it, shows a conversation as it streams, and asks you to allow or decline each gated tool call.

Build the daemon, then start the client. The client starts the daemon when none is running:

```sh
cargo build -p crowbotd
cargo run --manifest-path crates/crowbot-desktop/Cargo.toml -- --daemon target/debug/crowbotd
```

The client is its own Cargo workspace, so its commands run from `crates/crowbot-desktop` or with `--manifest-path`.

To try a change apart from your own data, `scripts/sandbox.ps1` (Windows) or `zsh scripts/sandbox.zsh` (Linux, macOS) builds both and opens the client on a throwaway daemon with its own database, runtimes, and provider sign-ins. `stop`, `reset`, and `clean` end it, wipe it, or remove it.

The daemon:

- `--db <path>` selects the database. The default is `~/.crowbot/crowbot.db`.
- `--bots-dir <dir>` is where a bot created with no folder gets one of its own. The default is `~/CrowBot/Bots`.
- `--data-dir <dir>` holds the agent runtimes the daemon installs and the files providers keep for bots. The default is the folder of the database.
- `--endpoint <address>` selects where it listens. The default is the named pipe `\\.\pipe\crowbotd-<user>` on Windows and the socket `~/.crowbot/crowbotd.sock` elsewhere. Only the user who runs the daemon can attach to it.
- `--agent "<command>"` runs another ACP agent, which every bot then runs in place of the three providers.
- `--runtime-manifest <file>` replaces the pinned runtimes with the ones a JSON file lists, for a mirror.

The client:

- It opens the most recent conversation. Closing the window does not stop a turn: the daemon keeps working, and an approval request waits until a client answers it.
- Enter sends the message, Shift+Enter adds a line. A message sent while the bot works waits in a queue and runs next. Stop ends the running turn.
- `--endpoint <address>` attaches to a daemon that listens somewhere else.
- When no daemon is running, the client starts `crowbotd` from the folder of its own executable, or from `--daemon <path>`. The daemon keeps running after the window closes and logs to `~/.crowbot/crowbotd.log`. Arguments after `--` go to the daemon it starts.
- When the daemon cannot start, the window states the reason and offers to retry. When the daemon stops, the client starts it again.

No Bun, Node.js, or adapter is needed: the first time a bot needs a provider, the daemon downloads that provider's agent at a pinned version, checks it against a pinned SHA-256, and keeps it under `--data-dir`. Settings > Providers can install a runtime ahead of time, or use your own executable instead. You sign in to each provider once from the same place, with your own subscription.

## Shape

- `crowbotd`: a headless Rust daemon that owns the bots, their conversations, their memory, and their computers. It runs on your computer or on a server.
- `crowbot` (the `crowbot-desktop` crate): a native desktop client written in Rust with GPUI. It attaches to a daemon, local today and remote later, and keeps no state of its own.
- `crowbot-protocol`: the messages the daemon and its clients exchange, one JSON message per line.
- Bots run on Claude, ChatGPT, or Grok, with your own subscription or your own API key.

## Pricing model

Personal use on your own computer is free. Hosted servers and licenses for use at work are paid.

## Licenses

| Crate | License |
| --- | --- |
| `crates/crowbot-desktop` | GPL-3.0-or-later |
| `crates/crowbot-protocol` | Apache-2.0 |
| `crates/crowbotd` | PolyForm Noncommercial 1.0.0 |

Each crate holds its license text.
