# Crow Bot

AI teammates that finish the work. Each bot has its own computer, keeps working when your laptop is closed, and learns how you work.

Crows learn by watching, use tools, and remember. So do the bots.

## Status

Prototype. The daemon listens on a local endpoint, starts Claude through the Agent Client Protocol, and records every event in SQLite. The desktop client attaches to it, shows a conversation as it streams, and asks you to allow or decline each gated tool call.

Start the daemon, then the client:

```sh
cargo run -p crowbotd -- --cwd <dir>
cargo run -p crowbot-desktop
```

The daemon:

- `--cwd <dir>` is the folder where new conversations work. The default is the folder the daemon starts in.
- `--db <path>` selects the database. The default is `~/.crowbot/crowbot.db`.
- `--endpoint <address>` selects where it listens. The default is the named pipe `\\.\pipe\crowbotd-<user>` on Windows and the socket `~/.crowbot/crowbotd.sock` elsewhere.
- `--agent "<command>"` starts another ACP agent for new conversations.

The client:

- It opens the most recent conversation. Closing the window does not stop a turn: the daemon keeps working, and an approval request waits until a client answers it.
- Enter sends the message, Shift+Enter adds a line.
- `--endpoint <address>` attaches to a daemon that listens somewhere else.
- When no daemon is running, the window says so and reconnects on its own.

It needs Bun, Node.js 22 or later, and a signed-in Claude Code.

## Shape

- `crowbotd`: a headless Rust daemon that owns the bots, their conversations, their memory, and their computers. It runs on your computer or on a server.
- `crowbot` (the `crowbot-desktop` crate): a native desktop client written in Rust with GPUI. It attaches to a daemon, local today and remote later, and keeps no state of its own.
- `crowbot-protocol`: the messages the daemon and its clients exchange, one JSON message per line.
- Bots run on Claude, ChatGPT, or Grok, with your own subscription or your own API key.

## Pricing model

Personal use on your own computer is free. Hosted servers and licenses for use at work are paid.
