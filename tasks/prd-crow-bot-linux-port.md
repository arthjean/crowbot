[PRD]
# PRD: Crow Bot Linux port

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-16 | Arthur Jean | Initial draft |
| 1.1 | 2026-11-16 | Arthur Jean | Distribution removed: the Distribution epic and the musl runtime story leave the PRD; the metainfo file leaves US-009 |
| 1.2 | 2026-11-16 | Arthur Jean | US-007 moves from EP-001 to its own epic, EP-007, which follows EP-001 and EP-002 |
| 1.3 | 2026-11-17 | Arthur Jean | Every check that needs Windows leaves its story for a final epic, EP-008, so that the Linux work is certified on Linux alone |
| 1.4 | 2026-11-17 | Arthur Jean | What GPUI cannot do on Linux at the pinned rev (COLRv1 emoji, the AT-SPI bridge) leaves US-002 and US-006 for a new story, US-026 in EP-009; the check of KDE's button layout moves from US-004 to US-007 |
| 1.5 | 2026-11-17 | Arthur Jean | EP-009 moves before EP-007, since US-007 is blocked by US-026 and US-026 only by EP-001 |
| 1.6 | 2026-11-18 | Arthur Jean | GPUI does publish to AT-SPI at the pinned rev, once `org.a11y.Status.IsEnabled` is true (EP-009 corrected); US-007 checks a message with emoji and Orca on a real session |

## Problem Statement

1. The desktop client cannot show a window on Linux. `crates/crowbot-desktop/Cargo.toml:28-29` enables neither the `wayland` nor the `x11` feature of `gpui_platform`, so the binary holds no display backend. Its 225 tests pass all the same, because they run on GPUI's test platform.
2. With the backends enabled, the window has no chrome on GNOME. Mutter draws no title bar for an application, GPUI draws none, and the client asks for none. The window cannot be moved, resized, minimized, or closed with the pointer. Under Xwayland the opposite happens: Mutter adds a 37 px title bar above the client's own 38 px band.
3. No desktop can tie the window to an application: there is no application ID, no desktop entry, and no icon.
4. The daemon is correct on Linux but not finished. It handles no signal and leaves its socket behind. It ends when the user logs out, because it lives in the systemd scope of the program that started it. Its files are readable by other accounts when `--db` is explicit. A socket path of 108 bytes fails after the database was created. The client writes the daemon's log into `~/.crowbot` even for a sandbox.
5. No provider was ever signed in to or run on Linux.
6. Nothing guards the Linux build. There is no continuous integration. One test fails under load, 109 fail with a long `TMPDIR`, and one fails as root.

**Why now:** The desktop core is done on Windows (`tasks/prd-crow-bot-desktop-core.md`, all 33 stories certified). The author's daily computer runs Fedora 45, and hosted servers, the paid part of the product, run Linux. The research of 2026-11-16 (`docs/linux-port.md`) measured every problem above on a real computer, so the work can be specified without guessing.

## Overview

This PRD makes Crow Bot work on Fedora Workstation, built from source, as it works on Windows. The architecture does not change: one daemon, one thin client, one protocol. The work is on the edges where the operating system shows: the window, the desktop's knowledge of the application, the daemon's life and folders, the providers, and the tests.

Distribution is out of this PRD (decided 2026-11-16): no package, archive, release build, or repository is prepared until the product works the way its owner intends on its target systems. The research for it stays in `docs/linux-port.md` (notes 20 and 28 to 32) for a later PRD.

Key decisions, each from the evidence in `docs/linux-port.md`:

- The client draws its own window chrome on Linux and hides its buttons when a compositor decorates.
- The daemon runs as a systemd user unit (a service the per-user systemd starts and supervises), installed from the repository by one documented command, with the detached start of today as the fallback.
- On Linux the files move to the XDG folders. Nobody has Linux data to migrate.
- The application ID is `io.github.arthjean.CrowBot`, held in one constant. No domain is bought before the product works on its target systems (decided 2026-11-16).

Every story lists references. Paths that start with `zed/` are in Zed's repository at the rev pinned in `crates/crowbot-desktop/Cargo.toml`, which Cargo keeps under `~/.cargo/git/checkouts/zed-*/0cfc4a4/`. "Note" followed by a number is a work item of `docs/linux-port.md`. The implementing agent reads them before it designs.

The work is phased, because it holds 26 stories:

| Phase | Epics | Stories | Outcome |
|-------|-------|---------|---------|
| A. Works from source on Fedora | EP-001, EP-002, EP-009, EP-007, EP-003, EP-004 | 20 | A window with full chrome, known to the desktop, on a daemon that stops, survives a logout, and keeps its files private, all guarded by continuous integration |
| B. Providers | EP-005 | 3 | Claude, ChatGPT, and Grok sign in and run a turn on Linux |
| C. Windows non-regression | EP-008 | 3 | The Windows client and daemon behave as before the port, and every push runs the gates on Windows |

Every story of phases A and B is certified on Linux alone. A check that needs a Windows computer or a Windows runner belongs to EP-008, which comes last.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Stories certified `DONE` by `/review-epic` | 20 (Phase A) | 26 (all phases) |
| Window actions that work with the pointer on GNOME Wayland (move, resize, minimize, maximize, close) | 5 of 5 | 5 of 5 |
| Providers that sign in and run a turn on Linux with daemon-owned approvals | 0 | 3 |
| Checks that run on every push | 12 (six gates on two systems) | 13 (with the real window test) |

## Target Users

### Solo builder on Linux
- **Role:** A developer or founder who pays for one or more AI plans and works on Fedora Workstation, GNOME on Wayland.
- **Behaviors:** Runs Claude Code or Codex in terminals; builds tools from source when no package exists.
- **Pain points:** Crow Bot does not start on their computer at all. Built from source, it gives a window that cannot be moved or closed.
- **Current workaround:** Runs Crow Bot on Windows, or runs agents in terminal tabs.
- **Success looks like:** Builds Crow Bot from source, finds it in the application grid with its icon, and uses it as on Windows. The bots keep working after the window closes and after a logout.

### Small team owner
- **Role:** The person who will run a daemon on a Linux server for a team.
- **Behaviors:** Administers servers over SSH with systemd.
- **Pain points:** The daemon has no unit and no clean stop, and its files are readable by other accounts.
- **Current workaround:** None: the daemon was never run on a server.
- **Success looks like:** This PRD does not serve the team yet. It leaves what the servers PRD needs: a unit that starts and stops the daemon, and files that only its account can read.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Zed** (same UI framework): draws client-side decorations on Linux. Its continuous integration starts no window. Crow Bot takes the decorations, and adds the window test Zed lacks.
- **Syncthing and Podman**: per-user daemons run as systemd user units that the package installs; both document lingering for a daemon that runs with no session. Podman uses socket activation; Crow Bot does not, because its start order says that an endpoint that answers belongs to a ready daemon.
- **Ollama and Tailscale**: system units under a dedicated account or root. Crow Bot stays per user: a bot runs on its owner's sign-in and folders.
- **Market gap:** agent products for the desktop are Electron applications. A native client with a daemon that outlives the session has no Linux counterpart.

### Best Practices Applied
- Mutter offers no `xdg-decoration` protocol, so an application draws its own title bar, buttons, shadow, and resize edges (`zed/crates/gpui_linux/src/linux/wayland/window.rs` lines 2215-2234).
- A file named `<app_id>.desktop` is enough for GNOME to match a window on Wayland and on X11.
- A unit of `Type=notify` is started only when the service says `READY=1`; the default `KillMode=control-group` ends every process of the unit.
- The socket belongs in `$XDG_RUNTIME_DIR`: it is owned by the user, mode 0700, and its path is short.

*Full research sources are in `docs/linux-port.md`.*

## Assumptions & Constraints

### Assumptions (to validate)
- A headless Mutter with CPU rendering runs inside a container job. Evidence: the windowed client started and stayed alive under `mutter --headless` with lavapipe on the reference computer; a container has no `/dev/dri`. Validated inside US-019.
- The archives unpacked from zip (Codex, Bun) keep their execute bit. Evidence: the tar path (Claude) installs and starts on Fedora. Validated inside US-020.
- The chrome specified from GPUI's source looks and behaves as described on a real GNOME session. Evidence: source only; no picture of the window exists. Validated by US-007.
- A systemd user unit gets the display variables on KDE as it does on GNOME. Evidence: measured on GNOME only. Validated inside US-015 and US-020.

### Hard Constraints
- The hard rules of `AGENTS.md` all stand: the license boundary, the daemon owns approvals, model usage belongs to the user, the database is the source of truth, the endpoint belongs to one user, ACP stays at the edge.
- GPUI comes from Zed's repository at the pinned rev, with Zed's patch entries. This PRD does not change the rev.
- UI work follows `DESIGN.md`; a visual change that departs from it updates it in the same commit.
- To look at the running client, use the `/agent-desktop` skill and no other tool. Never start or drive OpenBot.
- The Windows build must not change in behavior: every change is behind a Linux or Unix condition, or is proven neutral by the Windows tests. The proof on Windows is EP-008's; the stories of phases A and B do not wait for a Windows computer.
- No provider credential is read, copied, or stored, on any channel.
- The application ID is `io.github.arthjean.CrowBot` until the product owner changes it.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting of the root workspace (daemon and protocol)
- `cargo clippy --all-targets --no-deps` - lints of the root workspace, with no warning reported
- `cargo test` - tests of the root workspace
- `cargo fmt --all --check --manifest-path crates/crowbot-desktop/Cargo.toml` - formatting of the client workspace
- `cargo clippy --all-targets --no-deps --manifest-path crates/crowbot-desktop/Cargo.toml` - lints of the client workspace, with no warning reported
- `cargo test --manifest-path crates/crowbot-desktop/Cargo.toml` - tests of the client workspace

For UI stories, additional gates:
- Start `crowbotd` on a test database and a test endpoint, start the client against it on GNOME Wayland, and capture a screenshot of each state the story names with the `/agent-desktop` skill. The capture needs the user's consent in the desktop portal.
- Compare each screenshot with `DESIGN.md`: regions, sizes, radii, and states must match.

## Epics & User Stories

### EP-001: A usable window on Linux

The client opens a window on Wayland and X11 and gives it everything the desktop does not: a way to move it, buttons, resize edges, a shadow, and rounded corners.

**Definition of Done:** On GNOME Wayland the window moves, resizes from its 8 edges and corners, minimizes, maximizes, and closes with the pointer; under Xwayland and on KDE exactly one title bar shows; a start with no display or no GPU prints its reason and exits with a failure status; `DESIGN.md` holds the Linux chrome.

#### US-001: The client opens a window on Wayland and X11
**Description:** As a solo builder on Linux, I want the client built with its display backends so that starting it shows a window.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**References:**
- Note 1, note 27.
- Zed: the features, `zed/crates/gpui_platform/Cargo.toml` lines 26-27; how Zed enables them, `zed/crates/zed/Cargo.toml` lines 125-130; the backend choice, `zed/crates/gpui_linux/src/linux/display_connection.rs` lines 23-38.
- Crow Bot today: `crates/crowbot-desktop/Cargo.toml:28-29`.

**Acceptance Criteria:**
- [ ] Given a Linux build, when `cargo tree -e features -i gpui_linux` runs in the client workspace, then the `wayland` and `x11` features are active
- [ ] Given a Windows or macOS build, when the dependency tree is resolved, then no Wayland or X11 crate is compiled
- [ ] Given `WAYLAND_DISPLAY` names a running compositor, when the client starts, then a window titled "Crow Bot" opens and the process loads `libwayland-client`
- [ ] Given `WAYLAND_DISPLAY` is empty and `DISPLAY` is set, when the client starts, then a window opens on X11
- [ ] Given the lockfile was regenerated, when its new packages are compared with Zed's `Cargo.lock` at the pinned rev, then each shared package has Zed's version
- [ ] Given a computer that lacks a build package, when the build fails, then `README.md` names the Fedora packages a build needs and the ones a start needs

#### US-002: The title band moves the window and holds its buttons
**Description:** As a solo builder on Linux, I want to drag the window by its title band and use its buttons so that I control the window as in any application.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**References:**
- Note 2.
- Zed: `on_hit_test_window_control` drops its callback, `zed/crates/gpui_linux/src/linux/wayland/window.rs` line 2039 and `x11/window.rs` line 1772; the API, `zed/crates/gpui/src/window.rs` lines 3346-3390; Zed's Linux title bar, `zed/crates/platform_title_bar/src/platforms/platform_linux.rs`; the button layout, `zed/crates/gpui_linux/src/linux/xdg_desktop_portal.rs` line 70.
- Crow Bot today: the drag zones, `header.rs:40`, `rail.rs:113,183`, `bot_list.rs:39`, `settings.rs:1735,1775`, `screens.rs:358,372`, `creation.rs:1488`; the buttons, `workspace/header.rs:239-296`; the glyph fonts, `theme.rs:298-299`.

**Acceptance Criteria:**
- [ ] Given the window on GNOME Wayland, when the pointer presses and drags any drag zone of the title band, then the window follows the pointer
- [ ] Given the title band, when it is double clicked, then the window maximizes, and a second double click restores it
- [ ] Given the title band, when it is right clicked, then the window menu of the desktop opens at the pointer
- [ ] Given the compositor allows minimize, maximize, and close, when the desktop's button layout names all three, then the three buttons show in that order, on the side the layout names, each 46 wide
- [ ] Given GNOME's default layout, which names close alone, when the window opens, then only the close button shows
- [ ] Given a button, when it is pressed, then the window minimizes, maximizes or restores, or closes, and each button carries the Button role and its label in the client's accessibility tree (that assistive tools read it on Linux is US-026's)
- [ ] Given Linux has no Segoe font, when the buttons draw, then their icons are SVG assets of the client, with the stroke of the other icons
- [ ] Given a control inside the band (search, a panel button), when it is pressed, then the control acts and the window does not start a move

#### US-003: The window has resize edges, a shadow, and rounded corners
**Description:** As a solo builder on Linux, I want to resize the window from its edges and see it stand out from what is behind so that it behaves and looks like a native window.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002

**References:**
- Note 2, note 4.
- Zed: `client_side_decorations`, `zed/crates/workspace/src/workspace.rs` lines 12656-12919; sizes, `zed/crates/theme/src/theme.rs` lines 53-57; a small example, `zed/crates/gpui/examples/window_shadow.rs`; the types, `zed/crates/gpui/src/platform.rs` lines 841-908.
- Crow Bot today: no call to `window_decorations`, `start_window_resize`, or `set_client_inset`; the geometry rules of `DESIGN.md` (concentric radii; no fill may reach a rounded corner).

**Acceptance Criteria:**
- [ ] Given the window asks for client decorations, when the pointer is over one of the 4 edges or 4 corners, then the cursor shows the matching resize shape, and a press and drag resizes the window from there
- [ ] Given a floating window, when it draws, then it has a shadow, a 1 px border, and rounded corners, and the margin of the shadow is given to the compositor with `set_client_inset` so that clicks in it reach the window behind
- [ ] Given a maximized or fullscreen window, when it draws, then it has no shadow, no rounding, and no resize zone
- [ ] Given a window tiled to an edge, when it draws, then the tiled edges have no shadow, no rounding, and no resize zone, and the free edges keep theirs
- [ ] Given the window is resized to its minimum of 640 by 420, when the drag goes on, then the window stays at the minimum
- [ ] Given the rule that no fill reaches a rounded corner, when the window draws at 100% and at 150% scale, then no fill of the frame, the rail, or the card shows outside the rounded outline
- [ ] Given the change, when the commit lands, then `DESIGN.md` holds the Linux window: the corner radius derived by the concentric rule from the card, the shadow, the border, the resize zone width, and the tiled states

#### US-004: Exactly one title bar when the compositor decorates
**Description:** As a solo builder on KDE or under Xwayland, I want one title bar so that the window does not show two sets of controls.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**References:**
- Note 3.
- Zed: the fallback to client mode, `zed/crates/gpui_linux/src/linux/wayland/window.rs` lines 2215-2234; client decorations on X11 only with a compositing manager, `zed/crates/gpui_linux/src/linux/x11/client.rs` lines 419-422 and `x11/window.rs` lines 1874-1880, 1950-1966.
- Measured: under Xwayland, Mutter gives the test window `_NET_FRAME_EXTENTS` of 37 at the top.

**Acceptance Criteria:**
- [ ] Given the client asks for client decorations under Xwayland on GNOME, when the window opens, then `_NET_FRAME_EXTENTS` reports 0 at the top and the client's band is the only title bar
- [ ] Given KDE on Wayland, when the window opens, then the client's band is the only title bar (that its buttons follow KDE's layout is checked by US-007)
- [ ] Given `window_decorations()` answers `Server` (an X11 session with no compositing manager), when the window draws, then the client hides its own buttons, shadow, rounding, and resize zones, and keeps the band's content
- [ ] Given the answer changes while the window is open, when the next frame draws, then the chrome follows the new answer with no restart

#### US-005: A start that cannot show a window says why
**Description:** As a solo builder on Linux, I want a start that fails to print its reason so that I can fix my computer instead of guessing.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**References:**
- Note 5.
- Zed: no adapter, `zed/crates/gpui_wgpu/src/wgpu_context.rs` line 604; software rendering accepted at start, `zed/crates/gpui_wgpu/src/wgpu_renderer.rs` lines 2195-2215; the panic on an unreachable display server, `zed/crates/gpui_linux/src/linux/platform.rs` lines 351-390; `gpu_specs`, `zed/crates/gpui/src/gpui.rs` lines 344-352.
- Crow Bot today: `main.rs:129-132` quits with no message.

**Acceptance Criteria:**
- [ ] Given neither `WAYLAND_DISPLAY` nor `DISPLAY` is set, when the client starts, then it prints one line that names the missing display and exits with a failure status, with no panic text
- [ ] Given `WAYLAND_DISPLAY` names a compositor that does not answer, when the client starts, then it prints one line that names it and exits with a failure status
- [ ] Given no GPU adapter can draw the window, when the client starts, then it prints one line that names the missing Vulkan or GL driver and the Fedora package that brings one, and exits with a failure status
- [ ] Given the only adapter renders on the CPU, when the window opens, then the client works and writes one line to its error stream that says rendering runs on the CPU

#### US-006: Text draws in every script
**Description:** As a solo builder on Linux, I want every character to draw with a font my system has so that no text shows as empty boxes.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**References:**
- Note 6.
- Zed: `.SystemUIFont` is "IBM Plex Sans" on Linux, `zed/crates/gpui_linux/src/linux/platform.rs` line 130; the fallback stack and the panic when no family exists, `zed/crates/gpui/src/text_system.rs` lines 261-273, 604-628.
- Crow Bot today: `theme.rs:290-299`; Inter embedded, `assets.rs:139-145`.

**Acceptance Criteria:**
- [ ] Given a Linux build, when the text fallbacks are read, then they name families that Fedora and Ubuntu install by default (Adwaita Sans, Noto Sans, Cantarell, DejaVu Sans) and no Segoe family
- [ ] Given a message with Latin, Arabic, Chinese, Japanese, and Korean text, when it draws on Fedora 45, then every character shows a glyph (emoji are US-026's)
- [ ] Given a code block, when it draws, then it uses a monospaced font that fontconfig resolves, and columns align
- [ ] Given a computer with none of the named families, when the client starts, then it starts and draws text with whatever font the system has, with no panic

---

### EP-002: Identity on the desktop

The desktop learns what the window is: a name, an icon, an entry in the application grid.

**Definition of Done:** The running window carries the application ID on Wayland and X11; with the desktop files installed, GNOME shows the application's name and icon in the dock and the overview; the desktop entry passes its validator.

#### US-008: The window carries the application ID
**Description:** As a solo builder on Linux, I want the window to state which application it belongs to so that my desktop groups it, names it, and gives it its icon.

**Priority:** P0
**Size:** XS (1 pt)
**Dependencies:** Blocked by US-001

**References:**
- Note 8.
- Zed: `app_id`, `zed/crates/gpui/src/platform.rs` lines 2712-2713; Wayland, `zed/crates/gpui_linux/src/linux/wayland/window.rs` lines 598-600; X11, `zed/crates/gpui_linux/src/linux/x11/window.rs` lines 1629-1647.
- Measured: the test window has no `WM_CLASS`.

**Acceptance Criteria:**
- [ ] Given the client source, when the application ID is searched, then `io.github.arthjean.CrowBot` appears in exactly one constant, and every other use reads it
- [ ] Given the window on X11, when `xprop` reads `WM_CLASS`, then instance and class are the application ID
- [ ] Given the window on Wayland, when the compositor reports its application ID, then it is the constant
- [ ] Given a desktop with no desktop entry of that name, when the window opens, then it opens with a generic icon and no failure

#### US-009: The desktop entry and the icon
**Description:** As a solo builder on Linux, I want Crow Bot in my application grid with its icon so that I start it like any application.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**References:**
- Note 9.
- Zed: `zed/crates/zed/resources/zed.desktop.in`; how its bundle places them, `zed/script/bundle-linux` lines 213-240.
- Specifications: Desktop Entry 1.5, Icon Theme 0.13 (links in `docs/linux-port.md`, section B).
- Crow Bot today: no such file; the mascot layers in `crates/crowbot-desktop/assets/mascot`.

**Acceptance Criteria:**
- [ ] Given the repository, when the Linux resources are listed, then it holds a desktop entry named after the application ID, and an icon as a scalable SVG and as PNG files of 48, 128, 256, and 512 px
- [ ] Given the desktop entry, when `desktop-file-validate` reads it, then it reports no error
- [ ] Given one documented command that copies the files under `~/.local/share` and points the entry at the built client, when the client runs, then GNOME's dock and overview show the name "Crow Bot" and the icon
- [ ] Given that command's `remove` action, when it runs, then every file it copied is gone
- [ ] Given the icon at 48 px, when it is shown on a light and on a dark background, then the crow's outline has a contrast of at least 3 to 1 against both
- [ ] Given a file names an application ID other than the constant, when the client's tests run, then a test fails and names the file

---

### EP-009: GPUI on Linux

Two things GPUI seemed not to do on Linux at the pinned rev, found by the review of EP-001 on 2026-11-17: it draws no COLRv1 color glyph, the format of Fedora's "Noto Color Emoji", and it published nothing on the AT-SPI bus, so assistive tools saw no GPUI window. Zed's main branch has the same text code (issues 58287, 61660, and 51171 closed with no fix; checked again on 2026-11-18 at `5b282ac`). The second was not a defect: GPUI publishes its window once `org.a11y.Status.IsEnabled` is true, which Orca sets when it starts, and the session of that review had it false (found in US-026, proven by `scripts/accessibility-check.sh`).

**Definition of Done:** On Fedora 45 an emoji draws in color; with AT-SPI on, the client's window and its buttons show to assistive tools with their roles and labels; the way GPUI is changed is recorded in `AGENTS.md`.

#### US-026: GPUI draws emoji and speaks to assistive tools on Linux
**Description:** As a solo builder on Linux, I want emoji drawn and the window readable by a screen reader so that Crow Bot is complete and accessible on my desktop.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-006

**References:**
- Zed: the rasterizer's sources, `zed/crates/gpui_wgpu/src/cosmic_text_system.rs` lines 532-539 (swash: COLRv0, bitmaps, outlines; no COLRv1); fallback families without a Latin `m` dropped, same file lines 423-432; the emoji font known by name only, line 1204; the AT-SPI adapter, `zed/crates/gpui_linux/src/linux/wayland/window.rs` lines 2282-2295.
- cosmic-text 0.19: its Unix fallback picks "Noto Color Emoji" by name, `src/font/fallback/unix.rs` line 46.
- Crow Bot: the oracles `theme::system_fonts::an_emoji_draws_a_glyph` (fails on Fedora 45 today) and `every_script_of_a_message_draws_a_glyph`.
- `AGENTS.md`, "GPUI source": GPUI comes from Zed's repository at a pinned rev; a change to GPUI is an exception this story decides.
- AccessKit's Unix adapter activates on `org.a11y.Status.IsEnabled` (`accesskit_unix` 0.22, `src/context.rs`); Orca sets it at start (`orca/orca.py`, `_ensure_accessibility_enabled`).

**Acceptance Criteria:**
- [ ] Given Fedora 45 and its COLRv1 "Noto Color Emoji", when a message with emoji draws, then each emoji shows in color, and `an_emoji_draws_a_glyph` passes (from US-006)
- [ ] Given a fallback family with no Latin glyph (Noto Sans Arabic, an emoji family), when the client names it, then the text system uses it in the order named
- [ ] Given AT-SPI is on, when the client runs, then its window shows on the accessibility bus, and each window button reads with the Button role and its label (from US-002)
- [ ] Given the way GPUI is changed (a patch upstream and a rev bump, or a fork the client takes by path), when the story ends, then `AGENTS.md` records it, and the change is offered to Zed
- [ ] Given Windows, when the client runs, then nothing changes

---

### EP-007: Validation on real desktops

The chrome, the identity, and the text, specified from source and proven on the reference computer's headless compositors, are checked by hand on real sessions. The epic follows EP-001 and EP-002, whose work it checks, so that neither waits on the other, and EP-009, whose emoji and AT-SPI bridge it checks too.

**Definition of Done:** Every check of US-007 holds on GNOME Wayland, KDE on Wayland, Xwayland, and Sway, with its captures kept; a failed check is a new story or a fixed defect; the "Not verified" list of `docs/linux-port.md` is current.

#### US-007: Validate assumption: the chrome and the interface hold on real desktops
**Description:** As the product owner, I want the client checked on real sessions so that what was specified from source is proven on screen.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-006, US-009, US-026

**References:**
- Note 7; the "Not verified" lists of `docs/linux-port.md`.
- Zed's documented variables, `zed/docs/src/linux.md` lines 139-201, 308-314.
- `scripts/accessibility-check.sh` (US-026): the tree the client publishes, on a headless compositor.

**Acceptance Criteria:**
- [ ] Given GNOME Wayland at 100%, 125%, and 150% scale, when the window is captured in the light and the dark theme, then each capture matches `DESIGN.md` and is kept with the story
- [ ] Given two monitors with different scales, when the window is dragged from one to the other, then it redraws at the new scale factor and its text is rasterized at that scale, not stretched
- [ ] Given an AZERTY layout, when each shortcut of the client is pressed, then it acts, and the composer takes accented characters and a dead-key sequence
- [ ] Given an input method (IBus) with Japanese, when text is composed in the composer, then the marked text shows in place and commits
- [ ] Given a sign-in page address, when the client opens it, then the default browser opens it
- [ ] Given the desktop's reduced motion setting is on, when the mascot and the transitions draw, then they follow the reduced motion rules of `DESIGN.md`
- [ ] Given KDE on Wayland and Xwayland on GNOME, when the window is captured, then one title bar shows and the buttons act
- [ ] Given KDE on Wayland, when the window opens, then its buttons follow the button layout set in KDE's settings (from US-004)
- [ ] Given a desktop with no portal (Sway), when the client starts, then it starts in the light theme and does not fail
- [ ] Given GNOME Wayland, when a message holds `👍 🐦‍⬛ ❤️ 1️⃣ 👍🏽 🇫🇷` and a plain `❤`, then each emoji shows in color in the transcript and the plain heart in the text color, and the capture is kept (from US-026)
- [ ] Given Orca running on GNOME Wayland, when focus moves through the window buttons, the bot list, and the composer, then Orca speaks each with its role and label (from US-026); libatspi's "AddAccessible with unknown signature" warnings are not a failure unless Orca misses a change
- [ ] Given a check fails, when the story ends, then the failure is a new story or a fixed defect, and the "Not verified" list of `docs/linux-port.md` is updated

---

### EP-003: The daemon on Linux

The daemon stops when asked, lives where a Linux user expects, keeps its files to its account, and outlives the window and the session.

**Definition of Done:** SIGTERM ends the daemon within 5 seconds and removes its socket; the default files sit in the XDG folders with modes 0700 and 0600; a socket path of 108 bytes is refused before any file exists; agents end within 1 second of the daemon's death; with the unit installed, the daemon survives the end of the program that started the client.

#### US-010: The daemon stops cleanly on a signal
**Description:** As a solo builder on Linux, I want the daemon to finish its work when it is told to stop so that a logout or a service stop leaves nothing half done.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- Note 11.
- Crow Bot today: tokio without `signal`, `crates/crowbotd/Cargo.toml:23`; `Store::recover` ends left-over turns at the next start, `main.rs:201`; a stopped turn answers its waiting requests as cancelled (`AGENTS.md`, "The daemon owns approvals").
- Measured: after SIGTERM the socket file and the `-wal` and `-shm` files remain.

**Acceptance Criteria:**
- [ ] Given a daemon with no running turn, when it receives SIGTERM, SIGINT, or SIGHUP, then it exits with status 0 within 1 second and its socket file is gone
- [ ] Given a daemon with a running turn, when it receives SIGTERM, then the turn is stored as stopped, its waiting approval requests are answered as cancelled, the agent processes end, and the daemon exits within 5 seconds
- [ ] Given a daemon that stopped on a signal, when the database is opened afterward, then no `-wal` content is pending
- [ ] Given an agent that ignores the stop, when 5 seconds pass, then the daemon ends it by force and still exits
- [ ] Given a second signal during the stop, when it arrives, then the daemon exits at once
- [ ] Given attached clients, when the daemon stops, then each client sees the connection close and shows the daemon as stopped

#### US-011: The files sit in the XDG folders
**Description:** As a solo builder on Linux, I want Crow Bot's files where Linux applications keep theirs so that backups, cleaners, and I find them.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- Note 13, note 16.
- Specification: XDG Base Directory 0.8 (section B of `docs/linux-port.md`).
- Crow Bot today: `main.rs:87-98`, `crates/crowbot-protocol/src/endpoint.rs:27-36`, the log `connection.rs:368-371`, the sandbox `scripts/sandbox.zsh`.

**Acceptance Criteria:**
- [ ] Given Linux and no flag, when the daemon starts, then the database, the runtimes, and the provider homes are under `$XDG_DATA_HOME/crowbot` (default `~/.local/share/crowbot`), and the socket and its lock are under `$XDG_RUNTIME_DIR/crowbot`
- [ ] Given Linux and no flag, when the client starts the daemon, then the log is under `$XDG_STATE_HOME/crowbot` (default `~/.local/state/crowbot`)
- [ ] Given the client and the daemon with no flag, when each computes the default endpoint, then both get the same path from one function of the protocol crate
- [ ] Given `XDG_RUNTIME_DIR` is unset, when the daemon starts, then it prints a warning and uses a folder of mode 0700 under the data folder for the socket
- [ ] Given an `XDG_*` variable holds a relative path, when the daemon starts, then it ignores that variable, as the specification says
- [ ] Given `--endpoint` and `--db` point elsewhere, when the client starts the daemon, then the log is written beside that database and nothing is created in the default folders
- [ ] Given `scripts/sandbox.zsh`, when a sandbox runs and is cleaned, then no file exists outside the sandbox folder
- [ ] Given the change, when the commit lands, then `README.md` and `AGENTS.md` state the Linux folders, and the bots folder stays `~/CrowBot/Bots`

#### US-012: A socket path that is too long is refused first
**Description:** As a solo builder with a long home path, I want the daemon to refuse an endpoint it cannot use before it creates anything so that I get a clear reason and no stray database.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-011

**References:**
- Note 14.
- Kernel: `sun_path` holds 108 bytes (unix(7)).
- Measured: at 108 bytes the daemon exits 1 with `path must be shorter than SUN_LEN`, after it created the database.
- Crow Bot today: the exit statuses `EXIT_DATABASE_HELD` and `EXIT_ENDPOINT_HELD`, `crates/crowbot-protocol/src/lib.rs:41-45`; the start order of `docs/daemon-startup.md`.

**Acceptance Criteria:**
- [ ] Given an endpoint path of 108 bytes or more, when the daemon starts, then it exits before it creates the database, the lock, or any folder
- [ ] Given that refusal, when the message is read, then it gives the path, its length, and the limit of 107 bytes, and the exit status is a named constant of the protocol crate
- [ ] Given a path of 107 bytes, when the daemon starts, then it listens
- [ ] Given the client started that daemon, when it reads the exit status, then the window states the reason and names `--endpoint`

#### US-013: The daemon's files are private
**Description:** As a solo builder on a shared computer, I want only my account to read my conversations so that an explicit `--db` is as safe as the default.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-011

**References:**
- Note 15.
- Crow Bot today: `instance.rs:69` uses `create_dir_all`; only `~/.crowbot` is forced private, `main.rs:125-160`; the client's log, `connection.rs:102-115`.
- Measured: with `--db`, the folder is 0755 and the database, its lock, and its owner file are 0644.

**Acceptance Criteria:**
- [ ] Given a umask of 022 and an explicit `--db` in a folder that does not exist, when the daemon starts, then the folder it creates is 0700
- [ ] Given any way of starting the daemon, when its files are listed, then the database, its `-wal` and `-shm` files, the lock, the owner file, and the log are 0600
- [ ] Given a provider home or a runtimes folder the daemon creates, when it is listed, then it is 0700
- [ ] Given an existing folder that the user made 0755 on purpose, when the daemon starts with `--db` inside it, then the folder's mode is left alone and the files inside are still 0600
- [ ] Given a database folder owned by another account, when the daemon starts, then it refuses with a message that names the folder and its owner

#### US-014: Agents end at once when the daemon dies
**Description:** As a solo builder, I want no agent to keep running after its daemon is gone so that nothing acts on my computer with no approval path.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- Note 17.
- Crow Bot today: the guard, `lifetime.rs:120-160`; its test, `tests/daemon/recovery.rs:117`.
- Kernel: `PR_SET_PDEATHSIG` follows the thread that created the process (man page link in section C of `docs/linux-port.md`).

**Acceptance Criteria:**
- [ ] Given a running agent, when the daemon is killed with SIGKILL, then every process of the agent's group is gone within 1 second
- [ ] Given the daemon dies between starting the guard and the guard's first check, when the guard runs, then it ends the agent and exits
- [ ] Given an idle guard, when 10 seconds pass, then it has not woken on a timer (it waits on the daemon's end and the agent's end)
- [ ] Given the agent exits by itself, when the guard notices, then the guard exits with the agent's status
- [ ] Given an agent that left its process group, when the daemon dies under the systemd unit of US-015, then that process ends with the unit

#### US-015: The daemon runs as a systemd user unit
**Description:** As a solo builder on Linux, I want the daemon supervised by my session's service manager so that it survives the window, restarts after a crash, and can run with no session.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009, US-010, US-011

**References:**
- Note 12.
- systemd: `systemd.service` (`Type=notify`, `Restart=`), `systemd.kill` (`KillMode=`), `loginctl` (lingering); links in section C of `docs/linux-port.md`.
- Comparable products: Syncthing and Podman install user units with the package and document lingering.
- Crow Bot today: the detached start, `connection.rs:56-77,117-133`; the start order, `docs/daemon-startup.md`.
- Measured: the daemon the client started sits in the scope of the program that launched the client.

**Acceptance Criteria:**
- [ ] Given the repository, when the Linux resources are listed, then it holds a user unit that starts `crowbotd`, of `Type=notify`, that restarts on failure, with a stop timeout above the 5 seconds of US-010
- [ ] Given a build from source, when the documented command of US-009 runs, then it also copies the unit under `~/.config/systemd/user` with the path of the built daemon, and its `remove` action stops and removes it
- [ ] Given the unit, when the daemon has opened and recovered its database and claimed its endpoint, then it sends `READY=1`, and not before
- [ ] Given the unit is installed and no daemon listens, when the client starts, then it asks the user's systemd to start the unit and attaches, and the daemon's control group is the unit's, not the client's
- [ ] Given the unit is not installed, or the computer has no systemd, when the client starts, then it starts the daemon detached in a new session and attaches
- [ ] Given the client passes `--endpoint`, `--daemon`, or daemon arguments, when it starts a daemon, then it uses the detached start, so that a sandbox never touches the unit
- [ ] Given the daemon under the unit is killed with SIGKILL, when 5 seconds pass, then systemd has started it again and the client attaches again
- [ ] Given the unit fails to start, when the client waits, then the window states the reason from the exit status and offers to retry, as it does today
- [ ] Given the daemon runs under the unit, when the user closes the client and the program that launched it ends, then the daemon keeps running
- [ ] Given lingering is off, when the settings show the daemon, then one line states that bots stop at logout and gives the command that enables lingering; the client never enables it itself
- [ ] Given the daemon links no GPL crate, when the readiness message is added, then it uses no library under the GPL

---

### EP-004: Tests and continuous integration

The Linux build is guarded: the tests pass in any environment, the Unix transport has tests of its own, and every push builds, tests, and starts a real window.

**Definition of Done:** The six gates pass as root, with a 100-character `TMPDIR`, and 20 times in a row at 24 runs in parallel; every push runs them on Fedora and Ubuntu, x86_64 and arm64, in under 20 minutes; a build with no display backend fails the pipeline.

#### US-016: The tests pass in any environment
**Description:** As the product owner, I want tests that depend on nothing but the code so that a failure always means a defect.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- Notes 22, 23, 25.
- Crow Bot today: the unordered `tokio::select!`, `provider/acp.rs:481`; the fake agent, `examples/fake_acp_agent.rs:107,855-871`; the socket of the tests, `tests/daemon/support.rs:29-34` and `crates/crowbot-protocol/src/transport.rs:221`; the root case, `tests/daemon/runtimes.rs:392-407`; the leaks, `runtime.rs:606`, `tests/daemon/startup.rs:113`, `connection.rs:724-730`; the nested build, `connection.rs:471-491`; `.gitignore:1`.
- Measured: 8 failures in 240 parallel runs; 109 failures with a 106-character `TMPDIR`; 1 failure as root.

**Acceptance Criteria:**
- [ ] Given 24 runs of the daemon tests in parallel, repeated 20 times, when they end, then none failed
- [ ] Given a Grok sign-in whose page and whose end are ready at the same moment, when the daemon handles them, then the clients still receive the page before the end
- [ ] Given `TMPDIR` is a path of 100 characters, when both test suites run, then they pass
- [ ] Given the tests run as root, when they end, then they pass or each skipped test names root as its reason
- [ ] Given a full run of both suites, when `/tmp` is compared before and after, then no file or folder of the tests remains
- [ ] Given the client tests build the daemon, when they run Cargo, then they pass `--locked`
- [ ] Given both workspaces were built, when `git status` runs, then no build folder shows as untracked

#### US-017: The Unix transport has tests of its own
**Description:** As the product owner, I want the rule "the endpoint belongs to one user" tested on Unix as it is on Windows so that a change cannot open the socket unnoticed.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**References:**
- Note 24.
- Crow Bot today: `crates/crowbot-protocol/src/transport/unix.rs` (93 lines, no test); the 7 Windows tests, `transport/windows.rs:359-421`; the Unix tests of the daemon, `tests/daemon/startup.rs:150-212`.

**Acceptance Criteria:**
- [ ] Given a listener, when its socket and its lock are listed, then both are 0600 and a folder the listener created is 0700
- [ ] Given a peer whose user ID differs from the daemon's, when it connects, then the daemon drops the connection before it reads anything
- [ ] Given a client, when the daemon on the other end runs as another user ID, then the client closes the connection before it sends anything
- [ ] Given the two tests above, when they run as an ordinary user with no second account, then they still run (the peer's identity is supplied to the check, or a user namespace gives a second ID)
- [ ] Given a stale socket file and no holder of its lock, when a daemon starts, then it replaces the file and listens
- [ ] Given two daemons that start at once on one socket path, when both finish starting, then exactly one listens and the other exits with `EXIT_ENDPOINT_HELD`

#### US-018: Every push is built and tested on Linux
**Description:** As the product owner, I want every push checked on the systems the product targets so that a Linux regression is caught the day it is written.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**References:**
- Note 26.
- GitHub: hosted runner labels and container jobs (section D of the distribution research in `docs/linux-port.md`).
- Crow Bot today: no workflow file.

**Acceptance Criteria:**
- [ ] Given a push or a pull request, when the pipeline runs, then the six quality gates run in a Fedora container and on Ubuntu, each on x86_64 and on arm64
- [ ] Given the pipeline, when it runs Cargo, then it passes `--locked` for both workspaces, and a lockfile that is out of date fails the job
- [ ] Given a gate fails on one system, when the pipeline ends, then it is red and names the system and the gate
- [ ] Given a run with a warm cache, when it ends, then it took under 20 minutes
- [ ] Given the pipeline, when its steps are read, then none signs in to a provider, downloads a provider runtime, or holds a credential

#### US-019: A real window opens in the pipeline
**Description:** As the product owner, I want the pipeline to start the real client on a real compositor so that a build that cannot show a window never passes again.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-018

**References:**
- Note 26.
- Measured: the windowed client started and stayed alive under `mutter --headless --wayland --no-x11 --virtual-monitor 1280x800` in a private D-Bus session, with `VK_DRIVER_FILES` set to lavapipe's driver file.
- Mesa: `VK_DRIVER_FILES` (docs.mesa3d.org/envvars.html). Weston's headless backend as the alternative.

**Acceptance Criteria:**
- [ ] Given the repository, when the window test script runs on a developer's computer, then it starts a headless compositor, a throwaway daemon, and the client, and passes when the client is attached and alive after 5 seconds
- [ ] Given a client built with no display backend, when the script runs, then it fails and says that no window opened
- [ ] Given the script, when it ends, pass or fail, then the compositor, the daemon, and the client are stopped and no file remains outside its work folder
- [ ] Given the script runs while a desktop session is open, when it runs, then it uses a private D-Bus session and a compositor socket of its own, and no window appears on the user's screen
- [ ] Given the Fedora job of the pipeline, when it runs the script with CPU rendering, then it passes; if a container cannot run the compositor, the story records why and runs the script on the hosted Ubuntu system instead
- [ ] Given the X11 backend, when the script runs with an X server with no screen, then the client also opens a window there

---

### EP-005: Providers on Linux

The three providers install, sign in, and run a turn on Linux, with every gated tool call reaching the daemon.

**Definition of Done:** On Fedora 45 each of Claude, ChatGPT, and Grok signs in from the client and completes a turn whose declined write is not written; `docs/provider-sign-in.md` records the Linux runs.

#### US-020: Each provider signs in on Linux
**Description:** As a solo builder on Linux, I want to sign in to each provider from Crow Bot so that my bots run on my own plans.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

**References:**
- Note 18.
- Crow Bot today: `docs/provider-sign-in.md:125` lists Linux as not run; the sign-in's process group, `daemon/providers.rs:823-873,935-941`; the profiles, `provider/claude.rs`, `codex.rs`, `grok.rs`.
- Upstream: how each agent opens the browser and where it keeps its sign-in (section A2 of the distribution research).

**Acceptance Criteria:**
- [ ] Given Fedora 45 and a daemon under the systemd unit, when the user starts the sign-in of Claude, of ChatGPT, and of Grok from the client, then the provider's page opens in the default browser and the provider shows as signed in afterward
- [ ] Given a sign-in is cancelled or times out, when the daemon ends its process group, then a browser that was already running is still running, and a browser the sign-in started keeps its window
- [ ] Given Codex and Grok, when their homes are listed after a sign-in, then the sign-in files are under the daemon's provider homes and 0600, and the daemon read none of them
- [ ] Given no browser can be opened (no display variable), when a sign-in starts, then the client shows the address to open, or states that this provider cannot sign in here
- [ ] Given the three runtimes installed by the daemon, when their programs are listed, then each has its execute bit, including the ones unpacked from zip
- [ ] Given the runs above, when the story ends, then `docs/provider-sign-in.md` records them with the versions, and its outdated lines on `bunx` are corrected

#### US-021: Each provider runs a turn with the daemon's approvals
**Description:** As a solo builder on Linux, I want every gated tool call to reach me so that a bot on Linux is as controlled as on Windows.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-020

**References:**
- Note 19.
- `AGENTS.md`, "The daemon owns approvals".
- Crow Bot today: the ignored real-turn tests, `tests/daemon/claude.rs:13,53` and `tests/daemon/providers.rs:998`.
- Upstream: Codex isolates commands with bubblewrap and needs user namespaces (section A2 of the distribution research).

**Acceptance Criteria:**
- [ ] Given a bot on each provider on Fedora 45, when it is asked to write a file and the user declines, then the file does not exist and the turn records the declined request
- [ ] Given the same request, when the user allows it once, then the file exists and the next write asks again
- [ ] Given a bot on Codex, when it runs a shell command, then the command runs inside Codex's sandbox, with the `bwrap` the runtime brings or the system's
- [ ] Given user namespaces are disabled on the computer, when a bot on Codex runs a command, then the turn fails with a message that names the cause
- [ ] Given a bot on Full access, when a gated call arrives, then the daemon allows it once and records why, as on Windows
- [ ] Given the user's own `~/.claude`, `~/.codex`, and `~/.grok` settings allow everything, when a bot runs, then every gated call still asks

#### US-022: Small Linux corrections of the daemon
**Description:** As a solo builder on Linux, I want the daemon's small Unix gaps closed so that its messages are right and its tools behave as expected.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**References:**
- Note 21.
- Crow Bot today: `program_exists`, `provider/acp.rs:154-179`; the usage path, `main.rs:80`; the pins, `runtime/pins.rs`.
- Upstream: a `claude-agent-acp` release can have no archives (v0.107.3).

**Acceptance Criteria:**
- [ ] Given a file with no execute bit set as a provider's executable, when the daemon checks it, then it reports the program as not found and the message names the missing permission
- [ ] Given `crowbotd --version` and `crowbot --version`, when either runs, then it prints the version on standard output and exits 0
- [ ] Given `crowbotd --help`, when it runs, then it prints the usage on standard output and exits 0, and a wrong argument still exits 2 on the error stream
- [ ] Given the data folder is on a file system mounted `noexec`, when a runtime is started, then the turn fails with a message that names the mount option
- [ ] Given the ignored pin test, when it runs with the network, then it fails for any pinned address that does not answer or whose SHA-256 differs, on every platform

---

### EP-008: Windows non-regression

The port changes code that Windows also runs. This epic proves on Windows what the Linux stories cannot prove on Linux: the client and the daemon behave as before, and every push runs the gates there. It comes last, so that no Linux story waits for a Windows computer.

**Definition of Done:** On a Windows computer the client's band, buttons, window options, and fonts are those of today, and the daemon's stop, paths, files, and job object are those of today; the one intended change (a window that cannot open says why) holds; every push runs the six gates on Windows.

#### US-023: The Windows client is unchanged
**Description:** As the product owner, I want the client checked on Windows after the port so that a Linux change never breaks the build that already works.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002, US-003, US-004, US-005, US-006, US-008

**References:**
- The Windows title band and its caption buttons, `workspace/header.rs` (`render_caption_buttons`); the Linux branches, `chrome.rs`; the font stacks, `theme.rs` (`font`).
- FR-05 and FR-16.

**Acceptance Criteria:**
- [ ] Given Windows, when the client runs, then the band and its buttons are unchanged (from US-002)
- [ ] Given Windows, when the client runs, then the window options are unchanged (from US-008)
- [ ] Given Windows, when text and code draw, then the font stacks are the ones of today
- [ ] Given Windows and a window that cannot open, when the client starts, then it prints one line that names the reason and exits with a failure status, the one change FR-05 asks of every system
- [ ] Given the client workspace on Windows, when the six quality gates run, then they pass

#### US-024: The Windows daemon is unchanged
**Description:** As the product owner, I want the daemon checked on Windows after the port so that its stop, its folders, its files, and its job object stay as certified.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-010, US-011, US-013, US-014, US-022

**References:**
- The Windows branches of `lifetime.rs`, `daemon/providers.rs`, and `connection.rs`; `crates/crowbot-protocol/src/transport/windows.rs` (Files NOT to Modify).
- FR-16.

**Acceptance Criteria:**
- [ ] Given Windows, when the daemon runs, then its behavior is unchanged (from US-010)
- [ ] Given Windows or macOS, when the daemon starts with no flag, then its paths are the ones of today (from US-011)
- [ ] Given Windows, when the daemon runs, then nothing changes (from US-013)
- [ ] Given Windows, when the daemon runs, then the job object path is unchanged (from US-014)
- [ ] Given the root workspace on Windows, when the six quality gates run, then they pass

#### US-025: Every push is built and tested on Windows
**Description:** As the product owner, I want the pipeline to run on Windows too so that a Linux change that breaks Windows is seen the day it is written.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-018, US-023, US-024

**References:**
- Note 26.
- GitHub: hosted runner labels (section D of the distribution research in `docs/linux-port.md`).

**Acceptance Criteria:**
- [ ] Given the Windows build, when the pipeline runs, then the six gates also run on Windows, so that a Linux change that breaks Windows is seen (from US-018)
- [ ] Given the Windows job, when it runs Cargo, then it passes `--locked` for both workspaces
- [ ] Given a gate fails on Windows, when the pipeline ends, then it is red and names Windows and the gate

## Functional Requirements

- FR-01: On Linux the client must open a window on a Wayland compositor, and on an X11 server when no Wayland compositor is named.
- FR-02: On Linux the client must draw the window's title band, buttons, resize zones, shadow, border, and rounded corners, and must hide its buttons, shadow, and rounding when the compositor decorates.
- FR-03: The client must show only the window buttons the compositor allows, in the order and on the side of the desktop's layout.
- FR-04: The client must NOT show two title bars on any desktop.
- FR-05: When no window can open, the client must print the reason and exit with a failure status.
- FR-06: The window must carry the application ID, read from one constant.
- FR-07: On a stop signal the daemon must end its turns, answer waiting approvals as cancelled, end its agents, remove its socket, and exit.
- FR-08: On Linux the daemon and the client must use the XDG folders by default, and must compute the default endpoint with one shared function.
- FR-09: The daemon must refuse an endpoint path it cannot bind before it creates any file.
- FR-10: The daemon must create its folders with mode 0700 and its files with mode 0600, whatever the umask.
- FR-11: The agents of a daemon must end within 1 second of the daemon's death, however it dies.
- FR-12: When its unit is installed, the client must start the daemon through the user's systemd; otherwise it must start it detached in a new session.
- FR-13: The client must NOT enable lingering, install a unit, or change the user's systemd settings by itself; only the documented command the user runs installs the unit.
- FR-14: The daemon must NOT read, copy, or store a provider credential on Linux, in any sign-in path.
- FR-15: The pipeline must fail when the client cannot open a window on a headless compositor.
- FR-16: The Windows behavior of the daemon and the client must NOT change.

## Non-Functional Requirements

- **Performance:** The daemon exits within 1 second of a stop signal when idle and within 5 seconds with a running turn. The window test passes within 30 seconds on a hosted runner.
- **Security:** Folders 0700 and files 0600 for every file the daemon or the client creates. A peer of another user ID is dropped before 1 byte is read. 0 provider credentials read by the daemon.
- **Accessibility:** Each of the 3 window buttons has a role and a label. Text contrast of the chrome meets the 4.5 to 1 ratio that `DESIGN.md` sets. Reduced motion is honored on Linux.
- **Scalability:** Both workspaces build and pass their gates on 2 distributions (Fedora 45 and Ubuntu 24.04) and 2 architectures (x86_64 and arm64). 10 bots run in parallel on one Linux daemon with no failed turn caused by the daemon.
- **Reliability:** 0 failures in 20 repetitions of 24 parallel test runs. Under its unit, a killed daemon is running again within 5 seconds. Agents end within 1 second of the daemon's death. The pipeline ends in under 20 minutes with a warm cache.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | No display | Neither `WAYLAND_DISPLAY` nor `DISPLAY` is set | The client exits with a failure status | "Crow Bot found no display: neither WAYLAND_DISPLAY nor DISPLAY is set." |
| 2 | No GPU driver | No Vulkan or GL adapter can draw | The client exits with a failure status | "Crow Bot found no graphics driver. On Fedora, install mesa-vulkan-drivers." |
| 3 | CPU rendering | Only lavapipe or llvmpipe is present | The window opens and works | One line on the error stream: "Rendering runs on the CPU." |
| 4 | Compositor decorates | X11 with no compositing manager | The client hides its buttons, shadow, and rounding | None |
| 5 | Tiled or maximized window | The user snaps the window to an edge | No shadow, rounding, or resize zone on the tiled edges | None |
| 6 | No portal | Sway or another minimal desktop | Light theme, default button layout, no failure | None |
| 7 | Socket path too long | A home path over about 84 characters with no runtime folder | The daemon exits before it creates a file | "The endpoint path is N bytes; the limit is 107. Pass --endpoint with a shorter path." |
| 8 | No runtime folder | `XDG_RUNTIME_DIR` is unset (a bare SSH session) | The socket goes under the data folder, mode 0700 | A warning line in the log |
| 9 | Stop during a turn | SIGTERM while a bot works | The turn is stored as stopped, approvals are cancelled | The conversation shows the turn as stopped |
| 10 | Daemon killed | SIGKILL, power loss | Agents end within 1 second; the unit restarts the daemon; `Store::recover` ends the turn | The conversation shows the turn as interrupted |
| 11 | Logout without lingering | The user's last session ends | The daemon stops cleanly; it starts again at the next login | Settings: "Bots stop when you log out. To keep them working: loginctl enable-linger" |
| 12 | No systemd | A distribution with another init system, or a container | The client starts the daemon detached in a new session | None |
| 13 | `noexec` data folder | A hardened home | The turn fails | "The folder of the runtimes does not allow running programs (noexec)." |
| 14 | User namespaces off | A hardened kernel or a container | A Codex command fails | "ChatGPT's sandbox needs user namespaces, which this computer disables." |
| 15 | Sign-in with no browser | A daemon with no display variable | The address is shown to open by hand, or the provider says it cannot sign in here | "Open this address to sign in: ..." |


## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | The chrome specified from source behaves differently on screen (no picture exists yet) | Med | High | US-007 checks real sessions; US-002 and US-003 require screenshots through `/agent-desktop`, which needs the user's consent in the portal |
| 2 | A headless compositor does not run in a container job | Med | Med | US-019 falls back to the hosted Ubuntu system, and records why |
| 3 | Killing the sign-in's process group closes the user's browser | Med | Med | US-020 tests it on Linux |
| 4 | A Linux change breaks Windows | Low | High | Every change sits behind a Linux or Unix condition; EP-008 checks Windows by hand and runs the gates there on every push |
| 5 | The NVIDIA proprietary driver on Wayland misbehaves on other computers | Med | Med | The reference computer has it and the client started; US-005 gives a clear failure; US-007 records what is seen |
| 6 | The pinned GPUI rev has a Linux defect that needs a newer rev | Low | Med | The rev changes only on purpose, in its own commit, with Zed's patch entries copied |
| 7 | The systemd unit lacks the display variables on a desktop other than GNOME | Med | Low | US-020 shows the address when no browser opens; US-007 covers KDE |
| 8 | 26 stories is a large scope | Med | Med | Three phases; Phase A alone gives a working Fedora build from source, and no Linux story waits for Windows |
| 9 | The application ID changes later | Low | Low | One constant; a test ties every file to it; nothing is released by this PRD |

## Non-Goals

What this version explicitly does NOT include:

- Distribution of any kind: release builds, a glibc floor, a static musl daemon, an archive and install script, an RPM and a dnf repository, a `.deb`, a Flatpak, an AppImage, self-update, an AppStream metainfo file, and aarch64 release artifacts. The product owner decided on 2026-11-16 that distribution waits until the product works as intended on its target systems. `docs/linux-port.md` keeps the research (notes 28 to 32) for that PRD.
- Runtimes for a computer whose C library is musl (note 20). It matters only once a static daemon exists, so it moves with distribution.
- Other distributions than Fedora as a target. Ubuntu runs the tests in the pipeline; nothing else is promised.
- A single instance of the client. Two clients on one daemon work; a second launch opens a second window.
- Sign-in on a server with no display, and the remote transport. Claude's route there makes the caller hold a token, which the hard rules forbid; both wait for the servers PRD.
- macOS. Its Unix endpoint stays unverified, as `AGENTS.md` records.
- A tray icon, start at login beyond the unit, and desktop notifications.
- A change of the GPUI rev, of the protocol version, or of the database schema.
- A daemon-owned `CLAUDE_CONFIG_DIR`. It stays an open decision of `AGENTS.md`.

## Files NOT to Modify

- `crates/crowbot-desktop/LICENSE`, `crates/crowbot-protocol/LICENSE`, `crates/crowbotd/LICENSE.md`, `crates/crowbotd/NOTICE`: the license texts and the required notice.
- The GPUI rev and the `[patch.crates-io]` entries in `crates/crowbot-desktop/Cargo.toml`: they change only together, when the rev is bumped on purpose. US-001 adds features to the two GPUI dependencies and touches nothing else there.
- `crates/crowbot-protocol/src/transport/windows.rs` and the Windows branches of `lifetime.rs`, `daemon/providers.rs`, and `connection.rs`: the Windows behavior is done and certified.
- The schema text in `crates/crowbotd/src/store.rs` and the variants and field names of `Event` in `crates/crowbot-protocol/src/event.rs`: this PRD changes neither.
- The existing values of `EXIT_DATABASE_HELD` and `EXIT_ENDPOINT_HELD` in `crates/crowbot-protocol/src/lib.rs`: clients read them; a new status gets a new number.
- The pins and the choice of platform in `crates/crowbotd/src/runtime/pins.rs`: this PRD changes neither. US-022 only adds a test that checks them.
- `docs/product.md`: product decisions change with the product owner, not inside a story.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** Should the Linux chrome be one wrapper element around the workspace, as Zed's `client_side_decorations`, or logic spread over the band's elements? Recommended: one wrapper for the frame (shadow, border, rounding, resize zones) and one shared handler for the band's drag zones. Engineering to confirm that the wrapper does not break the rule that no fill reaches a rounded corner.
- **Features per target:** Can `gpui_platform`'s `wayland` and `x11` features be enabled for Linux only through a target-specific dependency table, given that features unify across a dependency graph? Recommended: a `[target.'cfg(target_os = "linux")'.dependencies]` entry; engineering to check with `cargo tree` on Windows.
- **Window radius:** Which corner radius does the Linux window take? Recommended: the concentric rule from the card (radius 14, inset 4), which gives 18; Zed uses 10. `DESIGN.md` records the choice.
- **Stop sequence:** Should the daemon stop turns through the same path as a user's stop request? Recommended: yes, so that a stopped turn has one shape in the database. Trade-off: the stop then waits on agents, bounded by the 5 seconds.
- **Exit status:** Should an unusable endpoint path get a new exit status in the protocol crate (5), or reuse status 2? Recommended: a new named status, so that the client states the reason as it does for 3 and 4.
- **Guard:** A pipe whose write end the daemon holds, or a `pidfd` of the daemon polled with `poll(2)`? Recommended: the pipe, which is portable Unix; `pidfd` is Linux only. Engineering to confirm that the write end is not inherited by agents.
- **Readiness:** Send `READY=1` with a few lines of code over `$NOTIFY_SOCKET`, or take a crate (`sd-notify` 0.5.0, `libsystemd` 0.7.2)? Recommended: the few lines, which add no dependency and no license question. Engineering to confirm the abstract socket case.
- **Installing the unit:** Should the command that installs the desktop entry and the unit be a script of the repository (`scripts/`) or an action of `scripts/sandbox.zsh`? Recommended: its own script, since the sandbox must never touch the unit.
- **Starting the unit:** Should the client call `systemctl --user start` or speak D-Bus to the user's systemd? Recommended: `systemctl`, which needs no D-Bus crate in the client; the answer comes from the endpoint, as today.
- **Dependencies:** The 44 packages that the `wayland` and `x11` features add to the client's lockfile. No new crate is expected in the daemon.
- **Migration:** None. No Linux user has data. Backward compatibility: yes for Windows and macOS, whose paths do not change. Rollback plan: each story is behind a Linux or Unix condition and reverts alone.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Stories certified `DONE` | 0 of 26 | 20 | Month-1 | `tasks/prd-crow-bot-linux-port-status.json` |
| Stories certified `DONE` | 0 of 26 | 26 | Month-6 | `tasks/prd-crow-bot-linux-port-status.json` |
| Window actions that work with the pointer on GNOME Wayland | 0 of 5 | 5 of 5 | Month-1 | The checks of US-002 and US-003 on the reference computer |
| Title bars shown under Xwayland | 2 | 1 | Month-1 | `xprop _NET_FRAME_EXTENTS` and a screenshot |
| Time from SIGTERM to exit, idle daemon | Immediate, with the socket left behind | Under 1 second, socket removed | Month-1 | The test of US-010 |
| Files of the daemon readable by other accounts with `--db` | 3 (database, lock, owner) | 0 | Month-1 | The test of US-013 |
| Test failures in 20 repetitions of 24 parallel runs | About 8 in 240 runs | 0 | Month-1 | The loop of US-016 |
| Systems that check every push | 0 | 4 (Fedora and Ubuntu on two architectures), then 5 with Windows (EP-008) | Month-1, Month-6 | The pipeline's job list |
| Providers that run a turn on Linux | 0 of 3 | 3 of 3 | Month-6 | The checks of US-021 |


## Open Questions

- Which image is the application icon? The product owner decides before US-009: the crow mascot's head in the brand color is the default, and it must read at 48 px.
- Is `io.github.arthjean.CrowBot` the final application ID? The product owner confirms in the distribution PRD, the first work that puts it on other people's computers.
- Should the detached start remain the default when the unit is installed but the user prefers no service? Engineering proposes inside US-015; a setting is out of scope unless the answer is yes.
- Does KDE's systemd user manager hold the display variables? Engineering answers in US-020; it decides whether a sign-in under the unit can open a browser there.
- Should the bots folder move from `~/CrowBot/Bots` to a localized Documents folder on Linux? The product owner decides with the servers PRD; US-011 keeps it.
[/PRD]
