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

On Linux the client opens its window on Wayland, or on X11 when `WAYLAND_DISPLAY` is empty. On Fedora:

- To build: `sudo dnf install gcc fontconfig-devel libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel`.
- To run: `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, `libwayland-client`, `libwayland-egl`, `vulkan-loader`, `libglvnd-egl`, a graphics driver (`mesa-vulkan-drivers`, or the vendor's), and `xdg-desktop-portal` with the backend of the desktop. Fedora Workstation installs all of them.

To try a change apart from your own data, `scripts/sandbox.ps1` (Windows) or `zsh scripts/sandbox.zsh` (Linux, macOS) builds both and opens the client on a throwaway daemon with its own database, runtimes, and provider sign-ins. `stop`, `reset`, and `clean` end it, wipe it, or remove it.

To find Crow Bot in the application grid of a Linux desktop, `sh scripts/desktop-files.sh` builds both and copies the desktop entry and the icons of `crates/crowbot-desktop/resources/linux` under `${XDG_DATA_HOME:-~/.local/share}`, with the entry pointing at the built client. It also copies the systemd user unit `crates/crowbotd/resources/linux/systemd/crowbotd.service` under `${XDG_CONFIG_HOME:-~/.config}/systemd/user`, pointing at the daemon built next to the client, and enables it, so the daemon starts at login. `install --release` uses a release build, `install --client <path>` a client already built (the daemon must sit next to it), and `remove` stops and disables the unit, then deletes every file the install copied. The window carries the application ID `io.github.arthjean.CrowBot`, so the desktop ties it to that entry and its icon.

Under the unit, the daemon belongs to systemd rather than to the program that launched the client: it keeps running when that program ends, and systemd starts it again within a second after a crash. It still stops when you log out, unless lingering is on: `loginctl enable-linger` keeps your user services running with no session, and Settings > About says so when it is off. Crow Bot never turns it on itself.

The daemon:

- `--db <path>` selects the database. The default is `~/.local/share/crowbot/crowbot.db` on Linux (`$XDG_DATA_HOME/crowbot`), and `~/.crowbot/crowbot.db` elsewhere. Its folder, when the daemon creates it, and its files are readable by your account only; a database folder of another account is refused.
- `--bots-dir <dir>` is where a bot created with no folder gets one of its own. The default is `~/CrowBot/Bots`.
- `--data-dir <dir>` holds the agent runtimes the daemon installs and the files providers keep for bots. The default is the folder of the database.
- `--endpoint <address>` selects where it listens. The default is the named pipe `\\.\pipe\crowbotd-<user>` on Windows, the socket `$XDG_RUNTIME_DIR/crowbot/crowbotd.sock` on Linux (in `run` under the data folder, with a warning, when `XDG_RUNTIME_DIR` is not set), and `~/.crowbot/crowbotd.sock` elsewhere. Only the user who runs the daemon can attach to it. A socket path longer than 107 bytes (103 on macOS) is refused before anything is created, with exit status 5.
- SIGTERM, SIGINT, or SIGHUP stops it: the running turns end as stopped within 5 seconds, their waiting approvals are cancelled, the clients are told, and the socket is removed. A second signal stops it at once.
- `--agent "<command>"` runs another ACP agent, which every bot then runs in place of the three providers.
- `--runtime-manifest <file>` replaces the pinned runtimes with the ones a JSON file lists, for a mirror.

The client:

- It opens the most recent conversation. Closing the window does not stop a turn: the daemon keeps working, and an approval request waits until a client answers it.
- Enter sends the message, Shift+Enter adds a line. A message sent while the bot works waits in a queue and runs next. Stop ends the running turn.
- `--endpoint <address>` attaches to a daemon that listens somewhere else.
- When no daemon is running, the client starts `crowbotd` from the folder of its own executable, or from `--daemon <path>`. On Linux, when the systemd unit is installed and none of `--endpoint`, `--daemon`, or daemon arguments is given, it asks your systemd to start the unit instead. The daemon keeps running after the window closes. A daemon the client started itself runs in a session of its own and logs to `~/.local/state/crowbot/crowbotd.log` on Linux (`$XDG_STATE_HOME/crowbot`) and `~/.crowbot/crowbotd.log` elsewhere, or beside the database that `--db` names; under the unit it logs to the journal (`journalctl --user -u crowbotd`). Arguments after `--` go to the daemon it starts.
- When the daemon cannot start, the window states the reason and offers to retry. When the daemon crashes, the client starts it again; when it was told to stop, the window says so and waits for you to retry.

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
