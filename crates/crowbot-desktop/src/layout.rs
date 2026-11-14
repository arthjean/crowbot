use crowbot_protocol::{Preferences, PreferencesPatch};

use crate::theme::layout::{
    CONVERSATION_MIN, SIDEBAR_COLLAPSE_BELOW, SIDEBAR_COMPACT, SIDEBAR_DEFAULT,
    SIDEBAR_EXPAND_ABOVE, SIDEBAR_KEY_STEP, SIDEBAR_MAX, SIDEBAR_MIN,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarLayout {
    width: f32,
    compact: bool,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self {
            width: SIDEBAR_DEFAULT,
            compact: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub width: f32,
    pub compact: bool,
}

impl SidebarLayout {
    pub fn from_preferences(preferences: &Preferences) -> Self {
        let width = preferences
            .sidebar_width
            .map(|width| width as f32)
            .filter(|width| (SIDEBAR_MIN..=SIDEBAR_MAX).contains(width))
            .unwrap_or(SIDEBAR_DEFAULT);
        Self {
            width,
            compact: preferences.sidebar_collapsed,
        }
    }

    pub fn is_compact(&self) -> bool {
        self.compact
    }

    fn limit(window_width: f32) -> f32 {
        (window_width - CONVERSATION_MIN).clamp(SIDEBAR_MIN, SIDEBAR_MAX)
    }

    pub fn drag(&mut self, pointer: f32, window_width: f32) {
        if self.compact {
            if pointer > SIDEBAR_EXPAND_ABOVE {
                self.compact = false;
                self.width = pointer.clamp(SIDEBAR_MIN, Self::limit(window_width));
            }
        } else if pointer < SIDEBAR_COLLAPSE_BELOW {
            self.compact = true;
        } else {
            self.width = pointer.clamp(SIDEBAR_MIN, Self::limit(window_width));
        }
    }

    pub fn step(&mut self, direction: f32, window_width: f32) {
        if self.compact {
            if direction > 0. {
                self.compact = false;
                self.width = SIDEBAR_MIN;
            }
        } else if direction < 0. && self.width <= SIDEBAR_MIN {
            self.compact = true;
        } else {
            self.width = (self.width + direction * SIDEBAR_KEY_STEP)
                .clamp(SIDEBAR_MIN, Self::limit(window_width));
        }
    }

    pub fn toggle(&mut self) {
        self.compact = !self.compact;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn shown(&self, window_width: f32) -> Shown {
        let compact = Shown {
            width: SIDEBAR_COMPACT,
            compact: true,
        };
        if self.compact {
            return compact;
        }
        let room = window_width - CONVERSATION_MIN;
        if room >= self.width {
            Shown {
                width: self.width,
                compact: false,
            }
        } else if room >= SIDEBAR_MIN {
            Shown {
                width: room,
                compact: false,
            }
        } else {
            compact
        }
    }

    pub fn changes_since(&self, saved: &Self) -> Option<PreferencesPatch> {
        let width = (self.width != saved.width).then(|| self.width.round() as u32);
        let collapsed = (self.compact != saved.compact).then_some(self.compact);
        (width.is_some() || collapsed.is_some()).then_some(PreferencesPatch {
            sidebar_width: width,
            sidebar_collapsed: collapsed,
            ..PreferencesPatch::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: f32 = 1080.;

    fn shown(layout: &SidebarLayout) -> (f32, bool) {
        let shown = layout.shown(WIDE);
        (shown.width, shown.compact)
    }

    #[test]
    fn a_first_start_shows_a_sidebar_of_280_pixels() {
        let layout = SidebarLayout::from_preferences(&Preferences::default());
        assert_eq!(layout, SidebarLayout::default());
        assert_eq!(shown(&layout), (280., false));
    }

    #[test]
    fn a_drag_below_110_snaps_to_compact_and_above_118_expands_again() {
        let mut layout = SidebarLayout::default();
        layout.drag(110., WIDE);
        assert_eq!(shown(&layout), (128., false));
        layout.drag(109., WIDE);
        assert_eq!(shown(&layout), (88., true));
        layout.drag(115., WIDE);
        assert_eq!(shown(&layout), (88., true));
        layout.drag(118., WIDE);
        assert_eq!(shown(&layout), (88., true));
        layout.drag(119., WIDE);
        assert_eq!(shown(&layout), (128., false));
        layout.drag(200., WIDE);
        assert_eq!(shown(&layout), (200., false));
    }

    #[test]
    fn a_drag_stops_at_the_range_and_keeps_the_conversation_at_424_pixels() {
        let mut layout = SidebarLayout::default();
        layout.drag(900., WIDE);
        assert_eq!(shown(&layout), (400., false));
        layout.drag(120., WIDE);
        assert_eq!(shown(&layout), (128., false));

        let narrow = 700.;
        layout.drag(390., narrow);
        assert_eq!(layout.shown(narrow).width, 276.);
        assert_eq!(narrow - layout.shown(narrow).width, 424.);
        layout.drag(-40., narrow);
        assert!(layout.is_compact());
    }

    #[test]
    fn a_window_too_narrow_for_the_saved_width_shrinks_the_sidebar_without_saving() {
        let layout = SidebarLayout::from_preferences(&Preferences {
            sidebar_width: Some(400),
            ..Preferences::default()
        });
        assert_eq!(layout.shown(1080.).width, 400.);
        assert_eq!(layout.shown(700.).width, 276.);
        let tiny = layout.shown(540.);
        assert_eq!((tiny.width, tiny.compact), (88., true));
        assert_eq!(layout.changes_since(&layout), None);
    }

    #[test]
    fn stored_preferences_open_the_sidebar_the_same_way() {
        let mut layout = SidebarLayout::default();
        layout.drag(333.4, WIDE);
        layout.toggle();
        let patch = layout.changes_since(&SidebarLayout::default()).unwrap();
        assert_eq!(patch.sidebar_width, Some(333));
        assert_eq!(patch.sidebar_collapsed, Some(true));
        assert_eq!(patch.appearance, None);

        let restored = SidebarLayout::from_preferences(&Preferences {
            sidebar_width: patch.sidebar_width,
            sidebar_collapsed: true,
            ..Preferences::default()
        });
        assert_eq!(shown(&restored), (88., true));
        let mut expanded = restored;
        expanded.toggle();
        assert_eq!(shown(&expanded), (333., false));
    }

    #[test]
    fn a_stored_width_out_of_range_gives_the_default() {
        for width in [0, 50, 127, 401, 4000] {
            let layout = SidebarLayout::from_preferences(&Preferences {
                sidebar_width: Some(width),
                ..Preferences::default()
            });
            assert_eq!(shown(&layout), (280., false), "stored {width}");
        }
    }

    #[test]
    fn the_keyboard_moves_the_edge_by_steps_and_reaches_the_compact_form() {
        let mut layout = SidebarLayout::default();
        layout.step(1., WIDE);
        assert_eq!(shown(&layout), (292., false));
        for _ in 0..40 {
            layout.step(-1., WIDE);
        }
        assert_eq!(shown(&layout), (88., true));
        layout.step(-1., WIDE);
        assert_eq!(shown(&layout), (88., true));
        layout.step(1., WIDE);
        assert_eq!(shown(&layout), (128., false));
        layout.reset();
        assert_eq!(shown(&layout), (280., false));
    }
}
