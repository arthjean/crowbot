# Crow Bot

AI teammates that finish the work. Each bot has its own computer, keeps working when your laptop is closed, and learns how you work.

Crows learn by watching, use tools, and remember. So do the bots.

## Status

First prototype. The daemon starts Claude through the Agent Client Protocol, sends one prompt, and prints the events of the turn.

```sh
cargo run -p crowbotd -- --cwd <dir> "<prompt>"
```

It needs Bun, Node.js 22 or later, and a signed-in Claude Code. The daemon declines every approval request unless you pass `--approve`. `--agent "<command>"` starts another ACP agent.

## Shape

- `crowbotd`: a headless Rust daemon that owns the bots, their conversations, their memory, and their computers. It runs on your computer or on a server.
- A native desktop client written in Rust with GPUI. It attaches to a daemon, local or remote.
- Bots run on Claude, ChatGPT, or Grok, with your own subscription or your own API key.

## Pricing model

Personal use on your own computer is free. Hosted servers and licenses for use at work are paid.
