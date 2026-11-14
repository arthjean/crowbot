# Crow Bot

AI teammates that finish the work. Each bot has its own computer, keeps working when your laptop is closed, and learns how you work.

Crows learn by watching, use tools, and remember. So do the bots.

## Status

Prototype. The daemon starts Claude through the Agent Client Protocol, keeps a conversation across turns and restarts, and records every event in SQLite.

```sh
cargo run -p crowbotd -- --cwd <dir> "<prompt>"
cargo run -p crowbotd -- --conversation <id> "<prompt>"
cargo run -p crowbotd -- --conversation <id> --history
```

- With a prompt, the daemon runs one turn. Without one, it reads one prompt per line from standard input.
- `--conversation <id>` continues a conversation, in the folder where it started.
- `--history` prints a conversation from the database and does not start the agent.
- `--approve` allows each approval request once. Without it, the daemon declines them all.
- `--db <path>` selects the database. The default is `~/.crowbot/crowbot.db`.
- `--agent "<command>"` starts another ACP agent.

It needs Bun, Node.js 22 or later, and a signed-in Claude Code.

## Shape

- `crowbotd`: a headless Rust daemon that owns the bots, their conversations, their memory, and their computers. It runs on your computer or on a server.
- A native desktop client written in Rust with GPUI. It attaches to a daemon, local or remote.
- Bots run on Claude, ChatGPT, or Grok, with your own subscription or your own API key.

## Pricing model

Personal use on your own computer is free. Hosted servers and licenses for use at work are paid.
