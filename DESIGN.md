# Crow Bot Design System

## 1. Contract

### 1.1 Purpose

`DESIGN.md` is the design contract of the Crow Bot desktop client
(`crates/crowbot-desktop`, GPUI): the rail, the sidebar, the conversation,
the creation form, the bot panel, Settings, menus, pickers, dialogs, and
toasts. It records the thesis, the tokens, the geometry, and the component
contracts that the code implements, so that a change to a surface does not
re-derive the system from the render code.

### 1.2 Authority

| Source | Owns |
| --- | --- |
| `DESIGN.md` | Thesis, geometry rules, component contracts, the delivery gate |
| `AGENTS.md` | Architecture, hard rules, engineering gates |
| `src/theme.rs` | Every color, size, radius, duration, and opacity, in both themes |
| `src/ui.rs` | The shared primitives: icons, buttons, tooltips, avatars, the menu surface and rows, the text field skin |
| `src/mascot.rs` | The mascot of a bot: its layers and what its eyes show |
| `src/identity.rs` | The application ID and name, and the guards of the desktop entry and the icon |
| `src/workspace/forms.rs` | Form fields, labels, notes, choice chips, provider notices |

When sources disagree, product intent wins, then a rule here, then the shared
primitives, then local render code. A visual change that contradicts this
document updates it in the same commit.

The view files hold no literal color, size, opacity, or duration: the test
`the_views_hold_no_literal_color_size_or_duration` in `src/theme.rs` fails
on one. A new value is a token in `theme.rs` first. A new view file joins the
`VIEWS` list of that test.

### 1.3 Words

`MUST` is required. `SHOULD` is the default and needs a stated reason to
diverge. `MAY` is optional.

## 2. Thesis

### 2.1 What Crow Bot is on screen

Crow Bot is a desk of AI teammates. The dominant idea of the window is the
conversation with one bot: its messages, its steps, its approvals, and the
composer. Everything else frames it: a rail of app-wide controls at the far
left, the list of bots, a title band that is mostly empty, and a bot panel
that opens only when its settings are needed.

### 2.2 Lineage

| Influence | What Crow Bot took | Limit |
| --- | --- | --- |
| OpenBot | The shell measurements: the 52 px rail, the 280 px sidebar with 62 px rows and 46 px avatars, the inset card of radius 14, the creation form column of 420 px, the palette of dark and light tokens | Ideas and numbers only. Its license is noncommercial: never copy or translate its code or text |
| Paneflow | The component contracts: the menu and picker surfaces, concentric radii, the text field skin and focus halo, the select trigger lift, the menu reveal, the rule that GPUI clips to rectangles | Paneflow is GPL-3.0, like this client: its code MAY be adapted here, never into the daemon or the protocol crate |
| T3 Code | The anatomy of the search palette (5.11): a field with no edge, groups of rows with a lead column, a title beside its context, a time at the trailing edge, a tinted row of key hints, a veil over the window; and the stroke of the icons (4.6) | T3 Code is MIT. Its structure and measures only: the colors, the radii, and the shadow stay the tokens of this document |

### 2.3 Three words

**Quiet.** The chrome recedes. Neutrals carry no hue. The accent marks focus,
selection, links, and the one primary action, never a large surface. Hover
and selection are washes of the text color, not colored fills.

**Concentric.** Every rounded surface takes its radius from 4.3, and a
surface inside another keeps the outer curve: outer radius equals inner
radius plus the padding between them. No inner surface paints a square
corner inside a rounded one.

**Honest.** What the chrome shows is real state: the ring around a provider
is its state from the daemon, a badge on a bot is its state, a model name is
the one the bot runs on. No decoration stands in for data.

## 3. Anatomy

```text
┌rail┬ sidebar topbar ─┬ header: avatar, name, title · model, history, new chat, panel ┬ caption ┐
│ ⌂  │ ▤               │                                                               │ _ ☐ ×   │
│    ├─────────────────┴───────────────────────────────────────────────────────────────┴─────────┤
│    │ card: inset 4 right and bottom, radius 14, canvas fill, hairline edge                   │
│    │ ┌ sidebar 280 ──┐ ┌ conversation ──────────────────────────────┐ ┌ bot panel 360 ┐     │
│    │ │ (🔍)       (+) │ │ transcript, column 760                     │ │ docked when   │     │
│    │ │ groups, rows  │ │                                            │ │ 424 px of     │     │
│ Cl │ │ 62 px         │ │ composer                                   │ │ conversation  │     │
│ Ch │ └───────────────┘ └────────────────────────────────────────────┘ │ remain        │     │
│ Gr │                                                                  └───────────────┘     │
│ ── │                                                                                         │
│ ⚙  │                                                                                         │
└────┴─────────────────────────────────────────────────────────────────────────────────────────┘
```

| Region | Role | Geometry |
| --- | --- | --- |
| Window | Client-drawn title band on Windows and Linux, traffic lights on macOS; on Linux the client also draws the frame (5.13) | Default 1080 by 760, minimum 640 by 420; title band 38 tall (`layout::TOP_ROW`); window buttons 46 wide |
| Rail | The pages (Bots, Settings) and one ring per provider | 52 wide on the frame color; logo row 38; buttons 36 with 20 px icons at radius 12, gap 6, bottom padding 12; a 24 by 1 separator above Settings |
| Sidebar | The bots, grouped: Pinned tiles, Needs you, Unread, Bots, Hidden | Default 280, 128 to 400, compact 88 below a 110 px drag; a 16 px resize handle on its right edge; no fill of its own, the card shows through |
| Card | Holds the sidebar, the conversation, and the docked panel | Inset 4 from the right and bottom edges, radius 14, `canvas` fill, 1 px `border` edge, on the `frame` color |
| Conversation | Transcript and composer | Column 760; keeps 424 beside the sidebar before the sidebar shrinks |
| Bot panel | The bot's settings | 360, docked or covering the conversation in a narrow window |

## 4. Foundations

### 4.1 Color roles

Values live in `Theme::dark` and `Theme::light`. Components read roles,
never hex values.

| Role | Use | Dark | Light |
| --- | --- | --- | --- |
| `frame` | Window, title band, rail | `#0d0d0d` | `#e8e8eb` |
| `canvas` | The card, pages, dialogs | `#1a1a1a` | `#f5f5f6` |
| `surface` | A selected sidebar row, cards inside a page | `#212121` | `#ffffff` |
| `control`, `control_hover`, `control_active` | Resting controls and their states | `#212121`, `#242424`, `#2a2a2a` | `#ffffff`, `#f0f0f2`, `#e8e8eb` |
| `sheet` | Tooltips, toasts, the jump button, the search palette | `#141414` | `#ffffff` |
| `menu_surface` | Menus, select popups, pickers: one step above `surface` | `#2a2a2a` | `#ffffff` |
| `menu_highlight` | A menu row under the pointer or the keyboard | text at 0.06 | ink at 0.06 |
| `menu_selected` | The chosen row of a list in a menu | text at 0.10 | ink at 0.10 |
| `menu_separator` | Lines between groups of a menu | text at 0.08 | ink at 0.08 |
| `dialog_ring` | The edge of menus, pickers, and dialogs | text at 0.10 | ink at 0.10 |
| `input_edge` | The edge of a text field and of the model trigger, on `control` | text at 0.045 | ink at 0.10 |
| `picker_surface`, `picker_rail` | The model picker, and its column of provider tabs | `#212121`, `#101010` | `#ffffff`, `#f4f4f5` |
| `picker_tab`, `picker_tab_edge` | The provider tab shown in the model picker | text at 0.08, 0.11 | ink at 0.06, 0.10 |
| `radio_edge` | The ring of a model that is not chosen | text at 0.22 | ink at 0.22 |
| `badge` | A small pill beside a name, such as `default`, and a key hint | text at 0.07 | ink at 0.06 |
| `palette_scrim` | The veil over the window behind the search palette | `frame` at 0.64 | `frame` at 0.60 |
| `palette_footer` | The row of key hints of the search palette, over `sheet` | text at 0.025 | ink at 0.025 |
| `round_button`, `round_button_hover`, `round_button_edge` | A round icon button, such as Search and New bot over the bot list | `#262626`, `#2e2e2e`, text at 0.08 | `#f0f0f2`, `#e8e8eb`, ink at 0.08 |
| `border`, `border_strong` | Hairlines | text at 0.035, 0.045 | ink at 0.07, 0.10 |
| `hover` | Hover wash on chrome | text at 0.06 | ink at 0.05 |
| `text`, `text_secondary`, `text_muted`, `text_dim` | Primary, labels, secondary, disabled marks | `#ffffff`, `#dcdcdc`, `#979797`, `#6a6a6a` | `#141414`, `#3a3a3c`, `#6b6b70`, `#8e8e93` |
| `accent` | Focus edge, selection, links | `#007cf7` | `#007cf7` |
| `success`, `warning`, `danger` | Provider and bot states | `#31cf76`, `#ff9412`, `#fd2f3b` | `#1fa85a`, `#f08a00`, `#e8222d` |
| `success_soft`, `warning_soft`, `danger_soft` | The fill of a state badge, under the matching `_text` color | each state color at 0.12, 0.12, 0.16 | at 0.08, 0.14, 0.12 |
| `inverse_surface`, `inverse_text` | The primary button | `#f0f0f0` on `#141414` | `#1c1c1e` on `#ffffff` |
| `mascot_eye`, `mascot_pupil`, `mascot_beak` | The eyes, the pupils, and the beak of a mascot, over the color of its bot (5.12) | `#ffffff`, `#17171a`, black at 0.42 | The same |

Text roles hold 4.5:1 on every surface they sit on, and focus marks 3:1; the
tests `text_meets_wcag_contrast_on_every_surface_it_sits_on` and
`focus_rings_and_status_marks_meet_non_text_contrast` enforce it. A new
surface joins those tests.

### 4.2 Shadows

| Surface | Shadow |
| --- | --- |
| Menus, pickers, dialogs, toasts (`menu_shadow`) | A 1 px `dialog_ring` edge, then 0 18, blur 44, spread -18, black at 0.80 in dark and 0.55 in light |
| The search palette (`drop_shadow`) | The drop of the menu shadow alone: its 1 px `dialog_ring` border is its only edge |
| The chosen head of the avatar picker | A 2 px spread of `accent_soft` outside its `accent_text` edge |
| The window on Linux, floating (`chrome::shadow`) | 0 0, blur 12, the color of `shadow_menu` at half its alpha, inside the 16 px margin (5.13) |

No other surface carries a shadow. Depth comes from the surface steps
(`frame`, `canvas`, `surface`, `menu_surface`).

### 4.3 Geometry

| Element | Radius | Notes |
| --- | --- | --- |
| The window on Linux (`chrome::RADIUS`) | 19 | The card's 14, plus its inset of 4 and the 1 px border. A tiled edge squares its two corners. A full-window fill (a dialog scrim) takes 18 (`Frame::round_inner`) |
| Card, wide picker (`menu::WIDE_RADIUS`), search palette (`palette::RADIUS`) | 14 | A wide picker and the palette pad 6, so their rows and inner panels take 8 |
| Template cards, toasts, the cards and the logo tile of Settings | 12 (`radius::LG`) | A clickable row at an end of a Settings card keeps that 12 on its outer corners |
| Plain menu (`menu::RADIUS`) | 8 | Pads 4, so its rows take 4 |
| Sidebar rows, text fields, the model trigger, primary and wide buttons | 8 (`radius::MD`) | |
| Menu rows in a plain menu (`menu::ROW_RADIUS`) | 4 | |
| Buttons, icon buttons, tooltips | 6 (`radius::SM`) | |
| Rail buttons | 12 (`radius::LG`) | |
| Swatches, state dots, chips, badges | full | |
| Avatars | None | A mascot is drawn alone, with no surface behind it (5.12) |

Two rules hold everywhere:

1. **Concentric radii.** A rounded surface inside another, with `p` pixels
   between their edges, has radius `outer - p`. Rows of a menu are the
   common case: 8 = 4 + 4, 14 = 8 + 6.
2. **GPUI clips to rectangles.** `overflow_hidden` on a rounded parent does
   not round its children: a child with a fill that reaches a rounded corner
   paints a square corner over it. A child that touches a rounded edge MUST
   either have no fill or carry its own concentric radius. Separate the
   columns of a panel with a hairline (`menu_separator`), never with a flush
   fill. A scroll container clips the focus halo of a field inside it: pad
   the field by `size::FOCUS_RING`.

### 4.4 Spacing and sizes

The spacing scale is 2, 4, 6, 8, 12, 16, 24, 32 (`space`). Controls are 24,
28, 32, 36, 40 tall (`control`). Icons are 12, 14, 16, 20 (`icon`).

| Measure | Value |
| --- | --- |
| Group gap | At least twice the gap inside the group |
| Sidebar list | Padding 8 on the sides, 4 between rows; rows 62 tall, padding 8, avatar 46 |
| Text field | 32 tall, padding 12 by 5 (`FIELD_HEIGHT`, `FIELD_PADDING_Y`) |
| Form field | 32 tall, padding 12 by 4, `text::FIELD` (`FORM_FIELD_PADDING_Y`); a form area pads 12 by 8 and holds 2 to 9 rows of `text::AREA` |
| Model trigger | 44 tall, padding 12 (`MODEL_ROW`) |
| Menu row | 28 tall, inset 8, gap 8; a check of 14 at the trailing edge |
| Menu heading | A 28 px row, 12 px Medium in `text_muted` |
| Menu separator | 1 px, inset 8, 4 above and below |
| Menu width | Context menus 192 to 288; the model picker is 360 |
| Menu height | Scrolls past 400 (`menu::MAX_HEIGHT`) |

### 4.5 Typography

Inter, bundled in Regular, Medium, and SemiBold, then the system stack:
`.SystemUIFont` and Segoe UI on Windows and macOS; Adwaita Sans, Noto Sans,
Cantarell, DejaVu Sans, then Noto Sans Arabic, Noto Sans CJK SC, and Noto
Color Emoji on Linux. Code is set in Consolas (Windows), Menlo (macOS), or on
Linux the family fontconfig resolves for `monospace`, then Adwaita Mono, Noto
Sans Mono, DejaVu Sans Mono, Ubuntu Mono, Liberation Mono, and Inter when the
system has none (`font::init`). A family must name a font exactly: the text
system does not know fontconfig's aliases. On Linux the fallbacks are used in
the order named, including families with no Latin glyph (Arabic, emoji), and
a grapheme that ends in the emoji selector U+FE0F, such as `❤️`, takes the
first color family that covers it. Emoji draw in color from COLRv1 fonts such
as Fedora's Noto Color Emoji (`patches/gpui_wgpu` of the client).

| Step | Size / line | Use |
| --- | --- | --- |
| `text::SM` | 12 / 16 | Field labels, group headings, notes, chips, timestamps in tiles |
| `text::MD` | 13 / 18 | Body default, labels, the header |
| `text::LG` | 14 / 20 | Inputs, menu rows, row previews |
| `text::TITLE` | 15 / 20 | Bot names in rows, the primary button of a form |
| `text::XL` | 16 / 22 | Suggestion titles, screen headings |
| `text::FIELD`, `text::AREA` | 16 / 22, 16 / 24 | The fields and areas of a form page, such as the creation form |
| `text::XXL` | 18 / 24 | The largest heading of a message, the headings of Settings |

Emphasis is one weight step. Labels are Medium in `text_secondary`. Sentence
case everywhere. Never an em dash.

### 4.6 Icons

Lucide stroke icons (ISC) in `assets/icons/`, drawn as T3 Code draws them:
the Lucide glyph with a stroke of 1.75 in place of 2, at every size. They are
painted with `text_color`: `text_muted` at rest, `text` on hover. A new icon
is the Lucide file with `stroke-width="1.75"`, added to the `icons!` list in
`src/assets.rs`; a test fails on another stroke. Provider marks are the
providers' logos, in `assets/icons/providers/`: Claude and Grok from LobeHub
Icons (MIT), OpenAI from T3 Code (MIT). They are filled with one
color by the client (`rail::provider_glyph`): 14 in the rail ring, 19 in a
picker tab, 15 beside the model in the trigger, where the Claude mark takes
`claude_mark` and the others `text`. They only name the provider a bot runs
on. A provider with no logo shows two letters of its name.

### 4.7 Motion

| Motion | Duration | Notes |
| --- | --- | --- |
| Menu and picker reveal (`ui::menu_reveal`) | 140 ms, ease-out quint | Fades in from 0 while dropping 4 px into place; no exit animation; shown at rest when the system asks for reduced motion (`cx.reduce_motion()`) |
| Spinner | 900 ms loop | Loading states |
| Working dots | 960 ms cycle | The three dots over the mascot of a bot that works hop one after the other, 5 of the 100 of its box high (`mascot::DOT_HOP`); they stand still when motion is reduced |
| Suggestion row fades (`duration::FADE`) | 160 ms, ease-out quint | Each edge fades in while the row can scroll past it; shown at rest when motion is reduced |
| Suggestion row glide (`motion`) | Per frame | A drag past 8 px moves the row; on release it keeps the speed of the drag (at most 1.75 px/ms) and loses 12 % of it per 60 Hz frame until it stops or meets an end; no glide when motion is reduced |
| Hover | Instant | Rows and buttons |
| Mascot eyes | Instant | The eyes change with the state of the bot at once: no transition between states yet (5.12) |
| Mascot sway | 6.1 to 9.7 s, ease-in-out, there and back | The head leans to one side then the other about its base; twice as fast while the bot waits (5.12) |
| Mascot blink | Every 3.1 to 4.9 s | Lids in the bot's color come down over the eyes and go back up, then lower a little once more |
| Mascot bob | A quarter of the sway | A mascot with something new rises 2.5 of the 100 of its box and comes down |
| Search palette | Instant | It opens and closes with no animation: it is opened many times a day |

Motion explains a state change. Nothing animates for decoration, with one
exception: a mascot is alive (5.12). Its life stands still when the system
asks for reduced motion. GPUI reads that wish from no system: the client sets
it (`reduced_motion.rs`) from the portal's `reduced-motion` and GNOME's
Animations switch on Linux, and from "Animation effects" on Windows.

## 5. Component contracts

### 5.1 Rail (`workspace/rail.rs`)

App-wide controls only: what belongs to one bot never goes there. Top: the
logo row, which drags the window, then Bots, the page of the bots. Bottom: one
button per provider, a 28 px ring of 2 px in the provider's state color
(`forms::provider_color`: success ready, warning signed out, danger runtime
missing, `text_dim` not installed or unknown) around its 14 px logo; its
tooltip and accessible name say `<name>: <state>`, and it opens the page of
that provider in Settings. Then the separator and Settings. The page shown
keeps the `control_hover` fill and its icon in `text`, and its accessible name
ends with ", selected". A page item joins the rail only when its page exists.

### 5.2 Title band

The sidebar topbar holds only the sidebar toggle, 10 px from the sidebar's
left edge (centered when compact). The header holds the bot button (avatar
18, name, title), the model, History, New chat, a divider, and the panel
toggle. The empty band drags the window: every drag zone takes
`WindowDrag::window_drag`, never a bare `WindowControlArea::Drag`. A control
inside the band occludes it (`.occlude()`), so its presses do not move the
window. The window buttons close the band on Windows and Linux (5.13).

### 5.3 Sidebar

Over the list, 9 below its top and 8 in from each side, aligned with the
edges of the rows, two round buttons
(`ui::round_button`): Search everything (Ctrl+K) on the leading edge, which
opens the search palette, and New bot on the trailing edge. Each is a 36 circle on `round_button` with a
1 px `round_button_edge`, `round_button_hover` under the pointer, a 14 px
icon in `text_secondary`, and a tooltip. A compact sidebar stacks them. The
list is not filtered in place: the palette searches bots, conversations, and
messages. Rows: name 15 Medium, preview
14 in `text_muted`, time 13 in `text_readable`; the selected row takes
`surface`, others `control_hover` under the pointer.

### 5.4 Text field (`ui::input_skin`, `Workspace::input_box`)

Radius 8, `control` fill with a 1 px `input_edge`. Focused: the edge turns
`border_focus`, with no ring. Holding an error: the edge turns `danger`, with
the message under the field in `danger_text`. A label sits 6 px above the
field, `text::SM` Medium in `text_secondary`; a required field adds a `danger`
asterisk 2 px after its label. The placeholder of a form field is a sample
value in `text_placeholder`; other placeholders are hints in `text_muted`.

### 5.5 Select trigger and picker

A trigger is a row that opens a picker beside it: label on the leading edge
(`text::SM` in `text_secondary`), value on the trailing edge in `text::MD`
(the provider logo, or a muted name for a provider with no logo, then the
value in SemiBold, then muted detail after a `·`),
then a 14 px chevron that turns over while the picker is open. It reads as a
form control: the `control` fill and the `input_edge`, unchanged under the
pointer and while the picker is open, so opening it never moves it; keyboard
focus turns the edge `border_focus`.

The model picker of the creation form (`workspace/creation.rs`, tokens in
`theme::picker`) opens 8 under the trigger, aligned on its leading edge,
through a `deferred` `anchored` element attached to the trigger's bounds, with
`ui::menu_reveal`; it moves above the trigger when the window has no room
under it. It is 360 wide, 226 to 520 tall (and leaves 100 of the window
height), radius 14, on `picker_surface` with the `dialog_ring` edge and the
`menu_shadow`.

- Provider tabs: a 52 column on `picker_rail`, padded 9 by 7, with a
  `border` line on its trailing edge and its own leading corners at radius
  13, so no fill reaches the panel's corner. A tab is 36 square, radius 8,
  and shows the logo of its provider: `text` at 0.48 at rest, 0.80 under
  the pointer, full while shown, on `picker_tab` with a `picker_tab_edge`
  edge. A provider that cannot run a turn shows its logo at 0.28 (0.60 under the pointer or shown) and a 6 dot 6 in from the top
  trailing corner, `warning` when it is signed out and `text_muted`
  otherwise, ringed 2 in `picker_rail`. Showing a tab never moves the bot to
  that provider: picking one of its models does. The foot of the column
  holds a 36 settings button (a 16 gear, `text_muted`, `text` under the
  pointer) that closes the picker and opens Settings > Providers.
- The panel beside the tabs pads 6. Heading, 6 above and 8 below, inset 10:
  the provider name in `text::LG` SemiBold, then its account, its runtime, or
  its state in `text::SM` `text_muted`, 1 below.
- What keeps the provider from listing models, when something does, sits in a
  box on `menu_highlight`, radius 8, padded 8 by 10, in `text::SM`
  `text_secondary`, 6 above the search.
- Search: 32 tall, radius 8, on `menu_highlight` with no edge, inset 10. It
  filters the models by name and id. A typed id the provider does not list
  ends the list as a "Use" row; Enter picks the first row.
- Model rows, 6 below the search: padded 6 by 10, radius 8, `text::MD`
  Medium, `menu_highlight` under the pointer. A 16 radio sits 10 in: a
  `radio_edge` ring, or filled with `text` and a 12 check in
  `picker_surface` when chosen. The model the provider runs by default
  carries a `default` pill 7 after its name (`text::SM` Medium `text_muted`
  on `badge`, padded 1 by 6, leading 15), and picking it follows the
  provider's default. A provider that lists no model offers "Default model".
  The daemon lists only the newest model of each family (the parts of the id
  that hold no number), and always the default model.
- Reasoning: a footer 6 below the list, across the whole panel under a
  `border` line, padded 6 (16 on the leading edge): "Reasoning" in `text::SM`
  `text_muted`, then a 128 by 32 select (radius 8, `menu_highlight`,
  `picker_tab` under the pointer or open, value in `text::MD` Medium, an
  up-down chevron). Its levels open 4 under it in a plain `ui::menu_panel`
  aligned on its trailing edge. The levels are those of the model: the
  bot's model on its own tab, the default model on another, as the daemon
  read them by moving a session to each model; a model with none of its own
  takes the provider's. A level the new model does not have goes back to its
  default. A level chosen on another tab waits for a model of that tab.
  Picking a model closes the picker unless the model has levels.

While a picker is open, a transparent backdrop covers the window: a click
outside closes the picker and does nothing else. Escape closes the reasoning
menu first, then the picker, before it reaches the form.

### 5.6 Creation form (`workspace/creation.rs`)

The form takes over the conversation pane. Its column is 420 wide (less 24
on each side in a narrow pane), padded 24 above and below, with 12 between
parts, and sits centered in the height left above the suggestions.

- The live avatar is 72 and rises 8 above its box, with 4 more below it.
- Colors: the automatic color, which follows the head, then the 17 of
  `AVATAR_COLORS`, in two rows of 9 (`SWATCH_COLUMNS`). A swatch is 18 with
  a `border_strong` edge in a 28 button; the chosen one carries a 1 px
  `accent_text` ring at the edge of the button, 4 out from the swatch.
- Heads: one row of the 6 heads. Each cell draws the avatar it gives, a 32
  mascot in the current color, in 40 cells 2 apart that wrap in a narrow
  pane. The chosen cell has an `accent_text` edge and the 2 px `accent_soft`
  ring. The form opens on the head a bot made now would take (`mascot_at`).
  The settings panel of a bot shows the same two pickers under its avatar.
  An avatar is two choices and no more: making a bot stays quick.
- The fields start 4 below the gap: Name (required, opened on the default
  name "New bot"), Purpose (an area), the model trigger, then any provider
  notice.
- The form opens with the focus on itself, not on a field, so no field shows
  a focus edge until it is clicked or reached with Tab; Escape cancels from
  anywhere in it. A name the daemon refuses moves the focus to the name.
- Create is the form primary, 4 below the gap. It is faded and inert while
  the name is empty, and shows a spinner while the bot is made.
- Suggestions: a heading in `text::SM` Medium `text_muted`, then a row of
  cards 256 by 112, 12 apart, radius 12, padding 16, `border` edge: a 42
  avatar centered beside a title (`text::XL` Medium, 2 lines at most) and a
  short description (`text::MD`, `text_muted`). Under the pointer a card
  takes `border_strong` and `hover`; the chosen one, the suggestion the form
  still holds unchanged, takes `accent` and `accent_soft`. The row scrolls by
  drag (open and closed hand cursors) and by a vertical wheel until it meets
  an end, then the wheel goes to the page. It fades 48 wide at each edge it
  can scroll past. A drag never counts as a click on the card under it.

The "Create your first bot" row of the sidebar shows the live avatar at 46,
and its label in `text::LG` SemiBold.

### 5.7 Menus (`ui::menu_panel`, `ui::menu_row_shape`)

A context menu is a plain `menu_panel` (radius 8, padding 4) of rows 28 tall
at radius 4. The keyboard highlight and the pointer share `menu_highlight`.
A destructive row keeps `danger_text` and the `danger_soft` wash. A disabled
row sits at 0.64 opacity and takes no hover. A separator is
`ui::menu_separator`. Up, Down, Home, End move the highlight; Enter runs it;
Escape or a press outside closes the menu and returns focus.

### 5.8 Buttons (`ui::button`)

32 tall, padding 12, radius 6, 13 Medium. Primary: `inverse_surface` with
`inverse_text`, one per action group, at 0.82 under the pointer
(`opacity::PRIMARY_HOVER`); a primary that cannot run yet keeps its look at
0.42 (`ButtonKind::PrimaryDisabled`). Outline: `control` with a
`border_strong` edge. Raised: the outline button of a card, `surface_raised`
with a `border_strong` edge and `control_active` under the pointer, because
`control` is the fill of the card itself. Ghost: no fill until hover. Danger: `danger_soft` with
`danger_text`. The primary action of a form is 36 tall, padding 16, radius 8,
14 Medium, at least 132 wide. Icon buttons are 28 with a 16 px icon at
radius 6 and always carry a tooltip.

### 5.9 Dialogs, toasts, tooltips

Dialogs sit on `canvas` at radius 14 or 20 with the `dialog_ring` edge and the
menu shadow over the overlay. Toasts sit on `sheet` at radius 12. Tooltips sit
on `surface_raised` at radius 6 with a `border_strong` edge, 12 SemiBold.

### 5.10 Settings (`workspace/settings.rs`, tokens in `theme::settings`)

Settings are a page, not a dialog: they take the place of the sidebar, the
conversation, and the title band of the bot. The rail stays.

- Title band: over the section column, one Back button (`ui::icon_button`,
  an arrow, 10 from the edge of the column) that goes one level up: from the
  page of a provider to the list, and from a section to the bots. Then, 8
  after the column, the section shown in `text::LG` SemiBold, or the path
  to the page of a provider: the parent in `text_muted` (a button), a 14
  chevron, the provider in SemiBold. No Close button stands beside the
  caption buttons. Escape closes, in this order: an open select, the
  executable field, the search, the page.
- Section column: as wide as the sidebar (220 at least), padded 12 on the
  sides and 16 above, with a `border` line on its trailing edge. "Settings"
  in `text::XXL` SemiBold, 8 in. 12 under it, the search: a text field
  (5.4) of 32 with a full radius and a 14 search icon. 12 under it, the
  sections: rows of 32, 4 apart, radius 8, padded 12, a 16 icon then the
  name in `text::LG`. The section shown takes `control_hover`, Medium, and
  `text`; the others are `text_secondary` with a `text_muted` icon, and take
  `hover` and `text` under the pointer.
- Search: a query replaces the sections with the rows, the sections, and the
  providers that hold each of its words; a title that starts with the query
  comes first. A result is two lines, its section (14 icon, `text::SM`,
  `text_muted`) over its title. Up and Down move through the results and
  Enter opens the one they are on, which clears the query. With no result,
  the column says `No setting matches “<query>”.`
- Content: a centered column of 720 at most, padded 24 (16 in a pane under
  520), that scrolls. A group is a heading (`text::MD` Medium,
  `text_secondary`), then, 8 under it, a card: `surface`, radius 12, no
  edge. Groups stand 24 apart. Rows are 64 at least, padded 16 by 12, with
  a `border` line between two rows, inset 16. A row holds a title
  (`text::TITLE` Medium) over a description (`text::MD`, `text_muted`, 2
  under), and its control at the trailing edge. In a pane under 460 the
  control goes under the text.
- Select: 168 by 32, radius 8, `surface_raised` with a `border_strong`
  edge, the value in `text::MD` Medium and a 14 up-down chevron;
  `control_active` under the pointer and while open, `border_focus` on
  keyboard focus. Its values open 4 under it in a plain `ui::menu_panel`
  of the same width, aligned on its trailing edge, the chosen one with a
  check. The arrows open the menu and move through it, Home and End go to
  its ends, Enter or Space takes the row, Escape or a press outside closes
  it. While the daemon is away a select is faded and inert.
- Providers: the providers that are ready under "Connected", the others
  under "Not connected". A row is a button: a 20 logo, the name over the
  account or what stops the provider, a state badge unless it is ready,
  and a 16 chevron. It takes `control_hover` under the pointer and opens
  the page of the provider.
- Page of a provider: a 48 tile (`surface_raised`, `border_strong` edge,
  radius 12) with the 24 logo, the name in `text::XXL` SemiBold beside the
  state badge, one line that says what stops the provider or that it is
  ready, and "Check again" at the trailing edge. Then "Account" (the
  sign-in and its controls) and "Runtime" (the version with its download,
  and the executable). Buttons on a card are Raised (5.8); the sign-in is
  the primary.
- State badge: 22 tall, a full radius, padded 8, a 6 dot then the state in
  `text::SM` Medium. Ready is `success_text` on `success_soft`, signed out
  `warning_text` on `warning_soft`, runtime missing `danger_text` on
  `danger_soft`, and a state with no color `text_secondary` on `badge`.
  The word carries the state: the color only repeats it.

### 5.11 Search palette (`workspace/palette.rs`, tokens in `theme::palette`)

Ctrl+K, or the Search button over the bot list, opens the palette over the
shell. Its anatomy is the command palette of T3 Code, drawn with the tokens
of this document.

- Veil: `palette_scrim` over the whole window. GPUI has no blur of what is
  behind an element, so the veil is a flat wash. A press on it closes the
  palette.
- Panel: 576 wide, 420 tall at most, on `sheet` at radius 14 with one 1 px
  `dialog_ring` border and the drop of the menu shadow, without its ring: a
  ring around a border makes an edge of 2. A tenth of the window height stays
  free over it and under it, and it never covers the title band; in a
  window too short for it, the list shrinks.
- Field: a row of 48 with no edge, no fill, and no line under it. A 16
  search icon in `text_muted` stands in a 20 lead box, 14 from the edge of
  the panel; the text is `text::LG`, 8 after it.
- List: padded 6, it scrolls. A group is a heading (`text::SM` Medium,
  `text_muted`, padded 8 by 6), then its rows; groups stand 6 apart. The
  groups are Bots, Conversations, Messages, in that order.
- Row: padded 8 by 6, radius 8, gap 8. A 20 lead box on the first line (the
  avatar of the bot, or a 16 icon in `text_muted`), then the title in
  `text::LG` and `text`, then, 6 after it, its context in `text::SM` and
  `text_muted`, on 45 % of the row at most. A bot shows its name and its
  title; a conversation its title and its bot; a message its author
  (`You`, then `to <bot>`) over one line of the message in `text::SM` and
  `text_muted`. A conversation and a message end with their time, in
  `text::SM` and `text_muted`, right-aligned in 48 at least. The words of
  the query are SemiBold in `text`.
- Highlight: one row holds `menu_highlight`, for the keys and the pointer
  alike. Up and Down move it, wrap at the ends, and scroll it into view;
  the pointer takes it when it moves onto a row, not when the list moves
  under it. Enter or a click opens the row. No row has a hover of its own.
- Status: with no query, while the daemon searches, and when nothing
  matches, one centered line in `text::LG` and `text_muted`, padded 40
  above and below.
- Key hints: a row padded 16 by 10 on `palette_footer`, with a
  `menu_separator` line over it and the radius of the panel less its edge
  on its lower corners (4.3, rule 2). Each hint is its keys, then, 4 after
  them, its action in `text::MD` Medium and `text_muted`; hints stand 12
  apart. A key is 20 tall and 20 wide at least, radius 4, on `badge`, in
  `text::SM` and `text_secondary`; an arrow key is a 12 icon. The hints are
  Navigate, Open (only while a row can be opened), and Close.

### 5.12 Mascot (`mascot.rs`, layers in `assets/mascot`, tokens in `theme::mascot`)

The avatar of a bot is its mascot: the head of a crow seen from the front,
in the bot's color, drawn alone. Nothing stands behind it: no disc, no ring,
no fill.

- An avatar is a color (one of `AVATAR_COLORS`, or any `#rrggbb`) and a
  head (smooth, crest, spikes, curl, plume, flat). Every mascot has the same
  short beak: the beak is not a choice. A bot made with no avatar takes the
  next head (`mascot_at`), so bots made one after the other differ.
- A mascot is a stack of SVG layers of one color each, on a shared box of
  100 by 100: the head in the bot's color, the beak in `mascot_beak`, the
  eyes in `mascot_eye`, the pupils in `mascot_pupil`. The head is 80 wide and
  sits in the lower 72 of the box; the feathers take the top. The size of an
  avatar is the size of that box, so a head reads smaller than a disc of the
  same size did.
- The eyes show the state of the bot:

  | State | Eyes |
  | --- | --- |
  | Idle | Round, looking ahead |
  | Working | Looking up at three dots in the bot's color, which hop (4.7) |
  | Waiting | Wide, with small pupils, on a head tilted 10 degrees to the left (`mascot::TILT`) |
  | Failed | Looking down under lids in the bot's color |
  | Unread | Closed in a smile |

- The sidebar and the header show the state. Everywhere else (messages, the
  empty conversation, the search palette, the pickers, the suggestions, the
  settings panel) a mascot is at rest.
- A mascot is alive where it stands for the bot itself: the sidebar, the
  header, the empty conversation, the creation form, and the settings panel.
  It is still where it is a label: beside a message, in the search palette,
  in a picker cell, on a suggestion.
- Alive, a mascot moves while nothing changes (`theme::mascot`, 4.7):
  - Its head sways about its base (`NECK`), 2.5 degrees to each side at
    rest, 1.5 at work, 2 about its tilt while it waits, 0.8 after a failure.
  - At rest its pupils look 1.2 to the side its head leans to. At work they
    stay on the dots, and while it waits, on the user.
  - It blinks: lids in its own color (`blink.svg`) hang from 28 on the box,
    above the eyes, and come down to 65. The first part of their way passes
    over the head and shows nothing, which keeps a blink short. A mascot
    with something new does not blink: its eyes smile, and it bobs.
  - Each bot has a sway and a blink of its own length, from its id, so the
    bots of a list do not move together.
- The life costs no drawing: each motion is a timed transformation of a
  layer (`Svg::with_timed_transformation`), which the window plays in the
  scene it already has, at 30 frames per second at most. Only the dots of a
  bot that works draw the view again. A new motion of a mascot takes the
  same path: never an animation that draws the view at each frame for a
  mascot that only stands there.
- Under 24 (`mascot::SMALL`: the header at 18, the palette at 20) the eyes
  are drawn 1.22 times larger about the point between them
  (`assets/mascot/small`).
- A sidebar row keeps a state badge at the lower right of the box for the
  states that ask for the user: waiting, failed, and unread, which carries
  the count of new messages. It covers the lower right of the head and
  leaves the eyes clear. A bot that works has no badge: the dots of its
  mascot show it.
- A new head is a file of `assets/mascot` with `fill="black"` on the shared
  box, a variant of `MascotShape` in the protocol, and its layer in
  `mascot.rs`.

### 5.13 Linux window (`chrome.rs`, tokens in `theme::chrome`)

GNOME draws no title bar for a Wayland window, so on Linux the client asks
for client decorations and draws the chrome itself.

- **Frame.** A floating window keeps a transparent margin of 16
  (`chrome::SHADOW`) around its surface, given to the compositor as the
  client inset. The surface carries the `frame` fill, a 1 px `dialog_ring`
  border, a radius of 19, and the window shadow (4.2). The workspace inside
  paints no fill of its own over the whole window. The margin is added to the
  default size, so the window shows 1080 by 760 (`chrome::default_size`).
  Wayland counts the minimum in the window's geometry, X11 counts the whole
  window, so on X11 the margin is added to the minimum too
  (`chrome::min_size`): a window the X server decorates is then 672 by 452 at
  its smallest.
- **Resize.** Each free edge resizes in the band of 10 of the margin next to
  the window (`chrome::RESIZE`, the border Mutter grabs around a window it
  decorates); each corner in a square of 15 (`chrome::RESIZE_CORNER`) from
  the band's outer edge, which reaches 5 into the window. The outer 6 of the
  margin are outside the input region (`chrome::input_region`), so a click
  there reaches the window behind; GPUI sets an input region on Wayland only,
  so on X11 the whole margin takes input. The cursor
  shows the matching resize shape there, and a press there resizes before any
  element under it acts (the close button and the rail's logo sit under the
  top corners).
- **Tiled states.** A tiled edge has no margin, no border, no rounding on its
  two corners, and no resize zone; the free edges keep theirs. A tiled
  window has no shadow. A maximized or fullscreen window is tiled on every
  edge.
- **Title band.** A press and a drag on a drag zone move the window; a double
  click maximizes or restores it; a right click opens the desktop's window
  menu.
- **Buttons.** Minimize, maximize (restore when maximized), and close, in
  the order and on the side of the desktop's button layout (KWin's own, read
  from `kwinrc`, on KDE), among the ones
  the compositor allows. A desktop with no layout gets the three on the
  right. GNOME's default names close alone. Left buttons open the band before
  the sidebar topbar; right buttons close it. Each is a 46 by 38 slot
  holding a 24 round hover of `hover` behind a 16 Lucide-style icon in
  `text_secondary` (`window_minimize`, `window_maximize`, `window_restore`,
  `window_close`), so no fill reaches the rounded corner. The slot takes the
  pointer; the round is the button: a tab stop with the Button role and its
  label, which Enter or Space presses, and whose 1 px edge turns
  `border_focus` when the keyboard brings the focus there. A click presses
  it and leaves the focus where it was.
- **A compositor that decorates** (`Decorations::Server`: X11 with no
  compositing manager, or KDE when it keeps its own decorations) gets no
  buttons, frame, shadow, rounding, or resize zone from the client; the band
  keeps its content. The chrome follows a change at the next frame.

### 5.14 Application icon (`identity.rs`, files in `resources/linux`)

The desktop shows Crow Bot by one icon: in the dock, the overview, and the
application grid. It is a mascot (5.12), so the product and its bots read as
one family.

- The icon is the mascot at rest with the crest head, in `accent`
  (`#007cf7`): the layers `head_crest`, `beak`, `eyes`, and `pupils_rest` of
  `assets/mascot`, on the shared box of 100, with no disc or plate behind it,
  as every mascot. The beak is black at 0.42, the eyes `#ffffff`, the pupils
  `#17171a`, as `mascot_beak`, `mascot_eye`, and `mascot_pupil`.
- `accent` keeps a contrast of at least 3 to 1 against both a light and a
  dark background (4.0 on white, 5.2 on black), so the outline of the crow
  holds on a light dock and a dark one with no rim. A change of color keeps
  that bound: the test of the 48 px icon fails under it.
- The scalable icon is the source. The PNG files at 48, 128, 256, and 512 are
  that SVG rendered at their size; a test fails when one drifts from it.
- Every file is named after the application ID (`identity::APP_ID`), and the
  desktop entry names the application "Crow Bot" (`identity::APP_NAME`),
  which is also the title of the window.

## 6. What not to do

- A fill that reaches a rounded corner of its parent (4.3, rule 2), the
  rounded window of Linux included (5.13).
- A radius that breaks concentricity, or a radius, size, color, or duration
  that is not a token.
- A shadow outside 4.2.
- An accent fill on anything larger than a chip, a selected swatch, or the
  focus halo.
- Hue in a neutral.
- A disc, a ring, or any fill behind a mascot (5.12).
- A control without an accessible name, or an icon button without a tooltip.
- `aria_expanded` or `aria_selected` on a clickable element without an
  `on_a11y_action` handler: GPUI then drops its Invoke pattern.
- Text, code, or assets taken from another product, such as OpenBot. Provider
  logos come from a source whose license allows it (4.6).

## 7. Delivery gate

Before a UI change is done:

1. `cargo test`, `cargo clippy --all-targets --no-deps`, and `cargo fmt
   --check` pass in `crates/crowbot-desktop`.
2. The changed surface is checked in the running client with the
   `/agent-desktop` skill, in dark and light, at the default size and at the
   640 by 420 minimum.
3. Hover, focus, open, selected, disabled, loading, empty, and error states
   of every control touched hold, and long names truncate. A one-line
   truncation goes through `truncated::truncated`, which never cuts inside a
   grapheme (a flag, a skin tone, a joined emoji); GPUI's `.truncate()`
   alone can.
4. Every rounded surface touched is checked up close for square corners.
