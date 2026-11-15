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
| Window | Client-drawn title band on Windows, traffic lights on macOS | Default 1080 by 760, minimum 640 by 420; title band 38 tall (`layout::TOP_ROW`); caption buttons 46 wide |
| Rail | One ring per provider, Settings | 52 wide on the frame color; logo row 38; buttons 36 with 20 px icons at radius 12, gap 6, bottom padding 12; a 24 by 1 separator above Settings |
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
| `sheet` | Tooltips, toasts, the jump button | `#141414` | `#ffffff` |
| `menu_surface` | Menus, select popups, pickers: one step above `surface` | `#2a2a2a` | `#ffffff` |
| `menu_highlight` | A menu row under the pointer or the keyboard | text at 0.06 | ink at 0.06 |
| `menu_selected` | The chosen row of a list in a menu | text at 0.10 | ink at 0.10 |
| `menu_separator` | Lines between groups of a menu | text at 0.08 | ink at 0.08 |
| `dialog_ring` | The edge of menus, pickers, and dialogs | text at 0.10 | ink at 0.10 |
| `input_edge` | The edge of a text field and of the model trigger, on `control` | text at 0.045 | ink at 0.10 |
| `picker_surface`, `picker_rail` | The model picker, and its column of provider tabs | `#212121`, `#101010` | `#ffffff`, `#f4f4f5` |
| `picker_tab`, `picker_tab_edge` | The provider tab shown in the model picker | text at 0.08, 0.11 | ink at 0.06, 0.10 |
| `radio_edge` | The ring of a model that is not chosen | text at 0.22 | ink at 0.22 |
| `badge` | A small pill beside a name, such as `default` | text at 0.07 | ink at 0.06 |
| `round_button`, `round_button_hover`, `round_button_edge` | A round icon button, such as Search and New bot over the bot list | `#262626`, `#2e2e2e`, text at 0.08 | `#f0f0f2`, `#e8e8eb`, ink at 0.08 |
| `border`, `border_strong` | Hairlines | text at 0.035, 0.045 | ink at 0.07, 0.10 |
| `hover` | Hover wash on chrome | text at 0.06 | ink at 0.05 |
| `text`, `text_secondary`, `text_muted`, `text_dim` | Primary, labels, secondary, disabled marks | `#ffffff`, `#dcdcdc`, `#979797`, `#6a6a6a` | `#141414`, `#3a3a3c`, `#6b6b70`, `#8e8e93` |
| `accent` | Focus edge, selection, links | `#007cf7` | `#007cf7` |
| `success`, `warning`, `danger` | Provider and bot states | `#31cf76`, `#ff9412`, `#fd2f3b` | `#1fa85a`, `#f08a00`, `#e8222d` |
| `inverse_surface`, `inverse_text` | The primary button | `#f0f0f0` on `#141414` | `#1c1c1e` on `#ffffff` |

Text roles hold 4.5:1 on every surface they sit on, and focus marks 3:1; the
tests `text_meets_wcag_contrast_on_every_surface_it_sits_on` and
`focus_rings_and_status_marks_meet_non_text_contrast` enforce it. A new
surface joins those tests.

### 4.2 Shadows

| Surface | Shadow |
| --- | --- |
| Menus, pickers, dialogs, toasts (`menu_shadow`) | A 1 px `dialog_ring` edge, then 0 18, blur 44, spread -18, black at 0.80 in dark and 0.55 in light |
| A chosen avatar picture | A 2 px spread of `accent_soft` outside its `accent_text` edge |

No other surface carries a shadow. Depth comes from the surface steps
(`frame`, `canvas`, `surface`, `menu_surface`).

### 4.3 Geometry

| Element | Radius | Notes |
| --- | --- | --- |
| Card, settings dialog, wide picker (`menu::WIDE_RADIUS`) | 14 | A wide picker pads 6, so its rows and inner panels take 8 |
| Template cards, toasts | 12 (`radius::LG`) | |
| Plain menu (`menu::RADIUS`) | 8 | Pads 4, so its rows take 4 |
| Sidebar rows, text fields, the model trigger, primary and wide buttons | 8 (`radius::MD`) | |
| Menu rows in a plain menu (`menu::ROW_RADIUS`) | 4 | |
| Buttons, icon buttons, tooltips | 6 (`radius::SM`) | |
| Rail buttons | 12 (`radius::LG`) | |
| Avatars, swatches, state dots, chips, badges | full | |

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

Inter, bundled in Regular, Medium, and SemiBold, then the system stack.
Consolas, Menlo, or monospace for code.

| Step | Size / line | Use |
| --- | --- | --- |
| `text::SM` | 12 / 16 | Field labels, group headings, notes, chips, timestamps in tiles |
| `text::MD` | 13 / 18 | Body default, labels, the header |
| `text::LG` | 14 / 20 | Inputs, menu rows, row previews |
| `text::TITLE` | 15 / 20 | Bot names in rows, the primary button of a form |
| `text::XL` | 16 / 22 | Suggestion titles, screen headings |
| `text::FIELD`, `text::AREA` | 16 / 22, 16 / 24 | The fields and areas of a form page, such as the creation form |
| `text::XXL`, `text::XXXL` | 18 / 24, 24 / 30 | Large glyphs in avatars |

Emphasis is one weight step. Labels are Medium in `text_secondary`. Sentence
case everywhere. Never an em dash.

### 4.6 Icons

Lucide stroke icons (ISC) in `assets/icons/`, painted with `text_color`:
`text_muted` at rest, `text` on hover. A new icon is the Lucide file, added
to the `icons!` list in `src/assets.rs`. Provider marks are the providers'
logos, from LobeHub Icons (MIT) in `assets/icons/providers/`, filled with one
color by the client (`rail::provider_glyph`): 14 in the rail ring, 19 in a
picker tab, 15 beside the model in the trigger, where the Claude mark takes
`claude_mark` and the others `text`. They only name the provider a bot runs
on. A provider with no logo shows two letters of its name.

### 4.7 Motion

| Motion | Duration | Notes |
| --- | --- | --- |
| Menu and picker reveal (`ui::menu_reveal`) | 140 ms, ease-out quint | Fades in from 0 while dropping 4 px into place; no exit animation; shown at rest when the system asks for reduced motion (`cx.reduce_motion()`) |
| Spinner | 900 ms loop | Loading states |
| Working dots | 960 ms cycle | A bot that works |
| Suggestion row fades (`duration::FADE`) | 160 ms, ease-out quint | Each edge fades in while the row can scroll past it; shown at rest when motion is reduced |
| Suggestion row glide (`motion`) | Per frame | A drag past 8 px moves the row; on release it keeps the speed of the drag (at most 1.75 px/ms) and loses 12 % of it per 60 Hz frame until it stops or meets an end; no glide when motion is reduced |
| Hover | Instant | Rows and buttons |

Motion explains a state change. Nothing animates for decoration.

## 5. Component contracts

### 5.1 Rail (`workspace/rail.rs`)

App-wide controls only: what belongs to one bot never goes there. Top: the
logo row, which drags the window. Bottom: one button per provider, a 28 px
ring of 2 px in the provider's state color (`forms::provider_color`: success
ready, warning signed out, danger runtime missing, `text_dim` not installed or
unknown) around its 14 px logo;
its tooltip and accessible name say `<name>: <state>`, and it opens Settings at
Providers. Then the separator and Settings. A page item joins the rail only
when a second page exists.

### 5.2 Title band

The sidebar topbar holds only the sidebar toggle, 10 px from the sidebar's
left edge (centered when compact). The header holds the bot button (avatar
18, name, title), the model, History, New chat, a divider, and the panel
toggle. The empty band drags the window.

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
- Colors: the automatic color, which follows the picture, then the 17 of
  `AVATAR_COLORS`, in two rows of 9 (`SWATCH_COLUMNS`). A swatch is 18 with
  a `border_strong` edge in a 28 button; the chosen one carries a 1 px
  `accent_text` ring at the edge of the button, 4 out from the swatch.
- Pictures: the first letter of the name, then 11 emoji, each drawn as a 24
  avatar in the current color, in 33 cells 2 apart that wrap in a narrow
  pane. The chosen cell has an `accent_text` edge and the 2 px
  `accent_soft` ring.
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
`border_strong` edge. Ghost: no fill until hover. Danger: `danger_soft` with
`danger_text`. The primary action of a form is 36 tall, padding 16, radius 8,
14 Medium, at least 132 wide. Icon buttons are 28 with a 16 px icon at
radius 6 and always carry a tooltip.

### 5.9 Dialogs, toasts, tooltips

Dialogs sit on `canvas` at radius 14 or 20 with the `dialog_ring` edge and the
menu shadow over the overlay. Toasts sit on `sheet` at radius 12. Tooltips sit
on `surface_raised` at radius 6 with a `border_strong` edge, 12 SemiBold.

## 6. What not to do

- A fill that reaches a rounded corner of its parent (4.3, rule 2).
- A radius that breaks concentricity, or a radius, size, color, or duration
  that is not a token.
- A shadow outside 4.2.
- An accent fill on anything larger than a chip, a selected swatch, or the
  focus halo.
- Hue in a neutral.
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
   of every control touched hold, and long names truncate.
4. Every rounded surface touched is checked up close for square corners.
