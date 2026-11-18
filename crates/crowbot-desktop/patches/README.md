# GPUI patches

The client takes GPUI from Zed's repository at the rev pinned in `../Cargo.toml`.
One crate of it is replaced by a copy here, through the entry
`[patch."https://github.com/zed-industries/zed"]` of that manifest: Cargo then
builds the copy wherever the graph names Zed's crate. The copy is Zed's crate at
the same rev plus the changes in `<crate>.diff`, and nothing else.

| Crate | Built on | Why |
|---|---|---|
| `gpui_wgpu` | Linux only (Windows and macOS draw text with their own system) | GPUI's Linux text system draws no COLRv1 glyph, and drops fallback families that have no Latin glyph |

The copy is not a member of the client's workspace (`exclude = ["patches"]`),
so it builds with Zed's own lints, written out in its `Cargo.toml`, and the
client's gates do not lint Zed's code. Its upstream dependencies are written
out with Zed's requirements at the rev, so the lockfile keeps the versions it
had; `skrifa` and `zeno`, which it adds, were already in the graph through
`swash`.

## The changes to `gpui_wgpu`

All in `src/cosmic_text_system.rs`, plus the new `src/color_glyph.rs`.

- **COLRv1 emoji.** swash draws COLRv0 layers, color bitmaps, and outlines.
  Fedora ships "Noto Color Emoji" as COLRv1 only, so every emoji came out empty.
  `color_glyph.rs` walks the paint graph of a glyph with skrifa and paints it on
  the CPU: transforms, glyph and box clips, solid fills, linear, radial
  (two-circle), and sweep gradients with their extend modes, and layers with the
  28 composite modes of the COLR table. Its output matches HarfBuzz and cairo
  (`hb-view`) on Noto Color Emoji. A glyph of a COLRv1 face is drawn there
  first, and by swash when it has no COLRv1 paint.
- **Color fonts by their tables.** A face is an emoji face when it holds a
  COLR, CBDT, sbix, or SVG table, not only when its PostScript name is
  `NotoColorEmoji`. The workaround that skips glyph 3 stays tied to that name.
- **Fallbacks with no Latin glyph.** A family loaded as a primary font must
  have a Latin `m`, which layout measures. Upstream applied that test to the
  families of a fallback chain too, so "Noto Sans Arabic" and "Noto Color
  Emoji" left the chain and cosmic-text picked a font of its own. A family
  loaded as a fallback now keeps such faces, under a key of its own, so it never
  becomes a primary font.
- **Emoji presentation.** A grapheme with the selector U+FE0F or a keycap
  (U+20E3) takes the first color font of the chain that covers it, and one with
  U+FE0E the first font that is not a color font (Unicode Technical Standard
  #51). Without it, `❤️` took the black heart of the first text family.

The accessibility half of US-026 needed no change: GPUI publishes its window to
AT-SPI once `org.a11y.Status.IsEnabled` is true, as Orca sets it.
`scripts/accessibility-check.sh` proves it.

## Proof

- Client tests, through the text system the window draws with, on the fonts of
  `../fixtures/fonts`: `theme::text_system` (4 of its 5 tests fail on
  upstream), and the oracles of `theme::system_fonts` (ignored; run them on the
  target desktop with `cargo test system_fonts -- --ignored`).
- Upstream's own tests of the crate, and the tests of `color_glyph.rs`, pass on
  the copy: 63 of 63 on 2026-11-18. They run outside the client's gates; to run
  them, place the copy at `crates/gpui_wgpu` of a tree whose `assets` is Zed's,
  and add upstream's dev-dependencies (`gpui` with `bench-support`, `image`, and
  `naga` at the rev of `wgpu`).
- `src/gpui_patches.rs` fails when the client's Zed rev and the copy's differ.

## Upkeep

```sh
sh scripts/gpui-patches.sh check     # the copy is upstream plus the diff
sh scripts/gpui-patches.sh diff      # after a change to the copy
sh scripts/gpui-patches.sh refresh   # after a rev bump in ../Cargo.toml
```

`refresh` copies the crate of the new rev and applies the diff. When it does not
apply, resolve it by hand, then run `diff`. Compare the copy's `Cargo.toml`
with upstream's at the new rev, since it is written out by hand.

## Upstream

The changes are written to be offered to Zed (issues 58287, 61660, and 51171
were closed with no fix). They are not offered yet: that needs the owner's go.
When Zed takes one, it leaves the diff at the next rev bump, and the copy goes
when the diff is empty.
