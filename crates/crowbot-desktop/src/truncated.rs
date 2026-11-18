//! One line of text cut to its width between two graphemes.
//!
//! GPUI's own truncation (`.truncate()`) adds up the advance of each character
//! measured alone and cuts between two characters, so it can split a flag, a
//! skin tone, a keycap, or a joined emoji, and leave half of it on screen. This
//! element shapes the line once, keeps the longest run of whole graphemes that
//! leaves room for the ellipsis, measured on the shaped line, and draws that.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId, HighlightStyle,
    InspectorElementId, LayoutId, Pixels, ShapedLine, SharedString, Style, TextRun, TextStyle,
    Window, size,
};
use unicode_segmentation::UnicodeSegmentation;

const ELLIPSIS: &str = "…";

/// A line of `text` that ends in an ellipsis when it does not fit.
pub fn truncated(text: impl Into<SharedString>) -> Truncated {
    Truncated {
        text: text.into(),
        highlights: Vec::new(),
    }
}

pub struct Truncated {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

impl Truncated {
    /// Styles the given byte ranges over the inherited text style. The ranges
    /// are sorted and do not overlap.
    pub fn with_highlights(
        mut self,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.highlights = highlights.into_iter().collect();
        self
    }
}

pub struct Shaped {
    line: ShapedLine,
    line_height: Pixels,
}

impl Element for Truncated {
    type RequestLayoutState = Rc<RefCell<Option<Shaped>>>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let runs = runs(&self.text, &style, &self.highlights);
        let text = self.text.clone();
        let shaped = Rc::new(RefCell::new(None));
        let measured = shaped.clone();
        let layout =
            window.request_measured_layout(Style::default(), move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let line = fit(&text, &runs, font_size, width, window);
                let size = size(known.width.unwrap_or(line.width.ceil()), line_height);
                *measured.borrow_mut() = Some(Shaped { line, line_height });
                size
            });
        (layout, shaped)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let align = window.text_style().text_align;
        if let Some(shaped) = shaped.borrow().as_ref() {
            shaped
                .line
                .paint(
                    bounds.origin,
                    shaped.line_height,
                    align,
                    Some(bounds.size.width),
                    window,
                    cx,
                )
                .ok();
        }
    }
}

/// The runs of the text: the inherited style, with each highlight over it.
fn runs(
    text: &str,
    style: &TextStyle,
    highlights: &[(Range<usize>, HighlightStyle)],
) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, highlight) in highlights {
        if at < range.start {
            runs.push(style.to_run(range.start - at));
        }
        runs.push(style.clone().highlight(*highlight).to_run(range.len()));
        at = range.end;
    }
    if at < text.len() {
        runs.push(style.to_run(text.len() - at));
    }
    runs
}

/// The line shaped whole when it fits `width`, else cut and closed by the
/// ellipsis, which takes the style the text ends with.
fn fit(
    text: &SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    width: Option<Pixels>,
    window: &mut Window,
) -> ShapedLine {
    let system = window.text_system().clone();
    let whole = system.shape_line(text.clone(), font_size, runs, None);
    let Some(width) = width.filter(|width| whole.width > *width) else {
        return whole;
    };
    let mut kept = clip(runs, text.len());
    let style = kept.last().or(runs.first()).cloned();
    let Some(style) = style else {
        return whole;
    };
    let ellipsis = TextRun {
        len: ELLIPSIS.len(),
        ..style
    };
    let room = width
        - system
            .shape_line(
                ELLIPSIS.into(),
                font_size,
                std::slice::from_ref(&ellipsis),
                None,
            )
            .width;
    let end = cut(text, |index| whole.x_for_index(index), room);
    kept = clip(runs, end);
    kept.push(ellipsis);
    let shown = format!("{}{ELLIPSIS}", &text[..end]);
    system.shape_line(shown.into(), font_size, &kept, None)
}

/// The end of the longest run of whole graphemes whose right edge stays
/// within `room`, with the spaces and the ASCII punctuation it ends with left
/// out, as GPUI does before the ellipsis.
fn cut(text: &str, right_edge: impl Fn(usize) -> Pixels, room: Pixels) -> usize {
    let mut end = 0;
    for (start, grapheme) in text.grapheme_indices(true) {
        let next = start + grapheme.len();
        if right_edge(next) > room {
            break;
        }
        end = next;
    }
    text[..end]
        .trim_end_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .len()
}

/// The runs that cover the first `len` bytes.
fn clip(runs: &[TextRun], len: usize) -> Vec<TextRun> {
    let mut clipped = Vec::new();
    let mut at = 0;
    for run in runs {
        if at >= len {
            break;
        }
        let take = run.len.min(len - at);
        clipped.push(TextRun {
            len: take,
            ..run.clone()
        });
        at += take;
    }
    clipped
}

#[cfg(test)]
mod tests {
    use gpui::{Pixels, px};

    use super::cut;

    /// Each `char` is 10 wide, as a sum of advances would count it.
    fn per_char(text: &str) -> impl Fn(usize) -> Pixels + '_ {
        move |index| px(10. * text[..index].chars().count() as f32)
    }

    #[test]
    fn a_cut_never_splits_a_flag_a_skin_tone_a_keycap_or_a_joined_emoji() {
        for (text, kept) in [
            ("ab🇫🇷cd", "ab"),
            ("ab👍🏽cd", "ab"),
            ("ab1️⃣cd", "ab"),
            ("ab🐦‍⬛cd", "ab"),
        ] {
            let room = px(10. * 3.);
            assert_eq!(&text[..cut(text, per_char(text), room)], kept, "{text}");
        }
    }

    #[test]
    fn a_cut_keeps_every_grapheme_that_fits_and_drops_trailing_spaces() {
        let text = "ab 🇫🇷 cd";
        assert_eq!(&text[..cut(text, per_char(text), px(60.))], "ab 🇫🇷");
        assert_eq!(&text[..cut(text, per_char(text), px(40.))], "ab");
        assert_eq!(cut(text, per_char(text), px(5.)), 0);
    }
}
