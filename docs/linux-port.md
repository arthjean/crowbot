# Linux port: state on Fedora and the work to do

> Status: research, 2026-11-16. Nothing here is implemented. Measured on one computer: Fedora 45 Workstation, x86_64, GNOME 51 on Wayland, glibc 2.44, rustc 1.100, an NVIDIA GPU with the proprietary driver. Sources are pinned to a tag or a commit.

This note answers one question: what must be done for Crow Bot, which works on Windows, to be a first-class application on Fedora Linux, and then on the other distributions? It has three parts: the state measured today, the work to do, and the research the work rests on.

## Terms

- **Compositor**: the Wayland display server. On GNOME it is Mutter, on KDE it is KWin.
- **Client-side decorations (CSD)**: a title bar, window buttons, border, shadow, and resize edges that the application draws itself.
- **Portal**: `xdg-desktop-portal`, a D-Bus service that gives an application the file dialogs, the light or dark preference, and other desktop services.
- **glibc and musl**: the two common C libraries of Linux. A **static** binary carries its C library inside and runs on any distribution.
- **systemd user unit**: a service that the per-user instance of systemd starts and supervises.
- **Lingering**: a setting of `loginctl` that keeps the user's systemd instance running when the user has no session.

## The state today

Everything in this table was run on the computer above.

| Check | Result |
| --- | --- |
| Root workspace: build, clippy, rustfmt | Clean, no warning |
| Root workspace: tests | 260 passed, 5 ignored, in every run at normal load |
| Ignored tests that spend no model usage | Pass: the search over 100,000 events, and the install of the pinned Claude runtime, which downloads, unpacks, starts, and reports "Ready" |
| Ignored tests that run a real turn (2 on Claude, 1 on Grok) | Not run: they spend the user's subscription or need a Grok executable |
| Client: build, clippy, rustfmt | Clean, no warning |
| Client: tests | 225 passed, 1 ignored (it needs `OPENBOT_DIR`). They run on GPUI's test platform, so they prove nothing about a real window |
| Client: the binary the repository builds | Has no display backend. It links only libc, libm, and libgcc, and cannot show a window |
| Client: a build with the `wayland` and `x11` features, made outside the repository | Builds, passes the same tests, starts on Wayland with Vulkan, attaches to a daemon |
| Daemon by hand | Starts, listens on a socket of mode 0600, a second daemon on the database exits 3, a held endpoint exits 4 |
| SELinux (enforcing) | No denial in the journals. The daemon and the runtimes run as `unconfined_t` |
| The 9 pinned Linux archives (3 providers, x86_64 and aarch64) | All answer and match their SHA-256 |

The visual pass was not done. The portal that takes screenshots on Wayland holds a stored refusal for the terminal the session ran in, GPUI exposed no accessibility tree to `agent-desktop`, and Wayland refuses synthetic pointer input. (Found later, in US-026: GPUI publishes its tree to AT-SPI only once `org.a11y.Status.IsEnabled` is true, which Orca sets when it starts; that session had it false. `scripts/accessibility-check.sh` checks the tree with it on.) What follows about the look of the window comes from the GPUI source and from the X11 properties of the test window, not from a picture.

## The work to do

Each item names its evidence. The order inside a group is the order of dependency. Groups A and B make the client usable, C makes the daemon correct, D keeps it that way, E ships it.

### A. The client must show a usable window

1. **Enable the display backends.** `crates/crowbot-desktop/Cargo.toml:28-29` takes `gpui_platform` with no feature. At the pinned rev the `wayland` and `x11` features are not defaults (`gpui_platform/Cargo.toml` lines 26-27) and Zed's workspace takes `gpui_linux` with no default feature, so the lockfile holds no `wayland-client`, `x11rb`, or `xkbcommon`. Enable both features for Linux targets and regenerate `Cargo.lock` (44 packages are added; take the versions of Zed's lockfile at that rev). This single change is what stands between today's binary and a window.
2. **Draw the window chrome on Linux.** Mutter does not offer the `xdg-decoration` protocol, GPUI draws nothing itself, and the application asks for nothing (`main.rs:115-126` leaves `window_decorations` unset), so on GNOME the window is a bare rectangle. What the client must add:
   - Request `WindowDecorations::Client`, and read `window.window_decorations()` at each frame: it tells whether the compositor decorates after all, and which edges are tiled.
   - Move the window. `WindowControlArea::Drag` does nothing on Linux: `on_hit_test_window_control` drops its callback on Wayland (`wayland/window.rs:2039`) and on X11 (`x11/window.rs:1772`). The title band (`header.rs:40`, `rail.rs:113,183`, `bot_list.rs:39`, `settings.rs:1735,1775`, `screens.rs:358,372`, `creation.rs:1488`) must call `start_window_move` on a press, zoom on a double click, and call `show_window_menu` on a right click.
   - Window buttons. `workspace/header.rs:245` returns nothing outside Windows, and the glyphs come from Segoe fonts that Linux does not have. Draw the buttons as SVG icons, show only the ones `window_controls()` allows, and follow the button layout of the desktop (GNOME shows close alone by default).
   - Resize edges, shadow, border, and rounded corners: `start_window_resize(edge)`, `set_client_inset` for the shadow margin, `set_client_corner_radii`, and no rounding or shadow on a tiled edge or a maximized window.
   - Zed's implementation is the reference to read: `crates/workspace/src/workspace.rs` lines 12656-12919, `crates/platform_title_bar/`, and `crates/gpui/examples/window_shadow.rs`.
3. **Handle a compositor that decorates.** Under Xwayland today, Mutter adds its own 37 px title bar above the 38 px band of the application (`_NET_FRAME_EXTENTS` of the test window): two title bars. When `window_decorations()` answers `Server`, the client must hide its own buttons, or always ask for client decorations. On X11, GPUI uses client decorations only with a compositing manager that supports `_GTK_FRAME_EXTENTS`.
4. **Write the Linux chrome into `DESIGN.md`**, in the same commit as the code (the design contract rule). `DESIGN.md:92` covers Windows and macOS only. The window corner radius must enter the concentric radii rule, and the rule that no fill reaches a rounded corner now applies to the window itself.
5. **Fail in words.** GPUI panics when the display server cannot be reached, and opening a window fails when no GPU adapter works. `main.rs:129-132` quits with no message. Print the reason and exit with a failure status. Decide whether software rendering deserves a notice (`gpu_specs().is_software_emulated`); a virtual machine with `mesa-vulkan-drivers` renders on the CPU.
6. **Check the fonts.** Inter is embedded and registered, so the main text is safe. The fallbacks in `theme.rs:290` name `.SystemUIFont`, which is "IBM Plex Sans" on Linux and absent from Fedora, and `Segoe UI`. Name Linux families (Adwaita Sans, Noto Sans, Cantarell, DejaVu Sans), and check that the `monospace` alias of `theme.rs:295` resolves, and that Arabic, CJK, and emoji draw.
7. **Do the visual pass** once items 1 to 3 exist: light and dark, fractional scaling at 125% and 150%, two monitors, the composer with an input method and an AZERTY layout, the clipboard, the sign-in page opened through `cx.open_url`, reduced motion, KDE on Wayland, and an X11 window under Xwayland. Done in US-007: see [Visual pass on real desktops](#visual-pass-on-real-desktops-us-007).

### B. Identity on the desktop

8. **Choose an application ID and use it everywhere.** The test window has no `WM_CLASS`, no icon, and no application ID, so no desktop can tie it to an application. The ID is a reverse-DNS name on a domain the project controls; it names the `.desktop` file, the icons, the metainfo file, and a Flatpak if one comes. Set `WindowOptions.app_id`: GNOME matches it to a file named `<app_id>.desktop` on Wayland and on X11.
9. **Ship the desktop files.** None exists: a desktop entry, icons in the `hicolor` theme (a scalable SVG, and PNG sizes), and an AppStream metainfo file for the software centers. On Wayland the window icon comes only from the desktop entry.
10. **Decide on a single instance.** Two clients on one daemon work (verified), but a second launch opens a second window. GPUI has no helper for this on Linux.

### C. The daemon

11. **Stop on a signal.** Nothing handles a signal: tokio is built without `signal` (`crates/crowbotd/Cargo.toml:23`). SIGTERM, SIGINT, and SIGHUP kill the daemon at once and leave the socket and the SQLite write-ahead files (verified). A stop from systemd, a logout, and Ctrl+C all send one. The daemon must end its turns, close the sessions, and remove the socket.
12. **Decide who supervises the daemon.** The client starts it with `process_group(0)` and no new session (`connection.rs:128-133`). Measured: the daemon stays in the systemd scope of the application that launched it, so it ends when the user logs out unless lingering is on, and `KillUserProcesses` does not change that. The product promises a bot that keeps working. The Linux answer is a systemd user unit (`Type=notify`, restart on failure), started by the client or at login, with lingering for a server. Two points to settle: socket activation would let systemd hold the endpoint before the daemon is ready, which contradicts the start order of `docs/daemon-startup.md` (an endpoint that answers belongs to a ready daemon); and a computer without systemd still needs the detached start, with `setsid`.
13. **Choose the folders.** Everything sits under `~/.crowbot` and `~/CrowBot/Bots`, from `HOME` alone (`main.rs:87-98`, `endpoint.rs:27-36`); no `XDG_*` variable is read. The XDG fit: the database, the runtimes, and the provider homes under `$XDG_DATA_HOME`, the log under `$XDG_STATE_HOME`, the socket and its lock under `$XDG_RUNTIME_DIR`. Linux has no users yet, so no migration is owed.
14. **Check the length of the socket path before anything else.** A Unix socket path holds 108 bytes. At 108 bytes the daemon exits 1 with `path must be shorter than SUN_LEN`, after it created the database. A home path over about 84 characters breaks the default socket. A socket under `/run/user/<uid>/` is short, which is one more reason for item 13.
15. **Make the files private.** With an explicit `--db`, the folder is created 0755 and the database, its lock, and its owner file 0644 (`instance.rs:69`): another account can read the conversations. Only the default `~/.crowbot` is forced to 0700. The client writes the log 0644.
16. **Keep the log with its daemon.** The client sends the daemon's errors to `~/.crowbot/crowbotd.log` whatever `--endpoint` and `--db` say (`connection.rs:370`). A sandbox started with `scripts/sandbox.zsh` creates `~/.crowbot` in the real home (verified).
17. **Close the gap in the agent guard.** The guard reads `getppid()` after it starts (`lifetime.rs:127`), then polls every 200 ms. A daemon that dies before that first read is never noticed, and an agent that calls `setsid` leaves the process group. A pipe that the daemon holds open tells the guard at once, with no poll; a systemd scope per agent would also catch the processes that leave the group. `PR_SET_PDEATHSIG` is a poor fit: it follows the thread that started the process, not the daemon.
18. **Run each sign-in on Linux.** `docs/provider-sign-in.md:125` lists Linux as not run. Points to check: a cancel or a timeout sends SIGKILL to the process group of the sign-in (`daemon/providers.rs:871-873,935-941`), which may take a browser that `xdg-open` just started; a daemon started as a systemd unit gets the display variables on GNOME, and other desktops are not verified; a server has no browser. Codex and Grok have a device-code flow. Claude's documented route for a computer with no browser makes the caller hold a token, which the hard rules forbid: this is a decision.
19. **Run a real turn of each provider on Linux**, on the runtime the daemon installs. Codex isolates its commands with bubblewrap and needs user namespaces: they work on Fedora Workstation, and are often blocked in a container.
20. **Find the C library at run time.** `platform()` in `runtime/pins.rs:28-38` gives a Linux platform only when the daemon itself was built for glibc. A static musl daemon, the best candidate for every distribution, would find no runtime, although Codex and Grok are static and Claude and Bun publish musl archives. Add the musl pins, and choose by the computer, not by the build.
21. **Small corrections.** `program_exists` accepts a file with no execute bit (`provider/acp.rs:158`). There is no `--version`, and `--help` writes to the error stream with status 2 (`main.rs:80`). A home mounted `noexec` would stop every runtime with no clear message. A pin bump of `claude-agent-acp` must check that the release has archives: v0.107.3 has none.

### D. Tests and continuous integration

22. **Fix the test that fails under load.** `providers::a_grok_sign_in_names_its_page_to_the_clients_until_it_ends` failed 8 times in 240 parallel runs. The `tokio::select!` of `provider/acp.rs:481` has no order, and the fake agent ends the sign-in 10 ms after the page is asked.
23. **Free the tests from the environment.** 109 tests fail when `TMPDIR` is longer than about 67 characters (the socket path again). One fails as root (`tests/daemon/runtimes.rs:392-407` relies on `chmod 0555`), which is how a container job runs. Each run leaves folders, locks, and sockets in `/tmp` (`runtime.rs:606`, `tests/daemon/startup.rs:113`, `connection.rs:724-730`).
24. **Test the Unix transport.** `transport/windows.rs` has 7 tests; `transport/unix.rs` has none. Nothing proves that a peer of another account is dropped or that the socket is 0600.
25. **Tidy the build.** `.gitignore` holds `/target`, which does not cover `crates/crowbot-desktop/target/`. The client tests run Cargo on the root workspace without `--locked` (`connection.rs:471-491`).
26. **Add continuous integration.** There is none. GitHub has no Fedora image: a Fedora job is a container job on an Ubuntu runner. Run the daemon and client tests there, on x86_64 and on arm64, and add one start of the real client under a headless compositor with CPU rendering, the only test that would have caught item 1.
27. **Document the Linux build.** The packages a build needs on Fedora: `gcc`, `libxcb-devel`, `libxkbcommon-devel`, `libxkbcommon-x11-devel`, `fontconfig-devel`, `freetype-devel`. At run time: `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, `libwayland-client`, `libwayland-egl`, `vulkan-loader`, a Mesa or vendor driver, and `xdg-desktop-portal`.

### E. Distribution

28. **Set the glibc floor.** A binary needs the glibc of the computer that built it: built here, `crowbotd` needs 2.39 and the windowed `crowbot` 2.44, so neither starts on Ubuntu 22.04 or Debian 12. Build the daemon as a static musl binary (it bundles SQLite and uses rustls, with no OpenSSL; not yet tried). Build the client against an old glibc, as Zed does (2.31 on x86_64, 2.35 on aarch64).
29. **Fedora: a signed dnf repository of our own, and a plain RPM.** The official repositories and COPR are closed twice over: the daemon's license is not a free license, and Fedora refuses software that downloads code to be useful. The RPM carries both binaries side by side (the client looks for `crowbotd` next to itself), the desktop files, the systemd user unit, and the three license texts.
30. **Every distribution: an archive and an install script into the home**, the way Zed ships. It needs no root, works on Fedora Atomic where `/usr` is read-only, and can update itself. Then a `.deb` with an apt repository.
31. **Flatpak later, if at all.** The sandbox fights a daemon that starts downloaded programs, Flathub builds with no network, and its policy forbids manifests written with the help of an AI.
32. **aarch64.** The pins exist and match. Nothing was built or run on that architecture.

## Decisions to take

| Decision | Options | Recommendation |
| --- | --- | --- |
| Application ID | A name on a domain the project controls, or a code-hosting name that needs no domain | Decided 2026-11-16: no domain before the product works on its target systems. Use `io.github.arthjean.CrowBot` until then, held in one constant. Nothing is released, so a later change costs one line |
| Decorations | Always client-side, or accept the compositor's | Always client-side: one look on GNOME and KDE, and the band already exists |
| Daemon supervision | Detached start by the client, or a systemd user unit | The unit, with the detached start as the fallback |
| Folders | `~/.crowbot`, or the XDG folders | XDG on Linux, since nobody has data to migrate |
| Daemon binary | glibc, or static musl | Static musl, with the run-time choice of item 20 |
| Sign-in on a server | Device codes, or wait for the remote transport | Open: Claude has no route that keeps the rule on credentials |
| Distribution (group E) | Now, or after the product works | Decided 2026-11-16: later. No package, release build, or repository is prepared until the product works as its owner intends on its target systems. Group E and item 20 stay here as research for that PRD |
| Scope of "done on Fedora" | GNOME on Wayland only, or also KDE and Xwayland | GNOME and KDE on Wayland; X11 as best effort |

## Visual pass on real desktops (US-007)

Run on 2026-11-18 on the computer above: a real GNOME Shell 51.1 session (Mutter 51.1) on two 2560 by 1440 monitors, with the AZERTY layout `fr+oss`, IBus 1.5.35 with Anthy 1.5.18, Orca 51.1, and xdg-desktop-portal 1.22.1. KDE ran as KWin 6.8.3 nested in that session (`kwin_wayland --wayland-display`), on a D-Bus session of its own with xdg-desktop-portal-kde 6.8.3 and a configuration folder of its own; Sway 1.12 ran nested on a D-Bus session that can start no service; 150% ran on a headless GNOME Shell with a 1920 by 1080 virtual monitor, captured through Mutter's ScreenCast API. The client ran on a throwaway daemon with the scripted agent of the tests (`fake_acp_agent`). Keys were sent as evdev key codes through Mutter's RemoteDesktop API, so the layout itself turned them into characters. The captures are in [`linux-port/us-007/`](linux-port/us-007/).

| Check | Result | Evidence |
| --- | --- | --- |
| GNOME at 100%, 125%, 133%, and 167%, light and dark | Holds. Mutter offers no 150% on a 2560 by 1440 monitor (its scales are 100, 125, 133, 167, 200), so 133% and 167% stand around it. The compositor sends `preferred_scale` 120, 150, 160, and 200, and the window is 1080 by 760 times the scale | `gnome-{light,dark}-{100,125,133,167}.png` |
| GNOME at 150%, light and dark | Holds on the headless shell (`preferred_scale(180)`). Its logical screen, 1280 by 720, is smaller than the window, so GNOME maximizes it | `gnome-headless-{light,dark}-150.png` |
| Two monitors at 167% and 100% | The window moved with Super+Shift+arrow redraws at each scale: on the 167% monitor it gets `preferred_scale(200)` and draws into 1853 by 1320 buffers, back on the 100% one `preferred_scale(120)` and 1112 by 792. Its text is drawn at the scale, not stretched | `gnome-two-monitors-wayland-trace.txt`, `gnome-two-monitors-text-at-167.png` |
| AZERTY shortcuts and characters | Every shortcut of the client acts on its AZERTY key (Ctrl+A, X, V, C, Backspace, K, comma; Shift+Enter, Enter, Escape, Tab). The composer takes `é è ç à ù`, and the dead keys give `ê ô ï`. Ctrl+C in the composer copied nothing: fixed (below) | `gnome-azerty-composer-keys.png`, `gnome-azerty-workspace-keys.png` |
| IBus with Anthy | The marked text shows underlined in place, converts, and commits | `gnome-ibus-anthy-preedit-convert-commit.png` |
| A sign-in page | Grok's sign-in (scripted) opens the page in the default browser (Helium), started through the systemd user manager | `gnome-sign-in-opens-default-browser.png` |
| Reduced motion | Failed: GPUI holds the flag but reads it from no system, so the mascots always moved. Fixed (below); with GNOME's Reduce Animation (`org.freedesktop.appearance reduced-motion`) or Animations off, a working mascot is still to the pixel, and it moves again when the setting goes back | `gnome-reduced-motion-mascot.png` |
| KDE on Wayland and Xwayland on GNOME: one title bar, the buttons act | Holds. KWin accepts client decorations (`zxdg_toplevel_decoration_v1.configure(1)`); Maximize, Restore, Minimize, and Close act under KWin, and under Xwayland Mutter adds no frame (`_MOTIF_WM_HINTS` without decorations, `_GTK_FRAME_EXTENTS` of 16) and each button changes `_NET_WM_STATE` or `WM_STATE` | `kwin-light-one-title-band.png`, `kwin-maximized.png`, `xwayland-gnome-dark-100.png`, `xwayland-gnome-maximized.png` |
| KDE's button layout | Failed: GPUI reads the layout only from the GNOME key the portal serves, which holds KWin's setting only when kde-gtk-config copies it there; this computer does not have it, and the KDE portal serves no KWin setting. Fixed (below): the client follows `kwinrc` on KDE | `kwin-kwinrc-layout-followed.png` |
| Sway with no portal | Starts in the light theme, with no failure, while GNOME is dark | `sway-no-portal-light.png`, `sway-no-portal-bus.txt` |
| Emoji in the transcript | `👍 🐦‍⬛ ❤️ 1️⃣ 👍🏽 🇫🇷` draw in color and a plain `❤` in the text color. The preview of the bot list cut the flag in half: fixed (below) | `gnome-emoji-dark-100.png`, `gnome-emoji-message-zoom.png`, `gnome-preview-split-flag-before-fix.png`, `gnome-preview-cut-between-graphemes.png` |
| Orca | Each window button, each bot of the list, and the composer read with their role and label, and a label that changes (Maximize, Restore) is spoken. The window buttons and the composer were out of the Tab order: fixed (below). Orca ran muted, with an extension that wrote what it would say; libatspi logged no "unknown signature" warning in this session | `gnome-orca-focus-cycle.txt`, `gnome-window-button-focus.png` |
| The identity of the window | With the desktop files installed (`scripts/desktop-files.sh`), the GNOME overview and the dock show Crow Bot's icon | `gnome-overview-identity.png` |

Defects found and fixed in the same story:

- **Reduced motion** (`reduced_motion.rs`). The client sets GPUI's flag from the portal's `reduced-motion` and GNOME's `enable-animations`, and follows their changes; on Windows it reads "Animation effects" (`SPI_GETCLIENTAREAANIMATION`) each time the window becomes active. The Windows path was type-checked for `x86_64-pc-windows-msvc`, not run.
- **The composer left the Tab order once left.** A focus handle tracked with `track_focus` ignores the element's `.tab_stop(true)`; the composer's handle is now a tab stop.
- **The window buttons took no keyboard focus.** The round inside each slot is now the button: a tab stop with the Button role and its label, pressed by Enter or Space, with a `border_focus` edge (`DESIGN.md` 5.13). A click presses it and leaves the focus where it was, as GTK's window buttons do: the first version moved the focus there, so a space typed next pressed the button again (found in the review).
- **Ctrl+C in the composer copied the transcript's selection, or nothing.** GPUI ranks a key bound with no context as deep as the focused element, so the workspace's copy key won over the composer's. The workspace's keys now live in its own context.
- **A cut line could split a grapheme.** GPUI's truncation sums the advance of each character and cuts between two of them. Every one-line truncation of the client now goes through `truncated.rs`, which shapes the line and cuts at the last grapheme boundary that leaves room for the ellipsis. The text clamped to several lines (`line_clamp` in the creation form) still uses GPUI's.
- **KDE's button layout** (`kwin.rs`). On a KDE desktop the client reads `ButtonsOnLeft` and `ButtonsOnRight` of `[org.kde.kdecoration2]` in `kwinrc` (the user's file, then `kdedefaults`, then `XDG_CONFIG_DIRS`), with KWin's defaults, and reads them again when its window becomes active.

## Not verified

- No sign-in and no real turn ran on Linux.
- Nothing ran on aarch64, on musl, in a full Plasma session (KDE ran as a nested KWin, with no Plasma shell), on a computer with no GPU driver, or on macOS.
- No release build was made; sizes and glibc needs come from debug builds.
- The execute bit after a zip unpack (Codex, Bun) was not checked; the tar path (Claude) works.
- 150% was checked on a headless GNOME only: no monitor of the computer offers it. A drag of the window between monitors was done with the keyboard (Super+Shift+arrow): Wayland refuses a synthetic pointer.
- The Windows reading of reduced motion was not run.
- GPUI sends `set_app_id` after `get_toplevel`, so a window preview that GNOME builds before the ID arrives (the overview open when the shell starts) keeps the generic icon until the overview opens again.

The two sections below are the research this list rests on, each claim with its source.

## Research: platform (GPUI, desktop specifications, user daemon, Fedora)

Conventions used below:

- `Z/` stands for `https://github.com/zed-industries/zed/blob/0cfc4a483dfab6ca171df0ceedaaa2a9f7954d0e/`, the rev pinned in `crates/crowbot-desktop/Cargo.toml` (committed 2026-11-14).
- "(probe)" marks a command run on 2026-11-16 on one Fedora 45 Workstation machine (GNOME Shell 51.1, Mutter 51.1, Wayland, systemd 262.1, glibc 2.44). It is evidence for that machine, not a document.
- Unreadable sources: docs.fedoraproject.org and src.fedoraproject.org (bot check), docs.redhat.com (HTTP 403).

### A. GPUI on Linux at the pinned rev

#### A1. System dependencies

- Zed's Fedora list is: `musl-gcc gcc g++ clang lld cmake alsa-lib-devel fontconfig-devel glib2-devel libva-devel wayland-devel libxcb-devel libxkbcommon-x11-devel openssl-devel libzstd-devel vulkan-loader sqlite-devel pipewire xdg-desktop-portal jq git tar` plus four `perl-*` packages. It serves the whole editor. `Z/script/linux#L106-L176`, `Z/docs/src/development/linux.md#L12-L22`
- GPUI alone needs far less. A minimal binary (`gpui_platform` with `wayland` and `x11`) built at the rev links only `libxcb.so.1`, `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`, `libc`, `libm`, `libgcc_s` (probe: `readelf -d`).
- It loads four more libraries at run time (names found in the binary): `libwayland-client.so.0`, `libwayland-egl.so.1`, `libvulkan.so.1`, `libEGL.so.1`. The Wayland library is loaded lazily by design (`dlopen` feature). `Z/crates/gpui_linux/Cargo.toml#L95-L99`
- Fedora run-time packages (probe: `rpm -qf`): `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, `libwayland-client`, `libwayland-egl`, `vulkan-loader`, `libglvnd-egl`, a Mesa driver (`mesa-vulkan-drivers`), and `xdg-desktop-portal` with a desktop backend for dialogs.
- Fedora build packages for GPUI alone: `gcc`, `libxcb-devel`, `libxkbcommon-devel`, `libxkbcommon-x11-devel` (the three linked libraries).
- OpenSSL, ALSA, SQLite, zstd, libva, GLib, PipeWire, and libgit2 do not appear in GPUI's dependency tree (probe: `cargo tree`). They are editor dependencies.

#### A2. Backends and renderer

- `gpui_platform` has the features `wayland` and `x11`. Neither is a default. `Z/crates/gpui_platform/Cargo.toml#L15-L27`
- Zed's workspace takes `gpui_linux` with `default-features = false`, so a third-party app must enable them. Zed does. `Z/Cargo.toml#L376`, `Z/crates/zed/Cargo.toml#L125-L130`
- Crow Bot does not enable them today. `cargo tree -e features -i gpui_linux` in `crates/crowbot-desktop` shows only `gpui_platform/default` (probe). Such a build has no display backend and starts headless: "Windows lay out and handle input but draw nothing". `Z/crates/gpui/src/platform.rs#L311`, `Z/crates/gpui_linux/src/linux/platform.rs#L351-L390`
- Run-time choice: Wayland when `WAYLAND_DISPLAY` is set and not empty, else X11 when `DISPLAY` is set, else headless. `Z/crates/gpui_linux/src/linux/display_connection.rs#L23-L38`
- `WAYLAND_DISPLAY=''` forces X11. `ZED_HEADLESS` forces headless. `Z/docs/src/development/linux.md#L58`, `Z/crates/gpui_linux/src/linux.rs#L40-L46`
- There is no fallback from Wayland to X11. When the selected display server cannot be reached, GPUI panics. `Z/crates/gpui_linux/src/linux/platform.rs#L351-L390`
- The renderer is wgpu (a Rust layer over GPU APIs) with the Vulkan and GL backends. `Z/crates/gpui_wgpu/Cargo.toml#L44-L45`, `Z/crates/gpui_wgpu/src/wgpu_context.rs#L491-L499`
- Adapters (GPU devices) are sorted: `ZED_DEVICE_ID` match, the compositor's GPU, device type (discrete, integrated, other, virtual, CPU), then Vulkan before GL. Each is tested against the real surface until one works. `Z/crates/gpui_wgpu/src/wgpu_context.rs#L523-L657`
- A software adapter (CPU rendering, such as Mesa's lavapipe or llvmpipe) is accepted at start. It is rejected only when the context is rebuilt after a device loss. `Z/crates/gpui_wgpu/src/wgpu_renderer.rs#L2195-L2215`
- With no working adapter, opening the window fails with "No GPU adapter found that can configure the display surface". `Z/crates/gpui_wgpu/src/wgpu_context.rs#L604`
- `Window::gpu_specs()` reports `is_software_emulated`. The "Unsupported GPU" dialog and `ZED_ALLOW_EMULATED_GPU` belong to Zed, not GPUI. `Z/crates/gpui/src/gpui.rs#L344-L352`, `Z/crates/zed/src/zed.rs#L728-L765`
- Documented variables: `ZED_DEVICE_ID`, Mesa's `MESA_VK_DEVICE_SELECT`, `DRI_PRIME`. `Z/docs/src/linux.md#L139-L201`
- Fedora 45's `mesa-vulkan-drivers` contains lavapipe (`lvp_icd.x86_64.json`) (probe: `rpm -ql`). So a virtual machine with that package renders on the CPU instead of failing (inference from the adapter order).

#### A3. Window decorations

- Terms: a compositor is the Wayland display server. Client-side decorations (CSD) are a title bar, border, and shadow drawn by the app.
- `WindowOptions.window_decorations` defaults to `None`, and GPUI then requests `Server`. `Z/crates/gpui/src/platform.rs#L2718-L2720`, `Z/crates/gpui/src/window.rs#L1746-L1747`
- On Wayland GPUI asks through the `xdg-decoration` protocol. When the compositor lacks it, the window stays in client mode. `Z/crates/gpui_linux/src/linux/wayland/window.rs#L290-L296`, `#L626`, `#L2215-L2234`
- Mutter does not offer `xdg-decoration`: absent from `wayland-info` on Mutter 51.1 (probe) and from Mutter's protocol list. https://github.com/GNOME/mutter/blob/51.1/src/meson.build#L1012-L1056
- KWin implements it. https://github.com/KDE/kwin/blob/v6.8.3/src/wayland/xdgdecoration_v1.cpp
- GPUI draws no decoration itself (no libdecor in the tree). On GNOME Wayland an app that does nothing therefore gets a bare rectangle: no title bar, no buttons, no border, no shadow, no resize edges. (Derived from source.)
- API, all in `Z/crates/gpui/src/window.rs`: `window_decorations()` returns `Decorations::Server` or `Decorations::Client { tiling }` (`#L3375`), `window_controls()` (`#L3390`), `start_window_move()` (`#L3354`), `start_window_resize(edge)` (`#L2823`), `show_window_menu(pos)` (`#L3346`), `set_client_inset(px)` for the invisible shadow margin (`#L3359`), `set_client_corner_radii` (`#L3365`), `request_decorations` (`#L2800`). Types: `Z/crates/gpui/src/platform.rs#L841-L908`, `#L1049-L1076`.
- `window_controls()` comes from the compositor's `wm_capabilities`. The button order comes from the portal setting `org.gnome.desktop.wm.preferences button-layout`. `Z/crates/gpui_linux/src/linux/wayland/window.rs#L1318-L1347`, `Z/crates/gpui_linux/src/linux/xdg_desktop_portal.rs#L70`
- Zed's CSD: `client_side_decorations` (shadow, border, rounded corners per tiled edge, resize hit zones) at `Z/crates/workspace/src/workspace.rs#L12656-L12919`; title bar and buttons in `Z/crates/platform_title_bar/src/platform_title_bar.rs` and `platforms/platform_linux.rs`; sizes (10 px rounding, 10 px shadow, 1 px border) at `Z/crates/theme/src/theme.rs#L53-L57`; a small example at `Z/crates/gpui/examples/window_shadow.rs`.
- On X11, CSD is used only with a compositing manager that supports `_GTK_FRAME_EXTENTS`; otherwise the window manager decorates. `Z/crates/gpui_linux/src/linux/x11/client.rs#L419-L422`, `Z/crates/gpui_linux/src/linux/x11/window.rs#L1874-L1880`, `#L1950-L1966`

#### A4. App identity

- `WindowOptions.app_id` becomes `xdg_toplevel.set_app_id` on Wayland and `WM_CLASS` (instance and class both equal to it) on X11. `Z/crates/gpui/src/platform.rs#L2712-L2713`, `Z/crates/gpui_linux/src/linux/wayland/window.rs#L598-L600`, `Z/crates/gpui_linux/src/linux/x11/window.rs#L1629-L1647`
- `WindowOptions.icon` is "X11 only" (`_NET_WM_ICON`). `gpui_linux` has no `xdg-toplevel-icon` code, and Mutter 51.1 does not list that protocol. On Wayland the icon comes from the `.desktop` file. `Z/crates/gpui/src/platform.rs#L2722-L2723`, `Z/crates/gpui_linux/src/linux/x11/window.rs#L808-L828`
- xdg-shell: "it is suggested to select app ID's that match the basename of the application's .desktop file". https://wayland.app/protocols/xdg-shell#xdg_toplevel:request:set_app_id
- Mutter copies the Wayland app ID into the window's WM class. GNOME Shell then matches the WM class to `StartupWMClass`, then to a `.desktop` file name. https://github.com/GNOME/mutter/blob/51.1/src/wayland/meta-wayland-xdg-shell.c#L278-L292, https://github.com/GNOME/gnome-shell/blob/51.1/src/shell-window-tracker.c#L191-L209
- Consequence: a file named `<app_id>.desktop` is matched on Wayland and X11; `StartupWMClass` is needed only when the names differ. Zed ships `$APP_ID.desktop` with no `StartupWMClass`, and PNG icons in hicolor `512x512` and `1024x1024`. `Z/script/bundle-linux#L213-L240`, `Z/crates/zed/resources/zed.desktop.in`

#### A5. Desktop services

- File dialogs: the FileChooser portal (a D-Bus service, xdg-desktop-portal, that shows system dialogs) through the `ashpd` crate. No portal means an error; files and folders cannot be mixed. `Z/crates/gpui_linux/src/linux/platform.rs#L60-L61`, `#L819-L945`
- Open URL: the `open` crate's launchers (`xdg-open` and others) first, then the OpenURI portal. `Z/crates/gpui_linux/src/linux/platform.rs#L1180-L1239`
- Clipboard: Wayland data device and primary selection; X11 has its own implementation. `Z/crates/gpui_linux/src/linux/wayland/clipboard.rs#L16-L22`, `Z/crates/gpui_linux/src/linux/x11/clipboard.rs`
- Appearance: portal `color-scheme`; "no preference" maps to light. The accent color is not read. `Z/crates/gpui_linux/src/linux/xdg_desktop_portal.rs#L46-L133`, `#L186-L191`
- Cursor: `wp_cursor_shape_v1` when offered, else a theme loaded with the portal's `cursor-theme` and `cursor-size`. `Z/crates/gpui_linux/src/linux/wayland/client.rs#L853-L854`, `Z/crates/gpui_linux/src/linux/wayland/cursor.rs#L55-L62`
- Fractional scaling: `wp_fractional_scale_v1` with `wp_viewporter`. On X11: `GPUI_X11_SCALE_FACTOR`, then `Xft.dpi`, then RandR. `Z/crates/gpui_linux/src/linux/wayland/window.rs#L1232-L1236`, `Z/docs/src/linux.md#L308-L314`
- IME (input method editor, for composing non-Latin text): `zwp_text_input_v3` on Wayland, XIM on X11. No direct IBus code. `Z/crates/gpui_linux/src/linux/wayland/client.rs#L57-L61`, `Z/crates/gpui_linux/Cargo.toml#L126-L130`
- Keyboard: xkbcommon; the compose table follows `LC_CTYPE`. `Z/crates/gpui_linux/src/linux/platform.rs#L1286-L1313`
- Fonts: cosmic-text, which loads the system fonts. `Z/crates/gpui_wgpu/src/cosmic_text_system.rs#L131-L145`
- `.SystemUIFont` resolves to "IBM Plex Sans" on Linux, hard-coded, not to the desktop's font. `.ZedSans` is IBM Plex Sans and `.ZedMono` is Lilex. Zed embeds those fonts; GPUI does not. `Z/crates/gpui_linux/src/linux/platform.rs#L130`, `Z/crates/gpui/src/text_system.rs#L1266-L1276`
- A missing family falls through: `.ZedMono`, `.ZedSans`, Helvetica, Segoe UI, Ubuntu, Adwaita Sans, Cantarell, Noto Sans, DejaVu Sans, Arial. GPUI panics when none exists. `Z/crates/gpui/src/text_system.rs#L261-L273`, `#L604-L628`
- Fedora 45 has Adwaita Sans and no IBM Plex Sans (probe: `fc-match`).

#### A6. Limitations for a third-party app

- Not implemented on Linux: `register_url_scheme`, `add_recent_document`, `set_dock_menu`, app-level `activate` and `hide`. `set_menus` only stores the menus (no native menu bar). `Z/crates/gpui_linux/src/linux/platform.rs#L755-L765`, `#L1047-L1059`, `#L1157-L1177`
- No tray icon API. Screen capture is not implemented on Wayland. `Z/crates/gpui_linux/src/linux/wayland/client.rs#L1277-L1284`
- Zed's own Flatpak leaves the sandbox at startup. `Z/docs/src/development/linux.md#L85-L89`
- Zed's release binaries need glibc 2.31 on x86_64 and 2.35 on aarch64. `Z/docs/src/linux.md#L22-L27`

### Not verified (A)

- Zed's issue tracker was not reviewed; A6 comes from source only.
- The undecorated window on GNOME was not observed on screen.
- Which `-devel` packages a GPUI build strictly needs (the probe machine had all of them).
- Behavior on a machine with no Vulkan and no GL driver was not tested.

### B. Desktop integration specs

- XDG Base Directory 0.8. `$XDG_DATA_HOME` (`~/.local/share`): "user-specific data files". `$XDG_STATE_HOME` (`~/.local/state`): data that "should persist between (application) restarts", with logs as an example. `$XDG_CACHE_HOME` (`~/.cache`): "non-essential data files". `$XDG_CONFIG_HOME` (`~/.config`). https://specifications.freedesktop.org/basedir/latest/
- `$XDG_RUNTIME_DIR` holds "sockets, named pipes". It "MUST be owned by the user", mode "MUST be 0700", on a local file system with working file locking. It is created at first login and removed when the user "fully logs out"; files "MUST not survive reboot or a full logout/login cycle". Large files should not go there. If unset, use a replacement and "print a warning message". (same URL)
- "User-specific executable files may be stored in `$HOME/.local/bin`." Nothing else covers executables. (same URL)
- Fit (inference): database in data; logs in state; socket and its lock in runtime; a lock that guards on-disk data next to that data; downloaded agent programs in data (cache is "non-essential", so a cleaner may delete it).
- A socket path under `/run/user/<uid>/` is short, which matters for the 108-byte `sun_path` limit (section C); a path under a deep home directory may not fit.
- Desktop Entry 1.5: only `Type` and `Name` are required; `Exec` is required unless `DBusActivatable=true`. `StartupWMClass` states that the app maps a window with that WM class. https://specifications.freedesktop.org/desktop-entry/latest/recognized-keys.html
- The file name should be reverse-DNS and lives in an `applications` folder of a data directory. `$XDG_DATA_HOME` outranks `$XDG_DATA_DIRS` (`/usr/local/share/:/usr/share/`). https://specifications.freedesktop.org/desktop-entry/latest/file-naming.html
- Icon Theme 0.13: implementations must fall back to `hicolor`. "Minimally you should install a 48x48 icon in the hicolor theme." An SVG in `hicolor/scalable/apps` serves every size. https://specifications.freedesktop.org/icon-theme/latest/
- AppStream: `/usr/share/metainfo/<id>.metainfo.xml`; required for a desktop app: `id`, `name`, `summary`, `description`, `metadata_license`, and `launchable` to be launchable. https://www.freedesktop.org/software/appstream/docs/sect-Metadata-Application.html
- Flathub adds: screenshots, releases, OARS content rating, developer, project license, homepage URL. https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines

Not verified (B): GNOME Software's own requirements; a per-user icon path in the icon spec (`~/.local/share/icons` is unverified, from memory); `DBusActivatable` details.

### C. A long-lived per-user daemon

- User units of an app installed in the home directory go in `$XDG_DATA_HOME/systemd/user`; `~/.config/systemd/user` is the user's own configuration. `%t` is `$XDG_RUNTIME_DIR`. https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html
- Socket activation (systemd listens, and starts the service at the first connection): `ListenStream=/path` creates a Unix socket; `SocketMode=` defaults to 0666; the service gets it as file descriptor 3 with `LISTEN_PID` and `LISTEN_FDS`. https://www.freedesktop.org/software/systemd/man/latest/systemd.socket.html, https://www.freedesktop.org/software/systemd/man/latest/sd_listen_fds.html
- `Type=notify`: the service sends `READY=1` to `$NOTIFY_SOCKET`. `Restart=` defaults to `no`, `RestartSec=` to 100ms, the start limit to 5 starts in 10s. `KillMode=control-group` kills every remaining process of the unit at stop. https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html, https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html
- `KillUserProcesses=` only governs the session scope (a scope is a systemd unit that groups processes started by something else). Upstream default "yes"; Fedora 45 builds it as "no" (probe: local `logind.conf(5)` and the logind D-Bus property). https://www.freedesktop.org/software/systemd/man/latest/logind.conf.html
- "After the user logs out of the last session, user@.service and all services underneath it are terminated", unless lingering is enabled; the delay is `UserStopDelaySec=` (10s). https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html
- On GNOME a GUI app runs under the user manager, not in the session scope: this shell is in `user@1000.service/app.slice/app-gnome-*.scope` (probe). Inference: a daemon detached by the client stays there, survives the window, and dies at logout whatever `KillUserProcesses` says.
- Lingering: "a user manager is spawned for the user at boot and kept around after logouts". On Fedora 45 a user may enable it for themselves without authentication (probe: `pkaction`, `set-self-linger` is `yes`). https://www.freedesktop.org/software/systemd/man/latest/loginctl.html
- `systemd-run --user --scope` starts a command in its own transient scope under the user manager. A scope still ends with the user manager, so it does not replace lingering. (same systemd-run URL)
- A user service with `WantedBy=default.target` starts with the user manager: at login, or at boot with lingering. https://www.freedesktop.org/software/systemd/man/latest/systemd.special.html
- The user manager of a GNOME session holds `WAYLAND_DISPLAY`, `DISPLAY`, and `DBUS_SESSION_BUS_ADDRESS` (probe).
- `PR_SET_PDEATHSIG`: the "parent" is the thread that created the process; cleared in a `fork` child and on set-user-ID exec; racy if the parent died first. https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html
- `SO_PEERCRED` returns the peer's credentials as of `connect(2)`. `sun_path` is 108 bytes. Socket file permissions should not be the only protection in portable programs. https://man7.org/linux/man-pages/man7/unix.7.html
- On NFS, `flock()` is emulated with `fcntl()` byte-range locks since Linux 2.6.12, so the two interact. Classic `fcntl` locks drop when the process closes any descriptor of the file; OFD locks (Linux 3.15) do not. https://man7.org/linux/man-pages/man2/flock.2.html, https://man7.org/linux/man-pages/man2/fcntl_locking.2.html

Not verified (C): the `PIDs` property of `StartTransientUnit`; whether other desktops import the display variables into the user manager.

### D. Fedora specifics

- Fedora 43 removed the GNOME X11 session ("Users will no longer have the \"GNOME on Xorg\" session"); "X11 applications are still supported on GNOME". The page adds that GDM can still start X11 sessions of other desktops. https://fedoraproject.org/wiki/Changes/WaylandOnlyGNOME
- Fedora 40 removed the Plasma X11 session and keeps Xwayland. https://fedoraproject.org/wiki/Changes/KDE_Plasma_6
- Fedora 45 ships GNOME 51 and Plasma 6.8. The probe machine has only Wayland sessions for GNOME and Plasma, and Xwayland installed. https://packages.fedoraproject.org/pkgs/gnome-shell/gnome-shell/, https://packages.fedoraproject.org/pkgs/plasma-workspace/plasma-workspace/
- No Plasma X11 session package exists for Fedora (`plasma-workspace-x11` is listed for EPEL only). https://packages.fedoraproject.org/pkgs/plasma-workspace/plasma-workspace-x11/
- SELinux (probe, enforcing): the user runs as `unconfined_t`; files under `~/.local/share` get the label `data_home_t`, `~/.local/bin` gets `home_bin_t`. `selinuxuser_execheap` is off; `selinuxuser_execmod` and `selinuxuser_execstack` are on. Tunable meanings: https://github.com/fedora-selinux/selinux-policy/blob/rawhide/policy/global_tunables
- Fedora Atomic: `/` and `/usr` are read-only; `/home` is `/var/home`, `/opt` is `/var/opt`, `/usr/local` is `/var/usrlocal`. Flatpak is "the primary way" to install GUI apps. https://github.com/fedora-silverblue/silverblue-docs/blob/master/modules/ROOT/pages/technical-information.adoc, https://github.com/fedora-silverblue/silverblue-docs/blob/master/modules/ROOT/pages/getting-started.adoc
- A Flatpak app starts host programs only with `flatpak-spawn --host`, which needs access to `org.freedesktop.Flatpak`. https://docs.flatpak.org/en/latest/flatpak-command-reference.html
- glibc (the C library every binary links): Fedora 43, 44, 45 have 2.42, 2.43, 2.44. Ubuntu 22.04, 24.04, 26.04 have 2.35, 2.39, 2.43. Debian 12, 13 have 2.36, 2.41. https://packages.fedoraproject.org/pkgs/glibc/glibc/, https://packages.ubuntu.com/search?keywords=libc6&searchon=names&exact=1&suite=all&section=all, https://packages.debian.org/search?keywords=libc6&searchon=names&exact=1&suite=all&section=all
- Support: Ubuntu 22.04 standard support ends May 2027; Debian 12 left regular support on 2026-07-11 (LTS to 2028-06-30). https://ubuntu.com/about/release-cycle, https://www.debian.org/releases/
- Rust's `x86_64-unknown-linux-gnu` target needs glibc 2.17 and kernel 3.2. https://doc.rust-lang.org/nightly/rustc/platform-support.html
- The real floor is the build machine: the probe binary built on Fedora 45 requires `GLIBC_2.44` symbols (probe: `objdump -T`).

Not verified (D): RHEL glibc versions (2.28 for 8, 2.34 for 9, 2.39 for 10: unverified, from memory and search snippets); RHEL lifecycle dates; Fedora's lifecycle page; whether Xwayland and `mesa-vulkan-drivers` are in the default install; any Fedora or Red Hat document on running downloaded files as an unconfined user (only the probe above); `noexec` mount defaults.

## Research: distribution (provider agents, packaging, continuous integration)

Method: upstream release pages, registry metadata, official docs, and license texts, read on 2026-11-16. "Probe" marks a fact read from an artifact itself: the pinned Linux archives were downloaded, their SHA-256 compared with `pins.rs` (all 9 match), and their binaries inspected with `readelf` without being run. "Local" marks a fact observed on the development computer (Fedora 45, glibc 2.44, x86_64).

Terms: glibc and musl are the two common C libraries of Linux (the base library every program loads). A static binary carries its C library inside and needs none from the system. aarch64 is 64-bit ARM.

### A1. Provider agents: Linux artifacts

| Agent | Pinned / latest | Linux artifacts upstream | C library need (probe) | Checksums published |
|---|---|---|---|---|
| `claude-agent-acp` | 0.107.0 / 0.107.3 (tag), 0.107.2 (last with archives) | `claude-agent-acp-linux-{x64,arm64}.tar.gz` (glibc) and `-linux-{x64,arm64}-musl.tar.gz`; each holds `claude-agent-acp` and `claude` | `claude-agent-acp`: glibc 2.17; `claude`: glibc 2.26; only libc, libm, libdl, libpthread, librt | SHA-256 digest per asset on the GitHub release |
| Bun (runs `codex-acp`) | 1.4.4 / 1.4.4 | `bun-linux-{x64,x64-baseline,aarch64}.zip`, plus `-musl` variants | glibc 2.17 | `SHASUMS256.txt` with a PGP signature, and release digests |
| `codex-acp` | 2.6.0 / 2.6.0 | npm tarball only (one JavaScript file, 2.1 MB); its GitHub releases hold no archive | none (JavaScript) | npm `integrity` (SHA-512), registry signatures, provenance attestation |
| Codex CLI | 0.166.0 / 0.169.0 | npm `codex-<version>-linux-{x64,arm64}.tgz`; GitHub `codex-{x86_64,aarch64}-unknown-linux-musl.tar.gz` with sigstore files. musl only, no glibc build | static (musl), no system library | npm SHA-512 and attestation; sigstore bundles on GitHub |
| `grok` | 1.0.77 / 1.0.77 (stable), 1.0.87 (alpha) | `https://x.ai/cli/grok-<version>-linux-{x86_64,aarch64}.gz`, one gzip of one binary; also npm `@xai-official/grok-linux-{x64,arm64}` | static, no system library | none beside the binary (the install script checks no hash); the npm package has a SHA-512 |

Sources: [claude-agent-acp v0.107.0 release](https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v0.107.0), [its README, "Standalone executables"](https://github.com/agentclientprotocol/claude-agent-acp#standalone-executables), [Bun 1.4.4 release](https://github.com/oven-sh/bun/releases/tag/bun-v1.4.4), [npm codex-acp](https://registry.npmjs.org/@agentclientprotocol/codex-acp), [npm @openai/codex](https://registry.npmjs.org/@openai/codex), [Codex 0.166.0 release](https://github.com/openai/codex/releases/tag/rust-v0.166.0), [Grok install script](https://x.ai/cli/install.sh), [Grok stable pointer](https://x.ai/cli/stable), [npm @xai-official/grok](https://registry.npmjs.org/@xai-official/grok).

- Every Linux address in `pins.rs` answers HTTP 200 and its SHA-256 matches, for x86_64 and aarch64 (probe). No pinned Linux target points at a missing artifact.
- All three are self-contained: no Node.js or Bun is needed on the computer. The Claude archive "needs no Node.js install" ([README](https://github.com/agentclientprotocol/claude-agent-acp#standalone-executables)); Codex and Grok are static binaries (probe); the daemon brings Bun for `codex-acp`.
- Bun states "glibc binaries require glibc 2.17 or newer", and the `-baseline` assets are now aliases of the single x64 build ([Bun install docs](https://bun.com/docs/installation)). Claude Code names Ubuntu 20.04+, Debian 10+, Alpine 3.19+ ([setup](https://code.claude.com/docs/en/setup)).
- Missing target: musl. `platform()` in `pins.rs` returns a platform only under `cfg!(target_env = "gnu")`, a test of how the daemon was built, not of the computer it runs on. A daemon built as a static musl binary would find no runtime at all, although Codex and Grok run anywhere and Claude and Bun publish musl archives. The host C library must be detected at run time.
- A release may have no archives: v0.107.3 (published 14:35 UTC today) and v0.105.0 show 0 assets. The build of archives does not gate the release ([RELEASES.md, "The release has no executables"](https://github.com/agentclientprotocol/claude-agent-acp/blob/main/docs/RELEASES.md)). A pin bump must check the assets exist.
- Unpacked sizes are large: Claude about 475 MB, Codex about 456 MB, Grok about 200 MB (probe).

Not verified: the artifacts were not run on Linux. The musl Claude archive needs `libgcc` and `libstdc++` on Alpine per the README; not probed. Codex 0.169.0 was not downloaded.

### A2. Sign-in on Linux

- **Claude.** The sign-in opens a browser; if it does not open, the terminal shows a link, and a code is pasted back into the terminal ([authentication](https://code.claude.com/docs/en/authentication)). On Linux credentials are a plain file, `~/.claude/.credentials.json`, mode 0600, with no keyring; `CLAUDE_CONFIG_DIR` moves it (same page, "Credential management"). For a computer with no browser the documented path is `claude setup-token`, which prints a one-year token for the `CLAUDE_CODE_OAUTH_TOKEN` variable (same page). That path makes the caller hold a credential, which the daemon's rules forbid.
- **Codex.** The default sign-in opens a browser and receives the result on `localhost:1455`; `codex login --device-auth` is the device-code flow (a code typed on another device), in beta, and must be enabled in the ChatGPT security settings ([Codex authentication](https://developers.openai.com/codex/auth)). Credentials: `auth.json` under `CODEX_HOME` by default (`file` is the default of `cli_auth_credentials_store` at the pinned tag, [types.rs](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/config/src/types.rs)); `keyring` and `auto` exist. `codex-acp` 2.6.0 offers a method `chat-gpt` that opens the page through the npm package `open` (which runs `xdg-open` on Linux), hidden when `NO_BROWSER` is set, and a method `chat-gpt-device-code` offered only to a client that supports URL elicitation (probe of `dist/index.js` and `README.md` inside the [pinned tarball](https://registry.npmjs.org/@agentclientprotocol/codex-acp/-/codex-acp-2.6.0.tgz)).
- **Grok.** `grok login` opens a browser; `grok login --device-auth` follows RFC 8628 for hosts with no browser ([enterprise docs](https://docs.x.ai/build/enterprise)). Credentials: `~/.grok/auth.json`, mode 0600, no keyring ([authentication guide](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md)); `GROK_HOME` moves the home ([settings](https://docs.x.ai/build/settings)).
- **Sandboxes.** Codex on Linux isolates commands with bubblewrap (a tool that runs a program in a restricted view of the system) plus seccomp (a kernel filter of system calls). It uses the first `bwrap` on `PATH`, else a `bwrap` bundled in the package, which needs unprivileged user namespaces ([Codex sandbox](https://developers.openai.com/codex/concepts/sandboxing), [linux-sandbox README](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/linux-sandbox/README.md)). The pinned npm package holds `codex-resources/bwrap` (probe). Grok's sandbox uses Landlock (a kernel access-control module) and is off by default ([Grok sandbox](https://docs.x.ai/build/features/sandbox)). Claude Code's optional sandbox needs `bubblewrap` and `socat` ([sandboxing](https://code.claude.com/docs/en/sandboxing)).
- **Fedora defaults (local).** `bubblewrap` 0.13.1 is installed as a dependency of `flatpak`; `bwrap --unshare-all` and `unshare -Ur` succeed; Landlock is in the active kernel modules; there is no AppArmor restriction (Fedora uses SELinux). `xdg-open` is present.

Not verified: no sign-in was run on Linux. How Claude Code opens the browser is not documented (its binary holds the string `xdg-open`, probe). Whether Claude's printed link can finish with no terminal on a server. Fedora Server defaults (only a Workstation was observed).

### B1. Fedora: official repositories and alternatives

- Fedora allows only free and open source licenses, and the rule covers COPR too: "everything available at ... Copr repositories is subject to the same licensing rules" ([license approval](https://docs.fedoraproject.org/en-US/legal/license-approval/)).
- PolyForm Noncommercial is in neither Fedora list (text search of [allowed](https://docs.fedoraproject.org/en-US/legal/allowed-licenses/) and [not-allowed](https://docs.fedoraproject.org/en-US/legal/not-allowed-licenses/)). It permits only noncommercial purposes ([license text](https://polyformproject.org/licenses/noncommercial/1.0.0/)), so it cannot meet the criteria; comparable licenses (BUSL-1.1, CC-BY-NC) are listed not-allowed.
- Second blocker, independent of the license: "Software which downloads code bundles from the internet in order to be functional or useful is not acceptable for inclusion in Fedora", and no pre-built binaries ([what can be packaged](https://docs.fedoraproject.org/en-US/packaging-guidelines/what-can-be-packaged/)).
- COPR (Fedora's public build service for personal repositories) refuses material "governed in whole or in part by a license not contained in the list of acceptable licenses for Fedora"; it does not require the packaging guidelines ([COPR docs](https://docs.copr.fedorainfracloud.org/user_documentation.html#what-i-can-build-in-copr)). The daemon cannot go there.
- RPM Fusion `nonfree` accepts "software with publicly available source-code that has 'no commercial use'-like restrictions", but follows the Fedora packaging guidelines ([RPM Fusion contributors](https://rpmfusion.org/Contributors)), so the run-time download rule still stands in the way.
- What remains: a self-hosted dnf repository or a plain RPM. A repository is a `.repo` file with `baseurl`, `gpgkey`, and signature checks (`pkg_gpgcheck` for packages, `repo_gpgcheck` for metadata, [dnf5.conf](https://dnf5.readthedocs.io/en/latest/dnf5.conf.5.html)). Anthropic ships Claude Code to Fedora exactly this way ([setup, "Install with Linux package managers"](https://code.claude.com/docs/en/setup#install-with-linux-package-managers)).

Not verified: no Fedora legal review of PolyForm was found; the conclusion is from the stated criteria. The Fedora License Data repository could not be searched (GitLab API refused).

### B2. Packaging a Rust application for Fedora

- A project that is not a published crate uses the generic naming, `%cargo_prep`, `%cargo_build`, and `install`, not `%cargo_install` ([Rust guidelines, "Non-crate Rust projects"](https://docs.fedoraproject.org/en-US/packaging-guidelines/Rust/)).
- Vendoring (shipping the sources of all dependencies in the source package): `%cargo_prep -v vendor`, never `%cargo_generate_buildrequires`, and `%cargo_vendor_manifest` output shipped as a `%license` file. `rust2rpm` in vendor mode writes the spec and the vendor tarball. A git dependency (GPUI here) must become a bundled path dependency (same page, "Vendored dependencies").
- These rules bind only inside Fedora and RPM Fusion. A self-hosted RPM may be built any way.
- `cargo-generate-rpm` writes a binary `.rpm` from `Cargo.toml` metadata with no spec file and no `rpmbuild`, and can sign it; it does not build or strip the binary ([README](https://github.com/cat-in-136/cargo-generate-rpm)). `cargo-deb` does the same for `.deb` ([README](https://github.com/kornelski/cargo-deb)).
- `dist` (formerly cargo-dist) builds archives and these installers: shell script, PowerShell, npm, Homebrew, MSI. It makes no RPM, deb, Flatpak, or AppImage ([installers](https://axodotdev.github.io/cargo-dist/book/installers/index.html)).

Not verified: none of these tools was run on this repository, which has two workspaces and one `[patch.crates-io]` block.

### C1. Flatpak and Flathub

- Flathub does not require an open source license: content "must allow legal redistribution" and the license must be declared ([requirements, "License"](https://docs.flathub.org/docs/for-app-authors/requirements#license)).
- The build has no network and must be from source; "Binary or precompiled files must not be present" (same page). GPUI's git dependencies must be listed as sources.
- Flathub's generative AI policy: "Flathub manifests must not contain AI-generated or AI-assisted content", and AI tools must not open submission pull requests (same page, "Generative AI policy").
- Development tools "with a broad scope" are accepted only from upstream, and static permissions "must be kept to an absolute minimum" (same page).
- Running a program outside the sandbox is `flatpak-spawn --host`, which needs `--talk-name=org.freedesktop.Flatpak` ([command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html)); the Flathub linter flags it as `finish-args-flatpak-spawn-access` ([linter](https://docs.flathub.org/docs/for-app-authors/linter)). The Background portal lets an app ask to keep running with no window and to start at login ([Background portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Background.html)).
- Zed: its Flatpak "exits the sandbox on startup" and Zed "currently does not work well with sandboxes" because it installs and runs developer tools ([Zed development docs](https://zed.dev/docs/development/linux)). Its manifest asks for `--filesystem=host` and `--talk-name=org.freedesktop.Flatpak` ([manifest](https://github.com/zed-industries/zed/blob/main/crates/zed/resources/flatpak/manifest-template.json)) and its launcher re-executes itself through `/usr/bin/flatpak-spawn --host` ([cli/src/main.rs](https://github.com/zed-industries/zed/blob/main/crates/cli/src/main.rs)). Zed lists the Flathub package among third-party packages ([Zed on Linux](https://zed.dev/docs/linux)).

Not verified: whether Codex's bubblewrap sandbox works inside a Flatpak sandbox (unverified, from memory: nested user namespaces are blocked there). Whether Flathub would accept a daemon that downloads executables.

### C2. Tarball and install script (Zed), AppImage, deb

- Zed's official channel is `curl -f https://zed.dev/install.sh | sh` ([Zed on Linux](https://zed.dev/docs/linux)). The [script](https://zed.dev/install.sh) downloads `zed-linux-<arch>.tar.gz`, unpacks it to `~/.local/zed.app/`, links `~/.local/bin/zed` to `zed.app/bin/zed`, copies `dev.zed.Zed.desktop` to `~/.local/share/applications/`, rewrites its `Icon=` and `Exec=` lines to absolute paths, and warns about missing libraries found with `ldd`. No root is needed.
- Layout of the archive ([bundle-linux](https://github.com/zed-industries/zed/blob/main/script/bundle-linux)): `bin/zed` (launcher), `libexec/zed-editor` (the application), `lib/` (bundled libraries, without glibc and the GLib family), `share/icons/hicolor/...`, `share/applications/<id>.desktop`, `licenses.md`.
- Update: Zed downloads the new archive, extracts it with `tar`, and copies it over the install with `rsync -av --delete`; `rsync` is required ([auto_update.rs](https://github.com/zed-industries/zed/blob/main/crates/auto_update/src/auto_update.rs)). Packagers turn it off with `ZED_UPDATE_EXPLANATION` ([development docs](https://zed.dev/docs/development/linux)).
- AppImage (one executable file that mounts its own file system): needs FUSE on the host ([FUSE](https://docs.appimage.org/user-guide/troubleshooting/fuse.html)); binaries must be built on a system no newer than the oldest target ([best practices](https://docs.appimage.org/reference/best-practices.html)); updates come from update information embedded with `appimagetool -u` and a `.zsync` file ([updates](https://docs.appimage.org/packaging-guide/optional/updates.html)).

Not verified: no AppImage of a GPUI application was examined; Zed publishes none.

### C3. The glibc baseline

- Local evidence: the debug builds made on Fedora 45 require glibc 2.39 (`crowbotd`) and glibc 2.43 (`crowbot`), read with `readelf -V`. Neither would start on Ubuntu 22 (glibc 2.35 per [Zed on Linux](https://zed.dev/docs/linux)).
- Rust itself supports glibc 2.17 and later on x86_64 and aarch64 ([platform support](https://doc.rust-lang.org/nightly/rustc/platform-support.html)); the requirement comes from the build computer.
- Three ways: build on an old distribution (Zed bundles both architectures on runners named `...-ubuntu-2004`, [release.yml](https://github.com/zed-industries/zed/blob/main/.github/workflows/release.yml), and states glibc 2.31 for x86_64, 2.35 for aarch64, [Zed on Linux](https://zed.dev/docs/linux)); `cargo-zigbuild` with a suffix such as `--target x86_64-unknown-linux-gnu.2.17`, with stated caveats about system headers ([README](https://github.com/rust-cross/cargo-zigbuild)); a static musl build.
- Daemon: static musl is plausible. It links SQLite bundled (`rusqlite` feature `bundled`), TLS through `rustls` with `aws-lc-rs` and no OpenSSL (`Cargo.toml`, `Cargo.lock`), and today loads only libc, libm, libgcc (local). `aws-lc-rs` supports both musl targets ([platform support](https://aws.github.io/aws-lc-rs/platform_support.html)). Zed ships its headless `remote_server` exactly so ([bundle-linux](https://github.com/zed-industries/zed/blob/main/script/bundle-linux)).
- Client: not static. A GPUI application loads Wayland, X11, xkbcommon, fontconfig, and Vulkan libraries of the system (build packages in Zed's [script/linux](https://github.com/zed-industries/zed/blob/main/script/linux)); Zed builds it against an old glibc and bundles the other libraries.
- Repository finding: `crates/crowbot-desktop/Cargo.toml` enables neither the `wayland` nor the `x11` feature of `gpui_platform`, and its `Cargo.lock` holds no `wayland-client`, `x11rb`, or `xkbcommon`. In Zed's workspace `gpui_linux` has no default features, so the Linux client appears to be built with no display backend.

Not verified: a musl build of the daemon was not attempted (it needs a new Rust target on the computer). The client was not started on Linux, so the missing display backend is read from the manifests only. Certificate lookup of `rustls-platform-verifier` under musl. Whether `cargo-zigbuild` can link a GPUI application. Release builds were not measured (debug builds only).

### C4. Self-update against package managers

- Tarball plus script: the application updates itself (Zed above; Claude Code's native install updates in the background, [setup](https://code.claude.com/docs/en/setup)). `dist` offers an updater program and the `axoupdater` library ([updater](https://axodotdev.github.io/cargo-dist/book/installers/updater.html)).
- dnf, apt: the package manager updates; Claude Code states that apt and dnf installs "do not auto-update" (same setup page). Files under `/usr` belong to root, so a self-update cannot write them.
- Flatpak: updates come from Flatpak; Zed sets "Please use flatpak to update zed". AppImage: zsync update information.
- The provider runtimes live under `--data-dir`, owned by the user, in every channel: the daemon's pinned downloads work the same in all of them.

### D. Continuous integration

- GitHub-hosted Linux labels: x86_64 `ubuntu-latest` (24.04), `ubuntu-24.04`, `ubuntu-22.04`; arm64 `ubuntu-24.04-arm`, `ubuntu-22.04-arm`. Both architectures are listed for public repositories (4 CPU) and for private ones (2 CPU, billed) ([GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)). The image list also names `ubuntu-26.04` and `ubuntu-26.04-arm` ([runner-images](https://github.com/actions/runner-images)).
- No Fedora image exists. A Fedora job is a container job (`jobs.<id>.container`), which needs an Ubuntu runner ([workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)); Fedora publishes its images on `registry.fedoraproject.org` ([Fedora containers](https://docs.fedoraproject.org/en-US/containers/)).

Not verified: whether a container job can test the client (it needs a display and a GPU) and Codex's bubblewrap sandbox (containers often block user namespaces).

## Research: supervision, window test, musl

Added on 2026-11-16 for `tasks/prd-crow-bot-linux-port.md`. "(probe)" marks a command run on the same Fedora 45 computer.

- Syncthing offers a user unit and a system unit, installed by its packages; a user of the archive copies the unit to `~/.config/systemd/user/` by hand. It documents `loginctl enable-linger` to start a user unit at boot. https://docs.syncthing.net/users/autostart.html
- Podman's package installs `/usr/lib/systemd/user/podman.socket` and `podman.service`, with the socket at `%t/podman/podman.sock`, and pairs them with `loginctl enable-linger`. It uses socket activation (systemd holds the socket and starts the service at the first connection). https://docs.podman.io/en/latest/markdown/podman-system-service.1.html
- Ollama and Tailscale run as system units, Ollama under a dedicated account. https://raw.githubusercontent.com/ollama/ollama/main/docs/linux.mdx, https://tailscale.com/docs/install/linux
- No product that was checked writes its own unit at first run. Docker Desktop and Zed's remote server were not checked.
- With `Type=notify`, systemd counts the service as started when it sends `READY=1`. `TimeoutStopSec` bounds a stop, then systemd sends SIGKILL. https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html
- The default `KillMode=control-group` ends every process of the unit's control group; `process` and `none` are "not recommended". https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html
- Measured on 2026-11-18 (review of EP-003): with `control-group`, `systemctl stop` sends SIGTERM to the agents and their guards at the same time as to the daemon, so a running turn is stored as failed ("Process exited with signal: 15") or as an unresponsive agent, not as stopped. The unit uses `KillMode=mixed`: SIGTERM to the daemon alone, which ends its turns and agents itself, then SIGKILL to every process left in the unit once the daemon is gone, also after a crash (`tests/daemon/service.rs`).
- Crates for the readiness message: `sd-notify` 0.5.0 and `libsystemd` 0.7.2. Whether `sd-notify` links libsystemd was not read. https://crates.io/crates/sd-notify, https://crates.io/crates/libsystemd
- Zed's continuous integration starts no window: its Linux jobs run `./script/linux` and `cargo nextest`, with no headless compositor and no software Vulkan. One job checks `remote_server` for `x86_64-unknown-linux-musl`. https://raw.githubusercontent.com/zed-industries/zed/main/.github/workflows/run_tests.yml
- A real window opens with no screen (probe): `dbus-run-session` around `mutter --headless --wayland --no-x11 --virtual-monitor 1280x800 --wayland-display <name>`, then the windowed client with `WAYLAND_DISPLAY=<name>` and `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`. The client attached to its daemon, loaded `libvulkan_lvp`, and was alive after 6 seconds. Not tried in a container.
- `VK_DRIVER_FILES` points the Vulkan loader at one driver file and replaces `VK_ICD_FILENAMES`. https://docs.mesa3d.org/envvars.html
- Weston's headless backend is the documented alternative, with `--renderer=pixman` for CPU rendering. https://wayland.pages.freedesktop.org/weston/toc/running-weston.html
- `aws-lc-rs` outside FIPS needs only a C compiler; CMake and Go are "never required"; both musl targets have generated bindings. https://aws.github.io/aws-lc-rs/requirements/linux.html
- On Linux, `rustls-platform-verifier` reads the system certificate bundle. Its documentation does not mention musl. https://github.com/rustls/rustls-platform-verifier
- Projects report a slow allocator under musl and switch to mimalloc for that target; the evidence is issue reports, not documentation. https://github.com/sharkdp/fd/issues/710
- `musl-gcc` and `cmake` are installed on the probe computer; the Rust musl targets are not (probe).

Not verified: a musl build of the daemon; a headless compositor inside a container job; musl DNS differences; the Zed issue tracker for the NVIDIA driver, input methods, and fractional scaling.
