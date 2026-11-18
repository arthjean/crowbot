# Provider sign-in: how the daemon reads the state and starts a sign-in

> Status: finding of US-025, amended by US-030 and US-032 on 2026-11-15 (see "What changed since"), and by EP-005 of the Linux port on 2026-11-18 (see "Linux runs") | Observed on 2026-11-14, Windows 11, Bun 1.4.4

This note answers one question: how can `crowbotd` tell whether a provider is ready, signed out, or missing its runtime, and how can it start the provider's own sign-in without touching a credential. Every line marked "observed" comes from a run on the author's computer. What was not run is listed at the end.

## Terms

- **Adapter**: the program that speaks the Agent Client Protocol (ACP) for a provider. For Claude it is `claude-agent-acp`, from the standalone archive the daemon installs (`crates/crowbotd/src/runtime/pins.rs`). The runs of 2026-11-14 below started it through `bunx`, the launcher of that time.
- **Runtime**: the files the daemon downloads, checks, and starts for a provider, at a pinned version: for Claude the adapter and the `claude` program it holds, with no Bun or Node.js on the computer. On 2026-11-14 it was Bun (the `bunx` launcher) and the adapter package.
- **Auth method**: an entry of `authMethods` in the answer to the ACP `initialize` request. A method of type `terminal` is a command the client runs; a method of type `agent` (the default) is handled by the agent through the ACP `authenticate` request.

## Claude

Versions: adapter `@agentclientprotocol/claude-agent-acp@0.107.0` (the version pinned in `crates/crowbotd/src/provider/claude.rs`), Claude Code 2.1.325.

### How the daemon learns the state

One short ACP connection, with no session, answers all three states.

| State | What the daemon does | What it gets (observed) |
|---|---|---|
| Runtime missing | Started `bunx @agentclientprotocol/claude-agent-acp@0.107.0` (the launcher of that time; see "What changed since") | The process cannot start. A daemon run with Bun taken out of `PATH` fails the turn of a Claude bot with `Internal error: "program not found"`. |
| Signed in | Sends `initialize` with `clientCapabilities.auth.terminal = true`, then the request `auth/status` with empty parameters | `{"state":"authenticated","accounts":[{"label":"Claude Ultra","active":true,"email":…,"organization":…}]}`, 2.6 s after the process started |
| Signed out | The same two requests | `{"state":"unauthenticated"}`, 1.9 s after the process started |

Details that matter for the implementation:

- The answer to `initialize` advertises the method: `agentCapabilities.auth` is `{"logout":{},"status":{}}`. The daemon sends `auth/status` only when `auth.status` is present.
- `auth/status` is not in the stable ACP schema. In the Rust SDK it sits behind the Cargo feature `unstable_auth_status` of `agent-client-protocol` 5.2.0 (types `AuthStatusRequest`, `AuthStatusResponse`, `AuthState`).
- `AuthState` has four values: `authenticated`, `unauthenticated`, `not_required`, `unknown`. The first and the third mean ready. `unknown` means the adapter could not read its own state (its status probe has a 5 second limit).
- The adapter also pushes the notification `auth/status_update` (and its older twin `_auth/status_update`) on the same connection when the state changes. Both were observed right before the answer to `auth/status`.
- The signed-out state was produced without signing out: the adapter was started with `CLAUDE_CONFIG_DIR` set to an empty folder.
- The account entry carries an email and an organization name. They are display data, not credentials, but the daemon keeps only the label (for example "Claude Ultra"): it does not store or log the rest.

### How the daemon starts the sign-in

The answer to `initialize` lists two auth methods of type `terminal` (observed):

| Id | Name | Arguments |
|---|---|---|
| `claude-ai-login` | Claude Subscription | `--cli auth login --claudeai` |
| `console-login` | Anthropic Console | `--cli auth login --console` |

A `terminal` method means: run the agent's own command with these arguments appended, and read the exit status (zero is success). The adapter lists these methods only when the client sends `auth.terminal = true`.

Observed run of `bunx @agentclientprotocol/claude-agent-acp@0.107.0 --cli auth login --claudeai`, with no terminal attached (standard input closed, output piped) and an empty `CLAUDE_CONFIG_DIR`:

1. After 0.6 s the process prints "Opening browser to sign in…" and a link to open by hand.
2. The default browser opens the provider's authorization page.
3. The process waits for the browser to finish.

So the daemon can run the sign-in as a hidden child process. The credential goes from the provider's page to the provider's CLI, which writes it to its own store. The daemon never reads it, and it does not record the output of the process (the link holds a one-time challenge).

What the user sees: their browser opens the provider's page. In Crow Bot, the provider shows "Signing in…" with a Cancel control. When the process exits, the daemon asks for the state again and tells every client.

### Fallback when the adapter cannot report the state

`<agent command> --cli auth status --json` runs the Claude CLI that the adapter bundles (observed):

| State | Exit status | Output |
|---|---|---|
| Signed in | 0 | JSON with `"loggedIn": true` |
| Signed out | 1 | JSON with `"loggedIn": false` |

The `claude` binary on `PATH` answers the same command the same way (observed).

Limits of the fallback:

- It needs the runtime too: when the runtime does not start, it fails like the adapter.
- It reads the credential store only. It cannot tell an expired or revoked sign-in from a valid one; neither can `auth/status`, whose `authenticated` means "credentials are configured".
- Its output holds the email and the organization: only the exit status and `loggedIn` may be read.
- A `claude` found on `PATH` may be another install than the one the adapter runs. The form with `--cli` avoids this.

## Codex

Observed with `bunx @zed-industries/codex-acp` (0.16.0) and `codex-cli` 0.169.0.

- `initialize` answers with `agentCapabilities.auth = {"logout":{}}`: no `status`. The request `auth/status` fails with `-32601 Method not found`.
- Auth methods: `chatgpt` (type `agent`: the agent runs the sign-in when it receives `authenticate`), and two of type `env_var` (`CODEX_API_KEY`, `OPENAI_API_KEY`).
- Fallback, observed: `codex login status` prints "Logged in using ChatGPT" and exits 0; with an empty `CODEX_HOME` it prints "Not logged in" and exits 1.
- npm marks `@zed-industries/codex-acp` as replaced by `@agentclientprotocol/codex-acp`.

What differs from Claude: the state comes from the CLI's status command, not from ACP, and the sign-in is an `authenticate` request on an open connection, not a command to run.

## Grok

Observed with `grok agent stdio` (Grok Build 1.0.46).

- `initialize` answers with `agentCapabilities.auth = {}`: no `status`. The request `auth/status` fails with `-32601 Method not found`.
- Auth method: `grok.com` (type `agent`).
- The CLI has `grok login` (with `--oauth` and `--device-auth`) and `grok logout`. Its help lists no status command.

What differs from Claude: no ACP status and no status command were found, so the state can only be learned from a failed request (`auth_required`, code `-32000`) or from a future version. The sign-in is an `authenticate` request or `grok login`.

## The method the daemon uses

For each provider profile, in this order:

1. Find the launcher program on `PATH`. Not found: **runtime missing**.
2. Start the agent and send `initialize` with `auth.terminal = true`. The process ends before it answers: **runtime missing**, with the reason.
3. `auth.status` advertised: send `auth/status`. `authenticated` or `not_required`: **ready**. `unauthenticated`: **signed out**. Anything else: **unknown**.
4. No `auth.status`: **unknown**. The status command described above is the fallback for a profile whose adapter does not report: it comes with the Codex profile (US-030), which needs it. The Claude adapter reports, so the Claude profile does not run it.
5. Sign-in: the first `terminal` method runs as a hidden child process of the daemon, in a job object on Windows (a process group on Unix) so that a cancel also ends the processes it started; a sign-in that ends by itself keeps what it started (the browser it opened is the user's); the daemon reads the state again when it exits. On Windows it starts outside the daemon's job (`CREATE_BREAKAWAY_FROM_JOB`, `lifetime.rs`), so that the end of the daemon does not end that browser either. A sign-in of type `agent` runs inside the agent, which is in the daemon's job: a browser that such a sign-in starts, when none was running, ends with the daemon. A method of type `agent` goes through `authenticate` (needed for Codex and Grok, US-030).

`unknown` never blocks a bot: the turn is the final check, and a turn that fails for a missing sign-in already carries `FailureCause::SignInRequired`.

## What changed since

US-030 and US-032 changed the method above. The observations above stay as they were made.

- **No launcher on `PATH`.** The daemon no longer starts an adapter through `bunx`. It runs the runtime it installed (`crates/crowbotd/src/runtime.rs`), or the executable the user set for the provider. A runtime that is not installed gives the state **not installed**, with a control that downloads it; **runtime missing** is left for a program that does not start or a platform with no pinned runtime.
- **Codex and Grok run in a home of the daemon** (`<data>/providers/<id>/home`, set through `CODEX_HOME` and `GROK_HOME`). The user's own sign-in to those tools is not seen: the user signs in once from Crow Bot, through the `authenticate` request. The daemon stores one flag per provider (`provider_settings.signed_in`), never a credential.
- **Grok opens no browser over ACP** (observed with Grok 1.0.77 on 2026-11-15). During `authenticate`, it picks a transport (`device` by default, from its remote config; `loopback` when the request carries `_meta.use_oauth`) and waits. It names the page where the person signs in only when the client asks `_x.ai/auth/get_url`, which answers `{"auth_url", "mode"}`; in `device` mode the address already holds the code to confirm. The Grok profile names that request (`sign_in_page`): while the sign-in runs, the daemon asks it, puts the address in the provider's status (`sign_in_url`, never stored), and the window that started the sign-in opens it. Every window shows "Open the page" for it.
- **How the state is read**, in this order (`probe` in `provider/acp.rs`): the request `auth/status` when the agent advertises it (Claude); else the pushed status update when the agent announces `authStatus` in the `_meta` of its capabilities (Codex); else, for an agent that wants its sign-in named before each session (Grok, method `grok.com`), **signed out** until a sign-in started from Crow Bot ended well, then one `authenticate` request with that method, which the agent accepts or refuses.
- **Several sign-in methods** are listed to the client by name, and the client starts the one picked. The Codex method `api-key` is not listed: it would need the daemon to hold a key.
- **On Linux the clients open every sign-in page** (EP-005, `crates/crowbotd/src/sign_in_page.rs`). Observed: under GNOME, `xdg-open` starts the browser in the process group of its caller, so a cancel, which kills the group of the sign-in, killed a browser the sign-in had started. Claude runs `$BROWSER`, else `xdg-open`, in that group; `codex-acp` runs `xdg-open` in a session of its own (the npm package `open` with `detached`); a daemon with no display variable could open nothing, and the address went to an output nobody reads. So while a sign-in runs, its `BROWSER` and the first `xdg-open` on its `PATH` are the daemon's own program, which opens nothing: it passes the address, an `http` or `https` page of at most 8 KB on one line, to the daemon over an abstract socket of that sign-in that only the daemon's account is heard on. The daemon puts it in `sign_in_url`, as for Grok, and the window that started the sign-in opens it in its own session. Windows and macOS keep the browser the agent opens.

## Linux runs (EP-005)

Run on 2026-11-18 on Fedora Linux 45 Workstation, GNOME Shell 51.1 on Wayland, kernel 7.3.4, bubblewrap 0.13.1, Helium as the default browser. Runtimes installed by the daemon: `claude-agent-acp` 0.107.0 with Claude Code 2.1.323, `codex-acp` 2.6.0 on Bun 1.4.4 with `codex-cli` 0.166.0, and Grok 1.0.77.

- **Runtimes.** Every program the three runtimes unpack has its execute bit, Bun from its zip archive included (`claude`, `claude-agent-acp`, `bun`, `codex`, `rg`, `bwrap`, `grok`). Every pinned address of every platform answers with the pinned SHA-256 (the ignored test `every_pinned_address_serves_its_pinned_file`, 24 artifacts in 44 s).
- **Sign-in from the client, daemon under the systemd unit.** The unit's daemon has `DISPLAY`, `WAYLAND_DISPLAY`, and `XDG_CURRENT_DESKTOP` on GNOME. ChatGPT and Grok were signed in from the client: each page opened in the running browser, the person finished it, and the provider showed as ready. A ChatGPT sign-in that was cancelled, then started again, opened a second page; finishing the first page then still signed in, which matches an authorization that the provider keeps per browser, since no process of the cancelled sign-in was left. Grok gives each sign-in its own device code: only the page of the last sign-in finishes.
- **Claude** was signed in from a second client, on a daemon of its own with an empty `CLAUDE_CONFIG_DIR`, so that the author's own `~/.claude` stayed as it was (the profile still shares it, an open decision of `AGENTS.md`). With the browser closed, the sign-in opened it; a cancel left it open, in the process group of the client and not of the sign-in; a second sign-in finished, and Claude showed as ready.
- **No display.** On a daemon started with no `DISPLAY` and no `WAYLAND_DISPLAY`, the sign-ins of Claude, ChatGPT, and Grok each put their address (`claude.com/cai/oauth/authorize`, `auth.openai.com/oauth/authorize`, `accounts.x.ai/oauth2/device`) in the provider's status, and a cancel ended each one.
- **Files.** After the sign-ins, `auth.json` of Codex and of Grok is 0600 under `<data>/providers/<id>/home`, in folders that are 0700; the files the agents write beside it with the user's umask stay in those folders. The daemon reads no file of those homes: it writes `config.toml` and removes the listed layers.
- **Turns** (US-021), each on a bot that asks first, unless noted. A write declined leaves no file and records the declined request; a write allowed once is written, and the next write of the same turn asks again; a bot on Full access is allowed once by the daemon, which records `permission_level`. All three held for Claude, ChatGPT, and Grok. Codex ran a shell command under `bwrap --as-pid-1 --new-session --die-with-parent --ro-bind / / …` and `codex-linux-sandbox`, from the system's `bwrap`. With the author's own `~/.codex` at `approval_policy = "never"` and `sandbox_mode = "danger-full-access"`, a Claude config whose `settings.json` allows every tool in `bypassPermissions`, and a home whose `.grok/config.toml` sets `yolo = true`, every write still asked.
- **No user namespaces.** In a user namespace whose limit of new user namespaces is 0 and with no capability, as on a computer that disables them, `bwrap` fails with "Creating new namespace failed: Operation not permitted". The daemon starts the `bwrap` Codex picks once before each Codex session, and the turn fails with "ChatGPT's sandbox needs user namespaces, which this computer disables (…)" (`codex::check_sandbox`, test `a_codex_turn_on_a_computer_without_user_namespaces_names_the_cause`).

## Not run

- A Claude sign-in carried to its end on Windows. It was carried to its end on Linux (see "Linux runs").
- A Claude sign-in into the author's own `~/.claude` from Crow Bot: it would replace the credential the author's other Claude sessions use.
- The sign-in on a computer with no browser at all, such as a server. On Linux the client opens the page; a page whose last step redirects to `localhost` must then be on the computer of the daemon. It belongs to the servers PRD.
- A sign-in stopped by its timeout of 10 minutes. It ends the same way as a cancel (`wait_for_sign_in`).
- KDE: whether the unit's daemon gets the display variables there. The page no longer depends on them, since the client opens it.
- The push of `auth/status_update` at the start of each prompt, described in the adapter's source (`dist/auth-status.js`).
- A state where the adapter answers `unknown`.
- Codex and Grok on Windows: a sign-in carried to its end in the home of the daemon, and a real turn after it. Both were run on Linux (see "Linux runs").
- macOS.
