# Crow Bot: product note

Read this before writing or reading a PRD. It states what the product is, for whom, and how it earns money. It does not describe how the code works; `AGENTS.md` does.

## What it is

Crow Bot gives a person or a team AI teammates, called bots. You talk to a bot like a colleague: you hand it work, it does the work on its own computer, and it comes back when it needs your approval or when it is done.

Three things define a bot:

- **Its own computer.** A bot works in its own workspace, with its own browser and files, so it can use apps and websites the way you do.
- **It keeps working.** A bot runs in a daemon, not in a window. Closing the app does not stop it, and a daemon on a server works around the clock.
- **It learns.** A bot keeps what it learns from conversations, builds skills from the tasks it completes, and improves them with use.

## Who it is for

- **A person** who wants several bots working in parallel on their own computer, with the AI plans they already pay for.
- **A small team** that wants shared bots on a server, each with one job, that everyone can talk to.

## What makes it different

- **Local first.** Conversations, files, and memory stay on the computer that runs the daemon: yours, or a server you choose.
- **Your own AI plans.** A bot runs on your subscription or your API key, through the provider's own tool. Crow Bot never sells or resells model usage.
- **Native.** The desktop client is written in Rust with GPUI. It starts fast, stays smooth, and uses little memory.
- **You stay in control.** Nothing a bot does that needs approval runs without an answer from a person, and a bot never inherits what its owner allowed for personal use.

## Providers

Claude, ChatGPT (Codex), and Grok, in that order. Each one runs through the Agent Client Protocol, with the user's own sign-in.

## How it earns money

| Offer | Price |
| --- | --- |
| Personal use, on your own computer, all features | Free |
| Hosted server: we run the daemon for you or your team | Paid, monthly |
| License for use at work | Paid, per member |

The desktop client is free software. The daemon is free for noncommercial use; a paid license covers use at work.

## What it does not do

- It does not provide models or plans to reach them.
- It does not send conversations, files, or commands to a Crow Bot service. A hosted server is a daemon that belongs to its customer.

## Reference products

- **Grok Bot** (`https://x.ai/bot`) is the product model: AI teammates, each with its own computer.
- **OpenBot** (`C:\dev\openbot`) is the closest product. It is the reference for the desktop experience, the team features, and the hosted servers.
- **Hermes Agent** (`C:\dev\hermes-agent`) is the reference for memory, skills, the learning loop, scheduled work, and messaging gateways.
- **Zed** (`C:\dev\zed`) is the reference for GPUI and for a native agent interface.

## Milestones

1. **Desktop core.** One person, several bots, Claude, conversations that last, approvals, a complete desktop interface.
2. **More providers.** ChatGPT and Grok.
3. **The bot's computer.** An isolated browser and workspace for each bot, visible live in the client.
4. **Memory and skills.** What a bot learns, how it reuses it, and how it improves.
5. **Scheduled work and channels.** Routines, and bots reachable from messaging apps.
6. **Servers and teams.** A daemon on a server, several members, and the hosted offer.
