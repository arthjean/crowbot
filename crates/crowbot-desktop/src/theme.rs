use crowbot_protocol::Appearance;
use gpui::{
    App, BoxShadow, Font, FontFallbacks, Global, Pixels, Rgba, WindowAppearance, point, px, rgb,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub frame: Rgba,
    pub canvas: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub surface_raised: Rgba,
    pub control: Rgba,
    pub control_hover: Rgba,
    pub control_active: Rgba,
    pub sheet: Rgba,
    pub overlay: Rgba,
    pub dialog_ring: Rgba,
    pub border: Rgba,
    pub border_strong: Rgba,
    pub border_focus: Rgba,
    pub hover: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub text_dim: Rgba,
    pub text_readable: Rgba,
    pub text_placeholder: Rgba,
    pub inverse_surface: Rgba,
    pub inverse_text: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_text: Rgba,
    pub accent_soft: Rgba,
    pub accent_strong: Rgba,
    pub on_accent: Rgba,
    pub success: Rgba,
    pub success_text: Rgba,
    pub warning: Rgba,
    pub warning_text: Rgba,
    pub warning_soft: Rgba,
    pub danger: Rgba,
    pub danger_hover: Rgba,
    pub danger_text: Rgba,
    pub danger_soft: Rgba,
    pub danger_soft_hover: Rgba,
    pub unread_surface: Rgba,
    pub unread_text: Rgba,
    pub on_status: Rgba,
    pub shadow_menu: Rgba,
    pub window_close: Rgba,
    /// A menu or a select popup: one step above `surface`.
    pub menu_surface: Rgba,
    /// A row under the pointer or the keyboard in a menu.
    pub menu_highlight: Rgba,
    /// The chosen row of a list in a menu.
    pub menu_selected: Rgba,
    /// A line between groups of a menu.
    pub menu_separator: Rgba,
    /// The resting edge of a text field, on the `control` fill.
    pub input_edge: Rgba,
}

impl Global for Theme {}

impl Theme {
    pub fn resolve(preference: Appearance, system: WindowAppearance) -> Self {
        match preference {
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
            Appearance::System => match system {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
                WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            },
        }
    }

    pub fn of(cx: &App) -> Self {
        cx.try_global::<Self>()
            .copied()
            .unwrap_or_else(|| Self::resolve(Appearance::System, cx.window_appearance()))
    }

    pub fn dark() -> Self {
        let white = |alpha| alpha_of(rgb(0xff_ff_ff), alpha);
        let black = |alpha| alpha_of(rgb(0x00_00_00), alpha);
        let text_muted = rgb(0x97_97_97);
        Self {
            frame: rgb(0x0d_0d_0d),
            canvas: rgb(0x1a_1a_1a),
            sidebar: rgb(0x1a_1a_1a),
            surface: rgb(0x21_21_21),
            surface_raised: rgb(0x24_24_24),
            control: rgb(0x21_21_21),
            control_hover: rgb(0x24_24_24),
            control_active: rgb(0x2a_2a_2a),
            sheet: rgb(0x14_14_14),
            overlay: black(0.1),
            dialog_ring: white(0.1),
            border: white(0.035),
            border_strong: white(0.045),
            border_focus: text_muted,
            hover: white(0.06),
            text: rgb(0xff_ff_ff),
            text_secondary: rgb(0xdc_dc_dc),
            text_muted,
            text_dim: rgb(0x6a_6a_6a),
            text_readable: white(0.52),
            text_placeholder: rgb(0xbf_bf_bf),
            inverse_surface: rgb(0xf0_f0_f0),
            inverse_text: rgb(0x14_14_14),
            accent: rgb(0x00_7c_f7),
            accent_hover: rgb(0x1f_8f_ff),
            accent_text: rgb(0x42_a0_ff),
            accent_soft: alpha_of(rgb(0x00_7c_f7), 0.16),
            accent_strong: alpha_of(rgb(0x00_7c_f7), 0.34),
            on_accent: rgb(0xff_ff_ff),
            success: rgb(0x31_cf_76),
            success_text: rgb(0x31_cf_76),
            warning: rgb(0xff_94_12),
            warning_text: rgb(0xff_94_12),
            warning_soft: alpha_of(rgb(0xff_94_12), 0.12),
            danger: rgb(0xfd_2f_3b),
            danger_hover: rgb(0xfd_5d_67),
            danger_text: rgb(0xff_60_69),
            danger_soft: alpha_of(rgb(0xfd_2f_3b), 0.16),
            danger_soft_hover: alpha_of(rgb(0xfd_2f_3b), 0.28),
            unread_surface: rgb(0x74_b9_ff),
            unread_text: rgb(0x07_13_1f),
            on_status: rgb(0x07_13_0c),
            shadow_menu: black(0.8),
            window_close: rgb(0xe8_11_20),
            menu_surface: rgb(0x2a_2a_2a),
            menu_highlight: white(0.06),
            menu_selected: white(0.1),
            menu_separator: white(0.08),
            input_edge: white(0.045),
        }
    }

    pub fn light() -> Self {
        let ink = |alpha| alpha_of(rgb(0x14_14_14), alpha);
        let text_muted = rgb(0x6b_6b_70);
        Self {
            frame: rgb(0xe8_e8_eb),
            canvas: rgb(0xf5_f5_f6),
            sidebar: rgb(0xf5_f5_f6),
            surface: rgb(0xff_ff_ff),
            surface_raised: rgb(0xfc_fc_fc),
            control: rgb(0xff_ff_ff),
            control_hover: rgb(0xf0_f0_f2),
            control_active: rgb(0xe8_e8_eb),
            sheet: rgb(0xff_ff_ff),
            overlay: ink(0.12),
            dialog_ring: ink(0.1),
            border: ink(0.07),
            border_strong: ink(0.1),
            border_focus: text_muted,
            hover: ink(0.05),
            text: rgb(0x14_14_14),
            text_secondary: rgb(0x3a_3a_3c),
            text_muted,
            text_dim: rgb(0x8e_8e_93),
            text_readable: ink(0.6),
            text_placeholder: rgb(0x5c_5c_61),
            inverse_surface: rgb(0x1c_1c_1e),
            inverse_text: rgb(0xff_ff_ff),
            accent: rgb(0x00_7c_f7),
            accent_hover: rgb(0x00_6a_d4),
            accent_text: rgb(0x00_64_c8),
            accent_soft: alpha_of(rgb(0x00_7c_f7), 0.12),
            accent_strong: alpha_of(rgb(0x00_7c_f7), 0.26),
            on_accent: rgb(0xff_ff_ff),
            success: rgb(0x1f_a8_5a),
            success_text: rgb(0x18_7a_41),
            warning: rgb(0xf0_8a_00),
            warning_text: rgb(0x9c_52_00),
            warning_soft: alpha_of(rgb(0xf0_8a_00), 0.14),
            danger: rgb(0xe8_22_2d),
            danger_hover: rgb(0xf0_4a_53),
            danger_text: rgb(0xc0_14_1d),
            danger_soft: alpha_of(rgb(0xe8_22_2d), 0.12),
            danger_soft_hover: alpha_of(rgb(0xe8_22_2d), 0.2),
            unread_surface: rgb(0x74_b9_ff),
            unread_text: rgb(0x07_13_1f),
            on_status: rgb(0x07_13_0c),
            shadow_menu: alpha_of(rgb(0x00_00_00), 0.55),
            window_close: rgb(0xe8_11_20),
            menu_surface: rgb(0xff_ff_ff),
            menu_highlight: ink(0.06),
            menu_selected: ink(0.1),
            menu_separator: ink(0.08),
            input_edge: ink(0.1),
        }
    }

    pub fn menu_shadow(&self) -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: self.dialog_ring.into(),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: size::HAIRLINE,
                inset: false,
            },
            BoxShadow {
                color: self.shadow_menu.into(),
                offset: point(px(0.), menu::SHADOW_DROP),
                blur_radius: menu::SHADOW_BLUR,
                spread_radius: -menu::SHADOW_DROP,
                inset: false,
            },
        ]
    }
}

fn alpha_of(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

pub mod font {
    use super::{Font, FontFallbacks};

    pub const SANS: &str = "Inter";
    pub const SANS_FALLBACKS: [&str; 3] = [".SystemUIFont", "Segoe UI", "Noto Sans Arabic"];
    #[cfg(target_os = "windows")]
    pub const MONO: &str = "Consolas";
    #[cfg(target_os = "macos")]
    pub const MONO: &str = "Menlo";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub const MONO: &str = "monospace";
    pub const MONO_FALLBACKS: [&str; 4] = ["SF Mono", "Menlo", "Consolas", "monospace"];
    pub const WINDOW_GLYPHS: &str = "Segoe Fluent Icons";
    pub const WINDOW_GLYPHS_FALLBACK: &str = "Segoe MDL2 Assets";

    fn with_fallbacks(family: &'static str, fallbacks: &[&str]) -> Font {
        let mut font = gpui::font(family);
        font.fallbacks = Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|name| (*name).to_owned()).collect(),
        ));
        font
    }

    pub fn sans() -> Font {
        with_fallbacks(SANS, &SANS_FALLBACKS)
    }

    pub fn mono() -> Font {
        with_fallbacks(MONO, &MONO_FALLBACKS)
    }

    pub fn window_glyphs() -> Font {
        with_fallbacks(WINDOW_GLYPHS, &[WINDOW_GLYPHS_FALLBACK])
    }
}

pub mod text {
    use super::{Pixels, px};

    #[derive(Clone, Copy)]
    pub struct Step {
        pub size: Pixels,
        pub leading: Pixels,
    }

    pub const SM: Step = Step {
        size: px(12.),
        leading: px(16.),
    };
    pub const MD: Step = Step {
        size: px(13.),
        leading: px(18.),
    };
    pub const LG: Step = Step {
        size: px(14.),
        leading: px(20.),
    };
    pub const TITLE: Step = Step {
        size: px(15.),
        leading: px(20.),
    };
    pub const XL: Step = Step {
        size: px(16.),
        leading: px(22.),
    };
    pub const XXL: Step = Step {
        size: px(18.),
        leading: px(24.),
    };
    pub const XXXL: Step = Step {
        size: px(24.),
        leading: px(30.),
    };
    /// The text of a form field, as the creation form shows it.
    pub const FIELD: Step = Step {
        size: px(16.),
        leading: px(22.),
    };
    /// The text of a form area: one more step of leading than a field.
    pub const AREA: Step = Step {
        size: px(16.),
        leading: px(24.),
    };
    pub const CODE: Step = Step {
        size: px(12.),
        leading: px(19.),
    };
    pub const WINDOW_GLYPH: Pixels = px(10.);
}

pub mod space {
    use super::{Pixels, px};

    pub const S0_5: Pixels = px(2.);
    pub const S1: Pixels = px(4.);
    pub const S1_5: Pixels = px(6.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S6: Pixels = px(24.);
    pub const S8: Pixels = px(32.);
}

pub mod radius {
    use super::{Pixels, px};

    pub const SM: Pixels = px(6.);
    pub const MD: Pixels = px(8.);
    pub const LG: Pixels = px(12.);
    pub const CARD: Pixels = px(14.);
    pub const XL: Pixels = px(20.);
    pub const BUBBLE: Pixels = px(22.);
}

/// The menu contract: every menu, select popup, and picker shares it.
pub mod menu {
    use super::{Pixels, px};

    /// A plain menu: radius 8 around 4 of padding holds rows of radius 4.
    pub const RADIUS: Pixels = px(8.);
    pub const PADDING: Pixels = px(4.);
    /// A wide picker: radius 14 around 6 of padding holds rows and panels
    /// of radius 8.
    pub const WIDE_RADIUS: Pixels = px(14.);
    pub const WIDE_PADDING: Pixels = px(6.);
    pub const ROW_HEIGHT: Pixels = px(28.);
    pub const ROW_RADIUS: Pixels = px(4.);
    pub const WIDE_ROW_RADIUS: Pixels = px(8.);
    pub const ROW_INSET: Pixels = px(8.);
    pub const ROW_GAP: Pixels = px(8.);
    pub const CHECK: Pixels = px(14.);
    pub const SEPARATOR_MARGIN: Pixels = px(4.);
    pub const MAX_HEIGHT: Pixels = px(400.);
    pub const SHADOW_DROP: Pixels = px(18.);
    pub const SHADOW_BLUR: Pixels = px(44.);
    pub const REVEAL_DROP: Pixels = px(4.);
    pub const DISABLED_OPACITY: f32 = 0.64;
}

pub mod control {
    use super::{Pixels, px};

    pub const XS: Pixels = px(24.);
    pub const SM: Pixels = px(28.);
    pub const MD: Pixels = px(32.);
    pub const LG: Pixels = px(36.);
    pub const XL: Pixels = px(40.);
}

pub mod icon {
    use super::{Pixels, px};

    pub const XS: Pixels = px(12.);
    pub const SM: Pixels = px(14.);
    pub const MD: Pixels = px(16.);
    pub const LG: Pixels = px(20.);
}

pub mod duration {
    use std::time::Duration;

    pub const SPINNER: Duration = Duration::from_millis(900);
    pub const WORKING: Duration = Duration::from_millis(960);
    pub const WORKING_RISE: f32 = 0.4;
    pub const WORKING_STAGGER: f32 = 0.135;
    pub const COPIED: Duration = Duration::from_secs(2);
    pub const SHINE: Duration = Duration::from_millis(2250);
    pub const DRAFT_SAVE: Duration = Duration::from_millis(400);
    pub const FIELD_SAVE: Duration = Duration::from_millis(600);
    pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
    pub const MENU_REVEAL: Duration = Duration::from_millis(140);
    pub const FLASH: Duration = Duration::from_millis(1600);
    pub const QUIT_FLUSH: Duration = Duration::from_millis(60);
    /// The edge fades of the suggestion row.
    pub const FADE: Duration = Duration::from_millis(160);
    /// One frame at 60 Hz: the unit of the momentum of a dragged row.
    pub const FRAME: Duration = Duration::from_micros(16_667);
}

/// The physics of the suggestion row: a drag hands its speed, in pixels per
/// millisecond, to a glide that loses `FRICTION` of it each 60 Hz frame.
pub mod motion {
    pub const FRICTION: f32 = 0.88;
    pub const MIN_SPEED: f32 = 0.01;
    pub const MAX_SPEED: f32 = 1.75;
    /// How much of the last speed a new pointer sample keeps.
    pub const SPEED_MEMORY: f32 = 0.65;
}

pub mod opacity {
    pub const DISABLED: f32 = 0.42;
    /// The primary button under the pointer: its fill over the canvas.
    pub const PRIMARY_HOVER: f32 = 0.82;
    pub const SHINE_LOW: f32 = 0.55;
    pub const HIDDEN: f32 = 0.;
    pub const SHOWN: f32 = 1.;
}

pub mod size {
    use super::{Pixels, px};

    pub const HAIRLINE: Pixels = px(1.);
    pub const CURSOR: Pixels = px(1.5);
    pub const SELECTION_OVERHANG: Pixels = px(4.);
    pub const AVATAR_ROW: Pixels = px(46.);
    pub const AVATAR_TILE: Pixels = px(56.);
    pub const AVATAR_TILE_COMPACT: Pixels = px(48.);
    pub const AVATAR_HEADER: Pixels = px(18.);
    pub const AVATAR_HEADER_GLYPH: Pixels = px(10.);
    pub const AVATAR_RING: Pixels = px(2.);
    pub const BADGE: Pixels = px(18.);
    pub const BADGE_WORKING_WIDTH: Pixels = px(30.);
    pub const BADGE_DOT: Pixels = px(5.);
    pub const BADGE_HOP: Pixels = px(3.);
    pub const BADGE_OFFSET_X: Pixels = px(5.);
    pub const BADGE_OFFSET_Y: Pixels = px(3.);
    pub const BADGE_ICON: Pixels = px(10.);
    pub const CHIP_ICON: Pixels = px(11.);
    pub const CHIP_HEIGHT: Pixels = px(18.);
    pub const CHIP_PADDING_START: Pixels = px(5.);
    pub const CHIP_PADDING_END: Pixels = px(7.);
    pub const CHIP_GAP: Pixels = px(3.);
    pub const COUNT_BADGE: Pixels = px(16.);
    pub const BADGE_PILL: Pixels = px(22.);
    pub const DIVIDER_HEIGHT: Pixels = px(20.);
    pub const AVATAR_MESSAGE: Pixels = px(32.);
    pub const AVATAR_MESSAGE_GLYPH: Pixels = px(14.);
    pub const AVATAR_EMPTY: Pixels = px(64.);
    pub const AVATAR_EMPTY_GLYPH: Pixels = px(28.);
    pub const QUOTE_BAR: Pixels = px(2.);
    pub const CHECKBOX: Pixels = px(14.);
    pub const JUMP: Pixels = px(44.);
    pub const SEND: Pixels = px(28.);
    pub const ACTION_ICON: Pixels = px(17.);
    pub const AVATAR_CREATION: Pixels = px(72.);
    pub const AVATAR_CREATION_GLYPH: Pixels = px(32.);
    pub const AVATAR_TEMPLATE: Pixels = px(42.);
    pub const AVATAR_TEMPLATE_GLYPH: Pixels = px(18.);
    pub const AVATAR_FACE: Pixels = px(24.);
    pub const AVATAR_FACE_GLYPH: Pixels = px(13.);
    /// The live avatar of the creation form rises this far above its box.
    pub const AVATAR_CREATION_LIFT: Pixels = px(8.);
    pub const AVATAR_PANEL: Pixels = px(62.);
    pub const AVATAR_PANEL_GLYPH: Pixels = px(26.);
    pub const AVATAR_RESULT: Pixels = px(20.);
    pub const AVATAR_RESULT_GLYPH: Pixels = px(11.);
    pub const SWATCH_BUTTON: Pixels = px(28.);
    pub const SWATCH: Pixels = px(18.);
    pub const GLYPH_CELL: Pixels = px(33.);
    pub const STATE_DOT: Pixels = px(8.);
    pub const FOCUS_RING: Pixels = px(3.);
    pub const RING: Pixels = px(2.);
    pub const PROVIDER_RING: Pixels = px(28.);
    pub const RAIL_SEPARATOR: Pixels = px(24.);
}

pub mod layout {
    use super::{Pixels, px};

    pub const TOP_ROW: Pixels = px(38.);
    pub const RAIL_WIDTH: Pixels = px(52.);
    pub const RAIL_LOGO: Pixels = px(20.);
    pub const SIDEBAR_DEFAULT: f32 = 280.;
    pub const SIDEBAR_MIN: f32 = 128.;
    pub const SIDEBAR_MAX: f32 = 400.;
    pub const SIDEBAR_COMPACT: f32 = 88.;
    pub const SIDEBAR_COLLAPSE_BELOW: f32 = 110.;
    pub const SIDEBAR_EXPAND_ABOVE: f32 = 118.;
    pub const SIDEBAR_KEY_STEP: f32 = 12.;
    pub const CONVERSATION_MIN: f32 = 424.;
    pub const RESIZER_HIT: Pixels = px(16.);
    pub const CARD_INSET: Pixels = px(4.);
    pub const WINDOW_WIDTH: Pixels = px(1080.);
    pub const WINDOW_HEIGHT: Pixels = px(760.);
    pub const WINDOW_MIN_WIDTH: Pixels = px(640.);
    pub const WINDOW_MIN_HEIGHT: Pixels = px(420.);
    pub const CAPTION_BUTTON: Pixels = px(46.);
    pub const MAC_TRAFFIC_LIGHTS: Pixels = px(78.);
    pub const TOPBAR_PADDING_START: Pixels = px(10.);
    pub const SEARCH_PADDING_TOP: Pixels = px(11.);
    pub const SEARCH_FIELD_PADDING: Pixels = px(9.);
    pub const ROW_HEIGHT: Pixels = px(62.);
    pub const TILE_WIDTH: Pixels = px(72.);
    pub const TILE_HEIGHT: Pixels = px(102.);
    pub const TILE_COMPACT: Pixels = px(60.);
    pub const PINNED_MAX_WIDTH: Pixels = px(236.);
    pub const SECTION_HEADER_PADDING: Pixels = px(7.);
    pub const HEADER_PADDING_START: Pixels = px(12.);
    pub const HEADER_TITLE_MIN: Pixels = px(52.);
    pub const HEADER_NAME_MAX: Pixels = px(160.);
    pub const HEADER_MODEL_MAX: Pixels = px(200.);
    pub const HEADER_TITLE_SHRINK: f32 = 1000.;
    pub const HEADER_CONTROLS_SHRINK: f32 = 10.;
    pub const BADGE_HEIGHT: Pixels = px(22.);
    pub const DIVIDER_MARGIN: Pixels = px(9.);
    pub const SIDE_PANEL: Pixels = px(360.);
    pub const SIDE_PANEL_HEADER: Pixels = px(39.);
    pub const MENU_MIN_WIDTH: Pixels = px(192.);
    pub const MENU_MAX_WIDTH: Pixels = px(288.);
    pub const DIALOG_WIDTH: Pixels = px(460.);
    pub const NOTICE_WIDTH: Pixels = px(520.);
    pub const TOAST_WIDTH: Pixels = px(360.);
    pub const TOOLTIP_PADDING_Y: Pixels = px(6.);
    pub const TOOLTIP_MAX_WIDTH: Pixels = px(260.);
    pub const COLUMN_WIDTH: Pixels = px(760.);
    pub const BUBBLE_MAX_WIDTH: Pixels = px(608.);
    pub const BUBBLE_PADDING_X: Pixels = px(12.);
    pub const BUBBLE_PADDING_Y: Pixels = px(8.);
    pub const AVATAR_GUTTER: Pixels = px(36.);
    pub const ROW_GAP: Pixels = px(12.);
    pub const ROW_GAP_GROUPED: Pixels = px(4.);
    pub const DAY_ROW: Pixels = px(24.);
    pub const ACTION_ROW: Pixels = px(24.);
    pub const STICK_THRESHOLD: f32 = 80.;
    pub const JUMP_BOTTOM: Pixels = px(14.);
    pub const BLOCK_GAP: Pixels = px(12.);
    pub const LIST_INDENT: Pixels = px(24.);
    pub const CODE_HEADER: Pixels = px(34.);
    pub const CODE_MAX_HEIGHT: Pixels = px(420.);
    pub const TABLE_MIN_COLUMN: Pixels = px(72.);
    pub const STEPS_MAX_HEIGHT: Pixels = px(220.);
    pub const STEP_ROW: Pixels = px(24.);
    pub const STEP_ERROR_MAX_HEIGHT: Pixels = px(160.);
    pub const APPROVAL_WIDTH: Pixels = px(540.);
    pub const APPROVAL_TARGET_MAX_HEIGHT: Pixels = px(160.);
    pub const QUEUE_ROW: Pixels = px(29.);
    pub const QUEUE_MAX_HEIGHT: Pixels = px(132.);
    pub const LIST_OVERDRAW: Pixels = px(600.);
    pub const COMPOSER_PADDING_Y: Pixels = px(5.);
    pub const COMPOSER_BOTTOM: Pixels = px(20.);
    pub const STATUS_ROW: Pixels = px(20.);
    pub const COMPOSER_MAX_ROWS: f32 = 8.;
    pub const FIELD_HEIGHT: Pixels = px(32.);
    pub const FIELD_PADDING_Y: Pixels = px(5.);
    /// The vertical padding of a form field, which holds `text::FIELD`.
    pub const FORM_FIELD_PADDING_Y: Pixels = px(4.);
    pub const CREATION_WIDTH: Pixels = px(420.);
    pub const CREATE_MIN_WIDTH: Pixels = px(132.);
    pub const PURPOSE_MIN_ROWS: f32 = 2.;
    pub const PURPOSE_MAX_ROWS: f32 = 9.;
    pub const MODEL_ROW: Pixels = px(44.);
    pub const PICKER_PROVIDERS: Pixels = px(136.);
    pub const TEMPLATE_WIDTH: Pixels = px(256.);
    pub const TEMPLATE_HEIGHT: Pixels = px(112.);
    /// The suggestion row fades over this width at an edge it can scroll past.
    pub const TEMPLATE_FADE: Pixels = px(48.);
    /// A press on the suggestion row turns into a drag past this distance.
    pub const DRAG_THRESHOLD: Pixels = px(8.);
    /// A row this close to an end has reached it.
    pub const SCROLL_SLACK: Pixels = px(1.);
    /// A glide that moves the row less than this in a frame has stopped.
    pub const SCROLL_STILL: Pixels = px(0.1);
    /// The swatches of an avatar color stand in rows of this many.
    pub const SWATCH_COLUMNS: usize = 9;
    /// The creation form keeps this much room on each side in a narrow pane.
    pub const CREATION_MARGIN: Pixels = px(24.);
    pub const PANEL_PADDING_X: Pixels = px(15.);
    pub const PANEL_PADDING_BOTTOM: Pixels = px(28.);
    pub const PANEL_ROW: Pixels = px(40.);
    pub const INSTRUCTIONS_MIN_ROWS: f32 = 3.;
    pub const INSTRUCTIONS_MAX_ROWS: f32 = 12.;
    pub const SETTINGS_WIDTH: Pixels = px(760.);
    pub const SETTINGS_HEIGHT: Pixels = px(520.);
    pub const SETTINGS_NAV: Pixels = px(196.);
    pub const SETTINGS_ROW: Pixels = px(64.);
    pub const PALETTE_TOP: Pixels = px(112.);
    pub const PALETTE_WIDTH: Pixels = px(640.);
    pub const PALETTE_INPUT: Pixels = px(48.);
    pub const PALETTE_RESULTS: Pixels = px(400.);
    pub const PALETTE_ROW: Pixels = px(36.);
    pub const PALETTE_HEADING: Pixels = px(28.);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn over(top: Rgba, under: Rgba) -> Rgba {
        let mix = |top_channel: f32, under_channel: f32| {
            top_channel * top.a + under_channel * (1. - top.a)
        };
        Rgba {
            r: mix(top.r, under.r),
            g: mix(top.g, under.g),
            b: mix(top.b, under.b),
            a: 1.,
        }
    }

    fn luminance(color: Rgba) -> f32 {
        let linear = |channel: f32| {
            if channel <= 0.040_45 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(text: Rgba, surface: Rgba) -> f32 {
        let (first, second) = (luminance(over(text, surface)), luminance(surface));
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    fn hex(color: Rgba) -> String {
        let channel = |value: f32| (value * 255.).round() as u8;
        let solid = format!(
            "#{:02x}{:02x}{:02x}",
            channel(color.r),
            channel(color.g),
            channel(color.b)
        );
        if color.a >= 1. {
            solid
        } else {
            format!("{solid}@{:.3}", color.a)
        }
    }

    fn openbot_tokens(theme: &Theme) -> Vec<(&'static str, Rgba)> {
        vec![
            ("--openbot-bg-frame", theme.frame),
            ("--openbot-bg-canvas", theme.canvas),
            ("--openbot-bg-sidebar", theme.sidebar),
            ("--openbot-bg-surface", theme.surface),
            ("--openbot-bg-surface-raised", theme.surface_raised),
            ("--openbot-bg-control", theme.control),
            ("--openbot-bg-control-hover", theme.control_hover),
            ("--openbot-bg-control-active", theme.control_active),
            ("--openbot-bg-sheet", theme.sheet),
            ("--openbot-dialog-overlay", theme.overlay),
            ("--openbot-dialog-ring", theme.dialog_ring),
            ("--openbot-border", theme.border),
            ("--openbot-border-strong", theme.border_strong),
            ("--openbot-hover", theme.hover),
            ("--openbot-text-primary", theme.text),
            ("--openbot-text-secondary", theme.text_secondary),
            ("--openbot-text-muted", theme.text_muted),
            ("--openbot-text-dim", theme.text_dim),
            ("--openbot-text-readable", theme.text_readable),
            ("--openbot-text-placeholder", theme.text_placeholder),
            ("--openbot-surface-light", theme.inverse_surface),
            ("--openbot-text-on-light", theme.inverse_text),
            ("--openbot-accent", theme.accent),
            ("--openbot-accent-hover", theme.accent_hover),
            ("--openbot-accent-text", theme.accent_text),
            ("--openbot-accent-soft", theme.accent_soft),
            ("--openbot-accent-strong", theme.accent_strong),
            ("--openbot-text-on-accent", theme.on_accent),
            ("--openbot-success", theme.success),
            ("--openbot-success-text", theme.success_text),
            ("--openbot-warning", theme.warning),
            ("--openbot-warning-text", theme.warning_text),
            ("--openbot-warning-soft", theme.warning_soft),
            ("--openbot-danger", theme.danger),
            ("--openbot-danger-hover", theme.danger_hover),
            ("--openbot-danger-text", theme.danger_text),
            ("--openbot-danger-soft", theme.danger_soft),
            ("--openbot-danger-soft-hover", theme.danger_soft_hover),
            ("--openbot-unread-surface", theme.unread_surface),
            ("--openbot-unread-text", theme.unread_text),
            ("--openbot-text-on-status", theme.on_status),
            ("--openbot-shadow-menu", theme.shadow_menu),
        ]
    }

    fn css_color(value: &str) -> Option<Rgba> {
        let value = value.trim();
        if value == "transparent" {
            return Some(alpha_of(rgb(0), 0.));
        }
        if let Some(digits) = value.strip_prefix('#') {
            return u32::from_str_radix(digits, 16).ok().map(rgb);
        }
        let numbers: Vec<f32> = value
            .strip_prefix("rgba(")?
            .strip_suffix(')')?
            .split(',')
            .map(|part| part.trim().parse().ok())
            .collect::<Option<_>>()?;
        match numbers[..] {
            [red, green, blue, alpha] => Some(Rgba {
                r: red / 255.,
                g: green / 255.,
                b: blue / 255.,
                a: alpha,
            }),
            _ => None,
        }
    }

    fn css_declarations(css: &str) -> std::collections::HashMap<String, String> {
        css.lines()
            .filter_map(|line| {
                let (name, value) = line.trim().strip_suffix(';')?.split_once(':')?;
                name.starts_with("--openbot-")
                    .then(|| (name.to_owned(), value.trim().to_owned()))
            })
            .collect()
    }

    #[test]
    #[ignore = "needs the OpenBot repository: set OPENBOT_DIR"]
    fn the_color_tokens_have_the_values_of_the_two_openbot_token_files() {
        let openbot = std::path::PathBuf::from(
            std::env::var("OPENBOT_DIR").expect("OPENBOT_DIR is the OpenBot repository"),
        )
        .join("packages/brand/src");
        let dark = css_declarations(&std::fs::read_to_string(openbot.join("tokens.css")).unwrap());
        let mut light = dark.clone();
        light.extend(css_declarations(
            &std::fs::read_to_string(openbot.join("tokens-light.css")).unwrap(),
        ));
        let mut compared = 0;
        for (declared, theme) in [(&dark, Theme::dark()), (&light, Theme::light())] {
            for (name, ours) in openbot_tokens(&theme) {
                let theirs = declared
                    .get(name)
                    .and_then(|value| css_color(value))
                    .unwrap_or_else(|| panic!("{name} has no color in the OpenBot file"));
                assert_eq!(hex(ours), hex(theirs), "{name}");
                compared += 1;
            }
            let focus = declared.get("--openbot-border-focus").unwrap();
            assert_eq!(focus, "var(--openbot-text-muted)");
            assert_eq!(hex(theme.border_focus), hex(theme.text_muted));
        }
        assert_eq!(compared, 84);
    }

    const VIEWS: [&str; 19] = [
        "main.rs",
        "composer.rs",
        "ui.rs",
        "markdown/element.rs",
        "workspace.rs",
        "workspace/bot_list.rs",
        "workspace/header.rs",
        "workspace/conversation.rs",
        "workspace/rows.rs",
        "workspace/steps.rs",
        "workspace/compose.rs",
        "workspace/overlays.rs",
        "workspace/screens.rs",
        "workspace/forms.rs",
        "workspace/creation.rs",
        "workspace/bot_panel.rs",
        "workspace/settings.rs",
        "workspace/palette.rs",
        "workspace/rail.rs",
    ];

    fn literal_values(source: &str) -> Vec<String> {
        let source = source.replace("\r\n", "\n");
        let code = source
            .split(
                "#[cfg(test)]
mod tests {",
            )
            .next()
            .unwrap_or_default();
        let mut found = Vec::new();
        for (line_number, line) in code.lines().enumerate() {
            let mut report = |what: &str| found.push(format!("line {}: {what}", line_number + 1));
            for (at, _) in line.match_indices("px(") {
                let method = line[..at].chars().next_back().is_some_and(|before| {
                    before == '.' || before == '_' || before.is_alphanumeric()
                });
                let argument = &line[at + 3..];
                let literal = argument.starts_with(|first: char| first.is_ascii_digit());
                if !method && literal && !argument.starts_with("0.)") {
                    report("a size in pixels");
                }
            }
            for color in [
                "rgb(",
                "rgba(",
                "hsla(",
                "hsl(",
                "Rgba {",
                "Hsla {",
                "gpui::white(",
                "gpui::black(",
                "gpui::red(",
                "gpui::green(",
                "gpui::blue(",
                "gpui::yellow(",
            ] {
                if line.contains(color) {
                    report("a color");
                }
            }
            for (at, _) in line.match_indices("opacity(") {
                if line[at + 8..].starts_with(|first: char| first.is_ascii_digit()) {
                    report("an opacity");
                }
            }
            if line.contains("rems(") {
                report("a size in rems");
            }
            if line.contains("Duration::from") || line.contains("from_millis(") {
                report("a duration");
            }
        }
        found
    }

    #[test]
    fn the_views_hold_no_literal_color_size_or_duration() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for view in VIEWS {
            let text = std::fs::read_to_string(source.join(view)).unwrap();
            assert_eq!(literal_values(&text), Vec::<String>::new(), "{view}");
        }
        let seeded = "div().w(px(12.)).bg(rgb(0x00_00_00)).opacity(0.5).h(px(0.)).px(space::S1)";
        assert_eq!(literal_values(seeded).len(), 3);
        assert_eq!(
            literal_values("with_animation(Duration::from_millis(5))").len(),
            1
        );
        // A checkout with CRLF line ends still stops at the tests.
        let tested = "div()\r\n#[cfg(test)]\r\nmod tests {\r\n    px(4.)\r\n}\r\n";
        assert_eq!(literal_values(tested), Vec::<String>::new());
    }

    #[test]
    fn the_appearance_preference_wins_over_the_system_and_system_follows_it() {
        let (dark, light) = (WindowAppearance::Dark, WindowAppearance::Light);
        assert_eq!(Theme::resolve(Appearance::Light, dark), Theme::light());
        assert_eq!(Theme::resolve(Appearance::Dark, light), Theme::dark());
        assert_eq!(Theme::resolve(Appearance::System, dark), Theme::dark());
        assert_eq!(Theme::resolve(Appearance::System, light), Theme::light());
        assert_eq!(
            Theme::resolve(Appearance::System, WindowAppearance::VibrantDark),
            Theme::dark()
        );
        assert_eq!(
            Theme::resolve(Appearance::default(), WindowAppearance::VibrantLight),
            Theme::light()
        );
    }

    #[test]
    fn the_interface_font_is_inter_then_the_system_stack() {
        let sans = font::sans();
        assert_eq!(sans.family.as_ref(), "Inter");
        let fallbacks = sans.fallbacks.unwrap();
        assert_eq!(fallbacks.fallback_list()[0], ".SystemUIFont");
        let mono = font::mono();
        assert!(["Consolas", "Menlo", "monospace"].contains(&mono.family.as_ref()));
    }

    #[test]
    fn text_meets_wcag_contrast_on_every_surface_it_sits_on() {
        for theme in [Theme::light(), Theme::dark()] {
            let sidebar_hover = over(theme.hover, theme.sidebar);
            for surface in [
                theme.frame,
                theme.canvas,
                theme.sidebar,
                theme.surface,
                theme.control_hover,
                theme.control_active,
                theme.sheet,
                sidebar_hover,
            ] {
                assert!(contrast(theme.text, surface) >= 4.5);
                assert!(contrast(theme.text_secondary, surface) >= 4.5);
            }
            for surface in [
                theme.canvas,
                theme.sidebar,
                theme.surface,
                theme.control_hover,
                theme.sheet,
            ] {
                assert!(
                    contrast(theme.text_muted, surface) >= 4.5,
                    "muted on {}",
                    hex(surface)
                );
            }
            for surface in [theme.canvas, theme.sidebar] {
                assert!(contrast(theme.text_readable, surface) >= 4.5);
            }
            for surface in [theme.canvas, theme.sidebar, theme.frame, sidebar_hover] {
                assert!(contrast(theme.text_placeholder, surface) >= 4.5);
            }
            let review_chip = over(theme.warning_soft, theme.sidebar);
            assert!(contrast(theme.warning_text, review_chip) >= 4.5);
            assert!(contrast(theme.warning_text, theme.canvas) >= 4.5);
            let next_model = over(theme.accent_soft, theme.frame);
            assert!(contrast(theme.text, next_model) >= 4.5);
            assert!(contrast(theme.accent_text, next_model) >= 3.);
            for surface in [theme.canvas, theme.surface, theme.sheet] {
                assert!(contrast(theme.danger_text, surface) >= 4.5);
                assert!(contrast(theme.success_text, surface) >= 4.5);
            }
            assert!(contrast(theme.inverse_text, theme.inverse_surface) >= 4.5);
            assert!(contrast(theme.unread_text, theme.unread_surface) >= 4.5);
            assert!(contrast(theme.on_status, theme.warning) >= 4.5);
            assert!(
                contrast(theme.danger_text, over(theme.danger_soft, theme.canvas)) >= 4.5,
                "danger text on its soft fill"
            );
        }
    }

    #[test]
    fn focus_rings_and_status_marks_meet_non_text_contrast() {
        for theme in [Theme::light(), Theme::dark()] {
            for surface in [theme.frame, theme.canvas, theme.sidebar, theme.sheet] {
                assert!(contrast(theme.border_focus, surface) >= 3.);
            }
            assert!(contrast(theme.text_muted, theme.frame) >= 3.);
            assert!(contrast(theme.danger, theme.sidebar) >= 3.);
            assert!(contrast(theme.on_accent, theme.danger) >= 3.);
        }
    }
}
