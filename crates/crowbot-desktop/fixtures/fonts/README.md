# Test fonts

Subsets of fonts under the SIL Open Font License (`OFL.txt`), renamed so that
no system has their family: a test that uses them does not depend on the fonts
of the computer it runs on. `make_fixtures.py` builds them again, byte for
byte, from the fonts of Fedora 45 (`python3 make_fixtures.py .`, with
fontTools).

| File | Family | From | Holds |
|---|---|---|---|
| `CrowTestColorEmoji.ttf` | Crow Test Color Emoji | Noto Color Emoji (COLRv1) | 👍, ❤ in color, no Latin glyph |
| `CrowTestTextEmoji.ttf` | Crow Test Text Emoji | Noto Emoji | 👍, ❤ in outline, no Latin glyph |
| `CrowTestArabic.ttf` | Crow Test Arabic | Noto Sans Arabic | the letters of مرحبا, no Latin glyph |
| `CrowTestLatinArabic.ttf` | Crow Test Latin Arabic | Vazirmatn | the letters of مرحبا and `m` |

`theme::text_system` in `src/theme.rs` uses them.
