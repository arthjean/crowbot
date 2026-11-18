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
    /// The white of a mascot's eyes, the same in both appearances.
    pub mascot_eye: Rgba,
    pub mascot_pupil: Rgba,
    /// The beak of a mascot: a shade over the color of its head.
    pub mascot_beak: Rgba,
    pub success: Rgba,
    pub success_text: Rgba,
    pub success_soft: Rgba,
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
    /// The model picker: its panel, the column of its provider tabs, and the
    /// fill and edge of the tab shown.
    pub picker_surface: Rgba,
    pub picker_rail: Rgba,
    pub picker_tab: Rgba,
    pub picker_tab_edge: Rgba,
    /// The ring of a model that is not chosen.
    pub radio_edge: Rgba,
    /// The fill of a small pill beside a name, such as "default".
    pub badge: Rgba,
    /// The Claude logo beside its model, in Claude's own color.
    pub claude_mark: Rgba,
    /// A round icon button: its fill, its fill under the pointer, and its
    /// edge.
    pub round_button: Rgba,
    pub round_button_hover: Rgba,
    pub round_button_edge: Rgba,
    /// The search palette: the veil over the window behind it, and the fill
    /// of its row of key hints.
    pub palette_scrim: Rgba,
    pub palette_footer: Rgba,
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
            mascot_eye: rgb(0xff_ff_ff),
            mascot_pupil: rgb(0x17_17_1a),
            mascot_beak: alpha_of(rgb(0x00_00_00), mascot::BEAK_SHADE),
            success: rgb(0x31_cf_76),
            success_text: rgb(0x31_cf_76),
            success_soft: alpha_of(rgb(0x31_cf_76), 0.12),
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
            picker_surface: rgb(0x21_21_21),
            picker_rail: rgb(0x10_10_10),
            picker_tab: white(0.08),
            picker_tab_edge: white(0.11),
            radio_edge: white(0.22),
            badge: white(0.07),
            claude_mark: rgb(0xd9_77_57),
            round_button: rgb(0x26_26_26),
            round_button_hover: rgb(0x2e_2e_2e),
            round_button_edge: white(0.08),
            palette_scrim: alpha_of(rgb(0x0d_0d_0d), 0.64),
            palette_footer: white(0.025),
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
            mascot_eye: rgb(0xff_ff_ff),
            mascot_pupil: rgb(0x17_17_1a),
            mascot_beak: alpha_of(rgb(0x00_00_00), mascot::BEAK_SHADE),
            success: rgb(0x1f_a8_5a),
            success_text: rgb(0x18_7a_41),
            success_soft: alpha_of(rgb(0x1f_a8_5a), 0.08),
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
            picker_surface: rgb(0xff_ff_ff),
            picker_rail: rgb(0xf4_f4_f5),
            picker_tab: ink(0.06),
            picker_tab_edge: ink(0.1),
            radio_edge: ink(0.22),
            badge: ink(0.06),
            claude_mark: rgb(0xd9_77_57),
            round_button: rgb(0xf0_f0_f2),
            round_button_hover: rgb(0xe8_e8_eb),
            round_button_edge: ink(0.08),
            palette_scrim: alpha_of(rgb(0xe8_e8_eb), 0.6),
            palette_footer: ink(0.025),
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
            self.drop_shadow(),
        ]
    }

    /// The drop of the menu shadow alone, for a surface whose only edge is
    /// its own 1 px border, such as the search palette.
    pub fn drop_shadow(&self) -> BoxShadow {
        BoxShadow {
            color: self.shadow_menu.into(),
            offset: point(px(0.), menu::SHADOW_DROP),
            blur_radius: menu::SHADOW_BLUR,
            spread_radius: -menu::SHADOW_DROP,
            inset: false,
        }
    }
}

fn alpha_of(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

pub mod font {
    use std::sync::OnceLock;

    use gpui::{App, SharedString};

    use super::{Font, FontFallbacks};

    pub const SANS: &str = "Inter";
    /// Inter is embedded; these draw what it has no glyph for. On Linux they
    /// are the families Fedora and Ubuntu install, then the scripts Inter
    /// lacks. A family the system does not have is skipped.
    #[cfg(target_os = "linux")]
    pub const SANS_FALLBACKS: &[&str] = &[
        "Adwaita Sans",
        "Noto Sans",
        "Cantarell",
        "DejaVu Sans",
        "Noto Sans Arabic",
        "Noto Sans CJK SC",
        "Noto Color Emoji",
    ];
    #[cfg(not(target_os = "linux"))]
    pub const SANS_FALLBACKS: &[&str] = &[".SystemUIFont", "Segoe UI", "Noto Sans Arabic"];
    #[cfg(target_os = "windows")]
    pub const MONO: &str = "Consolas";
    #[cfg(target_os = "macos")]
    pub const MONO: &str = "Menlo";
    /// The monospaced family before [`init`] resolves the system's own. A
    /// family name must match a font exactly: `monospace` is a fontconfig
    /// alias that the text system does not know, so it cannot be named here.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub const MONO: &str = "DejaVu Sans Mono";
    /// The monospaced families tried in order after the one fontconfig
    /// resolves for `monospace`.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub const MONO_CANDIDATES: &[&str] = &[
        "Adwaita Mono",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
        "Ubuntu Mono",
        "Liberation Mono",
    ];
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub const MONO_FALLBACKS: &[&str] = &[
        "Adwaita Mono",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
        "Noto Sans CJK SC",
        "Noto Color Emoji",
        SANS,
    ];
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub const MONO_FALLBACKS: &[&str] = &["SF Mono", "Menlo", "Consolas", "monospace"];
    pub const WINDOW_GLYPHS: &str = "Segoe Fluent Icons";
    pub const WINDOW_GLYPHS_FALLBACK: &str = "Segoe MDL2 Assets";

    static MONO_FAMILY: OnceLock<SharedString> = OnceLock::new();
    fn with_fallbacks(family: impl Into<SharedString>, fallbacks: &[&str]) -> Font {
        let mut font = gpui::font(family);
        font.fallbacks = Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|name| (*name).to_owned()).collect(),
        ));
        font
    }

    pub fn sans() -> Font {
        with_fallbacks(SANS, SANS_FALLBACKS)
    }

    /// The monospaced family code is set in.
    pub fn mono_family() -> SharedString {
        MONO_FAMILY
            .get()
            .cloned()
            .unwrap_or(SharedString::new_static(MONO))
    }

    pub fn mono() -> Font {
        with_fallbacks(mono_family(), MONO_FALLBACKS)
    }

    pub fn window_glyphs() -> Font {
        with_fallbacks(WINDOW_GLYPHS, &[WINDOW_GLYPHS_FALLBACK])
    }

    /// Picks the monospaced family on Linux, once the embedded fonts are
    /// registered: the one fontconfig resolves for `monospace`, then the
    /// first of [`MONO_CANDIDATES`] the system has. With none, code is set in
    /// Inter, which is embedded: a family that resolves nowhere makes GPUI
    /// panic. Other systems keep [`MONO`].
    pub fn init(cx: &App) {
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let installed = cx.text_system().all_font_names();
            let family = resolve_mono(&installed, fontconfig_monospace());
            let _ = MONO_FAMILY.set(family);
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let _ = cx;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn fontconfig_monospace() -> Option<String> {
        let output = std::process::Command::new("fc-match")
            .args(["--format=%{family[0]}", "monospace"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let family = String::from_utf8(output.stdout).ok()?;
        (output.status.success() && !family.trim().is_empty()).then(|| family.trim().to_owned())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub fn resolve_mono(installed: &[String], fontconfig: Option<String>) -> SharedString {
        let has = |family: &str| installed.iter().any(|name| name == family);
        fontconfig
            .filter(|family| has(family))
            .map(SharedString::from)
            .or_else(|| {
                MONO_CANDIDATES
                    .iter()
                    .find(|family| has(family))
                    .map(|family| SharedString::new_static(family))
            })
            .unwrap_or(SharedString::new_static(SANS))
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
    pub const SHADOW_DROP: Pixels = px(18.);
    pub const SHADOW_BLUR: Pixels = px(44.);
    pub const REVEAL_DROP: Pixels = px(4.);
    pub const DISABLED_OPACITY: f32 = 0.64;
}

/// The model picker of the creation form: a column of provider tabs beside
/// the models of the tab shown.
pub mod picker {
    use super::{Pixels, px};

    pub const WIDTH: Pixels = px(360.);
    pub const MIN_HEIGHT: Pixels = px(226.);
    pub const MAX_HEIGHT: Pixels = px(520.);
    /// The picker leaves at least this much of the window height free.
    pub const WINDOW_ROOM: Pixels = px(100.);
    /// The space between the picker and its trigger.
    pub const GUTTER: Pixels = px(8.);
    pub const RADIUS: Pixels = px(14.);
    pub const PADDING: Pixels = px(6.);
    pub const ROW_RADIUS: Pixels = px(8.);
    /// Text in a row starts this far in from the row's edge.
    pub const INSET: Pixels = px(10.);
    pub const RAIL: Pixels = px(52.);
    pub const RAIL_PADDING_X: Pixels = px(7.);
    pub const RAIL_PADDING_Y: Pixels = px(9.);
    pub const TAB: Pixels = px(36.);
    /// The provider logo of a tab, and the one beside the model in the
    /// trigger.
    pub const TAB_LOGO: Pixels = px(19.);
    pub const TRIGGER_LOGO: Pixels = px(15.);
    pub const TAB_DOT: Pixels = px(6.);
    pub const TAB_DOT_INSET: Pixels = px(6.);
    /// The ring of background around the dot of a tab.
    pub const TAB_DOT_RING: Pixels = px(2.);
    pub const HEADING_TOP: Pixels = px(6.);
    pub const HEADING_BOTTOM: Pixels = px(8.);
    pub const ROW_PADDING_Y: Pixels = px(6.);
    pub const RADIO: Pixels = px(16.);
    pub const RADIO_CHECK: Pixels = px(12.);
    pub const NAME_GAP: Pixels = px(7.);
    pub const BADGE_PADDING_X: Pixels = px(6.);
    pub const BADGE_PADDING_Y: Pixels = px(1.);
    pub const BADGE_LEADING: Pixels = px(15.);
    pub const SELECT_WIDTH: Pixels = px(128.);
    /// How a tab mark shows: muted at rest, brighter under the pointer, full
    /// when its tab is shown; a provider that cannot run is dimmer.
    pub const MARK_REST: f32 = 0.48;
    pub const MARK_HOVER: f32 = 0.8;
    pub const MARK_OFF: f32 = 0.28;
    pub const MARK_OFF_SHOWN: f32 = 0.6;
}

/// The search palette: a field with no edge over groups of results, then a
/// row of key hints.
pub mod palette {
    use super::{Pixels, px};

    pub const WIDTH: Pixels = px(576.);
    pub const MAX_HEIGHT: Pixels = px(420.);
    /// The palette starts this share of the window height below its top, and
    /// keeps the same share free under it.
    pub const WINDOW_SHARE: f32 = 0.1;
    /// Radius 14 around 6 of padding holds rows of radius 8.
    pub const RADIUS: Pixels = px(14.);
    pub const PADDING: Pixels = px(6.);
    pub const ROW_RADIUS: Pixels = px(8.);
    pub const INPUT: Pixels = px(48.);
    /// The search icon, an avatar, or the icon of a result: the text of the
    /// field and of every row starts after it.
    pub const LEAD: Pixels = px(20.);
    pub const ROW_PADDING_Y: Pixels = px(6.);
    /// The most of a row that the text beside its title may take.
    pub const CONTEXT_SHARE: f32 = 0.45;
    pub const TIME_MIN: Pixels = px(48.);
    pub const STATUS_PADDING_Y: Pixels = px(40.);
    pub const FOOTER_PADDING_X: Pixels = px(16.);
    pub const FOOTER_PADDING_Y: Pixels = px(10.);
    pub const KEY: Pixels = px(20.);
    pub const KEY_RADIUS: Pixels = px(4.);
}

/// The settings page: a column of sections at the width of the sidebar,
/// beside groups of rows in a centered column.
pub mod settings {
    use super::{Pixels, px};

    /// The section column is as wide as the sidebar, and never narrower.
    pub const NAV_MIN: f32 = 220.;
    pub const NAV_PADDING_X: Pixels = px(12.);
    pub const NAV_PADDING_TOP: Pixels = px(16.);
    /// The title and the text of a search result start this far in from the
    /// edge of the section rows.
    pub const NAV_TEXT_INSET: Pixels = px(8.);
    pub const NAV_ROW: Pixels = px(32.);
    pub const NAV_ROW_GAP: Pixels = px(4.);
    pub const RESULT_PADDING_Y: Pixels = px(6.);
    /// The path in the title band starts this far after the section column.
    pub const PATH_INSET: Pixels = px(8.);
    /// The widest line of the groups.
    pub const COLUMN: Pixels = px(720.);
    /// A content pane narrower than this pads less, and one narrower than
    /// `STACK_BELOW` puts the control of a row under its text.
    pub const COMPACT_BELOW: f32 = 520.;
    pub const STACK_BELOW: f32 = 460.;
    pub const ROW_MIN: Pixels = px(64.);
    pub const SELECT_WIDTH: Pixels = px(168.);
    pub const LOGO_TILE: Pixels = px(48.);
    pub const LOGO: Pixels = px(24.);
    pub const ROW_LOGO: Pixels = px(20.);
    pub const BADGE_HEIGHT: Pixels = px(22.);
    pub const BADGE_DOT: Pixels = px(6.);
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

pub mod mascot {
    use std::time::Duration;

    use super::{Pixels, px};

    /// How dark the beak is over the head.
    pub const BEAK_SHADE: f32 = 0.42;
    /// Below this size a mascot draws its eyes larger, so that they still read.
    pub const SMALL: Pixels = px(24.);
    /// The side of the box the layers are drawn on.
    pub const BOX: f32 = 100.;
    /// A mascot that waits for an answer tilts its head: the angle in degrees,
    /// and the point of the box it turns about.
    pub const TILT: f32 = -10.;
    pub const TILT_CENTER: (f32, f32) = (50., 60.);
    /// How high a dot of a mascot that works hops, on the box.
    pub const DOT_HOP: f32 = 5.;

    // The life of a mascot: what moves while nothing changes.

    /// The point of the box a head sways about: its base.
    pub const NECK: (f32, f32) = (50., 90.);
    /// How far a head sways to each side of its pose, in degrees: at rest,
    /// at work, while it waits, and after a failure.
    pub const SWAY_REST: f32 = 2.5;
    pub const SWAY_WORKING: f32 = 1.5;
    pub const SWAY_WAITING: f32 = 2.;
    pub const SWAY_FAILED: f32 = 0.8;
    /// How far the pupils of a mascot at rest look to the side its head leans
    /// to, on the box.
    pub const GAZE: f32 = 1.2;
    /// How high a mascot with something new bobs, on the box.
    pub const BOB: f32 = 2.5;
    /// A mascot that waits sways this many times faster, and one with
    /// something new bobs this many times faster.
    pub const WAITING_PACE: u32 = 2;
    pub const BOB_PACE: u32 = 4;
    /// The top and the bottom of the lids that close in a blink, on the box.
    /// The lids hang from their top, well above the eyes, so the first half
    /// of their way down passes over the head and shows nothing: a blink is
    /// short, and the half blink that follows it only lowers the lids a
    /// little.
    pub const LID_TOP: f32 = 28.;
    /// The bottom is drawn in the layer: only the tests of the blink read it.
    #[cfg(test)]
    pub const LID_BOTTOM: f32 = 65.;
    /// The height of an open lid, as a share of its height when closed.
    pub const LID_OPEN: f32 = 0.02;
    /// The frames per second the life of the mascots asks for: its motion is
    /// slow, so it needs no more.
    pub const FPS: f32 = 30.;

    /// How long a sway to one side, to the other, and back lasts for the
    /// mascot `id`: 6.1 to 9.7 s. Each bot has its own, so that the bots of a
    /// list do not move together.
    pub fn sway_period(id: usize) -> Duration {
        Duration::from_millis(6100 + (id as u64 * 997) % 3600)
    }

    /// How long passes from a blink of the mascot `id` to the next: 3.1 to
    /// 4.9 s. A blink, a half blink, then open eyes for the rest of it.
    pub fn blink_period(id: usize) -> Duration {
        Duration::from_millis(3100 + (id as u64 * 619) % 1800)
    }
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
    pub const AVATAR_RING: Pixels = px(2.);
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
    pub const AVATAR_EMPTY: Pixels = px(64.);
    pub const QUOTE_BAR: Pixels = px(2.);
    pub const CHECKBOX: Pixels = px(14.);
    pub const JUMP: Pixels = px(44.);
    pub const SEND: Pixels = px(28.);
    pub const ACTION_ICON: Pixels = px(17.);
    pub const AVATAR_CREATION: Pixels = px(72.);
    pub const AVATAR_TEMPLATE: Pixels = px(42.);
    /// A mascot in a cell of the head picker.
    pub const AVATAR_FACE: Pixels = px(32.);
    /// The live avatar of the creation form rises this far above its box.
    pub const AVATAR_CREATION_LIFT: Pixels = px(8.);
    pub const AVATAR_PANEL: Pixels = px(62.);
    pub const SWATCH_BUTTON: Pixels = px(28.);
    pub const SWATCH: Pixels = px(18.);
    pub const FACE_CELL: Pixels = px(40.);
    pub const RING: Pixels = px(2.);
    pub const PROVIDER_RING: Pixels = px(28.);
    /// A round icon button, such as search and new bot over the bot list.
    pub const ROUND_BUTTON: Pixels = px(36.);
    /// A provider logo inside its ring on the rail.
    pub const PROVIDER_LOGO: Pixels = px(14.);
    pub const RAIL_SEPARATOR: Pixels = px(24.);
}

/// The window chrome the client draws on Linux (`chrome.rs`, `DESIGN.md` 5.13).
pub mod chrome {
    use super::{Pixels, px};

    /// The edge of the window: 1 px inside its outline.
    pub const BORDER: Pixels = px(1.);
    /// The concentric rule from the card: its radius of 14, its inset of 4,
    /// and the border of 1 (a test holds the sum).
    pub const RADIUS: Pixels = px(19.);
    /// The transparent margin around a floating window, where the shadow
    /// draws. It is given to the compositor as the client inset.
    pub const SHADOW: Pixels = px(16.);
    /// The blur of the shadow, inside the margin.
    pub const SHADOW_BLUR: Pixels = px(12.);
    /// The band of the margin next to the window that resizes it, as wide as
    /// the border Mutter grabs around a window it decorates. The rest of the
    /// margin is outside the input region: a click there reaches the window
    /// behind.
    pub const RESIZE: Pixels = px(10.);
    /// A corner resizes along both edges in a square this wide, from the
    /// outer edge of the band, which reaches 5 px into the window.
    pub const RESIZE_CORNER: Pixels = px(15.);
    /// The round hover of a window button, centered in its 46 wide slot.
    pub const BUTTON_HOVER: Pixels = px(24.);
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
    /// The round actions over the bot list start this far below its top, so
    /// the list starts where it did under the old 32 search field.
    pub const SIDEBAR_ACTIONS_TOP: Pixels = px(9.);
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

    const VIEWS: [&str; 20] = [
        "main.rs",
        "composer.rs",
        "ui.rs",
        "mascot.rs",
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
        let first = if cfg!(target_os = "linux") {
            "Adwaita Sans"
        } else {
            ".SystemUIFont"
        };
        assert_eq!(fallbacks.fallback_list()[0], first);
        let mono = font::mono();
        assert!(
            ["Consolas", "Menlo", "DejaVu Sans Mono"].contains(&mono.family.as_ref()),
            "{}",
            mono.family
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_names_its_own_families_and_no_segoe_one() {
        for family in ["Adwaita Sans", "Noto Sans", "Cantarell", "DejaVu Sans"] {
            assert!(font::SANS_FALLBACKS.contains(&family), "{family}");
        }
        assert!(
            font::SANS_FALLBACKS
                .iter()
                .chain(font::MONO_FALLBACKS)
                .all(|family| !family.contains("Segoe") && !family.starts_with('.'))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_monospaced_family_is_fontconfigs_choice_or_a_family_the_system_has() {
        let installed = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        let fedora = installed(&["Inter", "Adwaita Mono", "Noto Sans Mono"]);
        assert_eq!(
            font::resolve_mono(&fedora, Some("Noto Sans Mono".into())),
            "Noto Sans Mono"
        );
        assert_eq!(
            font::resolve_mono(&fedora, Some("Gone Mono".into())),
            "Adwaita Mono"
        );
        assert_eq!(font::resolve_mono(&fedora, None), "Adwaita Mono");
        // A system with no font of its own: code is set in the embedded Inter.
        assert_eq!(font::resolve_mono(&installed(&["Inter"]), None), "Inter");
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
            // The state badge of a provider, on the settings page.
            for (text, fill) in [
                (theme.success_text, theme.success_soft),
                (theme.warning_text, theme.warning_soft),
                (theme.danger_text, theme.danger_soft),
                (theme.text_secondary, theme.badge),
            ] {
                assert!(
                    contrast(text, over(fill, theme.canvas)) >= 4.5,
                    "{} on the badge {}",
                    hex(text),
                    hex(fill)
                );
            }
            // The search palette: its rows, the row the keys are on, and
            // its key hints.
            let palette_row = over(theme.menu_highlight, theme.sheet);
            let palette_footer = over(theme.palette_footer, theme.sheet);
            let key_cap = over(theme.badge, palette_footer);
            assert!(contrast(theme.text, palette_row) >= 4.5);
            assert!(contrast(theme.text_muted, palette_row) >= 4.5);
            assert!(contrast(theme.text_muted, palette_footer) >= 4.5);
            assert!(contrast(theme.text_secondary, key_cap) >= 4.5);
            // A row of the settings page, and its controls.
            assert!(contrast(theme.text_muted, theme.surface_raised) >= 4.5);
            assert!(contrast(theme.text, theme.surface_raised) >= 4.5);
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

/// What the real text system of Linux draws: system fonts, through the same
/// fallbacks as the window.
#[cfg(test)]
#[cfg(target_os = "linux")]
mod system_fonts {
    use gpui::{FontRun, px};

    use super::font;

    /// The characters of `text` that `font` and its fallbacks shape to no
    /// glyph, or to a glyph the rasterizer draws with no pixel.
    fn undrawn(text: &str, font: &gpui::Font) -> Vec<char> {
        let platform = gpui_platform::current_platform(true);
        let system = platform.text_system();
        system.add_fonts(crate::assets::fonts()).unwrap();
        let id = system.font_id(font).unwrap();
        let layout = system.layout_line(
            text,
            px(13.),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        );
        let mut missing = Vec::new();
        for run in &layout.runs {
            for glyph in &run.glyphs {
                let character = text[glyph.index..].chars().next().unwrap();
                if character.is_whitespace() || character == '\u{200d}' {
                    continue;
                }
                let params = gpui::RenderGlyphParams {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    font_size: px(13.),
                    subpixel_variant: gpui::Point::default(),
                    scale_factor: 1.,
                    is_emoji: glyph.is_emoji,
                    subpixel_rendering: false,
                    dilation: 0,
                };
                let drawn = glyph.id.0 != 0
                    && system.glyph_raster_bounds(&params).is_ok_and(|bounds| {
                        bounds.size.width.0 > 0
                            && bounds.size.height.0 > 0
                            && system
                                .rasterize_glyph(&params, bounds)
                                .is_ok_and(|(_, pixels)| pixels.iter().any(|byte| *byte != 0))
                    });
                if !drawn {
                    missing.push(character);
                }
            }
        }
        missing
    }

    #[test]
    #[ignore = "reads the fonts this computer has; run on the target desktop"]
    fn every_script_of_a_message_draws_a_glyph() {
        let message = "Latin é Ж Ω مرحبا 你好 こんにちは 안녕하세요";
        let missing = undrawn(message, &font::sans());
        assert!(missing.is_empty(), "nothing drawn for {missing:?}");
    }

    /// Fedora's "Noto Color Emoji" is COLRv1, which swash does not draw:
    /// `patches/gpui_wgpu` draws it.
    #[test]
    #[ignore = "reads the fonts this computer has; run on the target desktop"]
    fn an_emoji_draws_a_glyph() {
        let missing = undrawn("🐦‍⬛ 👍 ❤️", &font::sans());
        assert!(missing.is_empty(), "nothing drawn for {missing:?}");
    }

    #[test]
    #[ignore = "reads the fonts this computer has; run on the target desktop"]
    fn an_emoji_draws_in_color() {
        let platform = gpui_platform::current_platform(true);
        let system = platform.text_system();
        system.add_fonts(crate::assets::fonts()).unwrap();
        let id = system.font_id(&font::sans()).unwrap();
        let text = "👍 ❤️";
        let layout = system.layout_line(
            text,
            px(13.),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        );
        let emoji = layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(move |glyph| (run.font_id, glyph)))
            .filter(|(_, glyph)| !text[glyph.index..].starts_with(' '))
            .collect::<Vec<_>>();
        assert_eq!(emoji.len(), 2);
        for (font_id, glyph) in emoji {
            assert!(glyph.is_emoji, "{:?}", &text[glyph.index..]);
            let params = gpui::RenderGlyphParams {
                font_id,
                glyph_id: glyph.id,
                font_size: px(13.),
                subpixel_variant: gpui::Point::default(),
                scale_factor: 2.,
                is_emoji: true,
                subpixel_rendering: false,
                dilation: 0,
            };
            let bounds = system.glyph_raster_bounds(&params).unwrap();
            let (_, bgra) = system.rasterize_glyph(&params, bounds).unwrap();
            let colored = bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|&[b, g, r, a]| a > 200 && r.max(g).max(b) - r.min(g).min(b) > 100);
            assert!(colored, "{:?} is not in color", &text[glyph.index..]);
        }
    }

    #[test]
    #[ignore = "reads the fonts this computer has; run on the target desktop"]
    fn a_code_block_is_set_in_a_monospaced_family_the_system_has() {
        let platform = gpui_platform::current_platform(true);
        let system = platform.text_system();
        let installed = system.all_font_names();
        let family = font::resolve_mono(&installed, None);
        assert!(installed.iter().any(|name| *name == *family), "{family}");
        let mut code = font::mono();
        code.family = family.clone();
        let id = system.font_id(&code).unwrap();
        let narrow = system
            .advance(id, system.glyph_for_char(id, 'i').unwrap())
            .unwrap();
        let wide = system
            .advance(id, system.glyph_for_char(id, 'W').unwrap())
            .unwrap();
        assert_eq!(narrow.width, wide.width, "{family} is not monospaced");
    }
}

/// The text system of Linux, the one the window draws with, on the fonts of
/// `fixtures/fonts`: subsets of Fedora's fonts under family names no system
/// has, so the result does not depend on the computer.
#[cfg(test)]
#[cfg(target_os = "linux")]
mod text_system {
    use std::{borrow::Cow, sync::Arc};

    use gpui::{FontFallbacks, FontRun, PlatformTextSystem, ShapedGlyph, px};

    const COLOR_EMOJI: &str = "Crow Test Color Emoji";
    const TEXT_EMOJI: &str = "Crow Test Text Emoji";
    /// Arabic letters and no Latin glyph.
    const ARABIC: &str = "Crow Test Arabic";
    /// Arabic letters and a Latin `m`.
    const LATIN_ARABIC: &str = "Crow Test Latin Arabic";

    fn system() -> Arc<dyn PlatformTextSystem> {
        let system = gpui_platform::current_platform(true).text_system();
        let mut fonts = crate::assets::fonts();
        fonts.extend([
            Cow::Borrowed(include_bytes!("../fixtures/fonts/CrowTestColorEmoji.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../fixtures/fonts/CrowTestTextEmoji.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../fixtures/fonts/CrowTestArabic.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../fixtures/fonts/CrowTestLatinArabic.ttf").as_slice()),
        ]);
        system.add_fonts(fonts).unwrap();
        system
    }

    /// Inter, the client's own family, then `fallbacks` in that order.
    fn inter_then(fallbacks: &[&str]) -> gpui::Font {
        let mut font = gpui::font(super::font::SANS);
        font.fallbacks = Some(FontFallbacks::from_fonts(
            fallbacks.iter().map(|family| family.to_string()).collect(),
        ));
        font
    }

    /// Each glyph of `text` set in `font`, with the font it came from.
    fn shape(
        system: &dyn PlatformTextSystem,
        text: &str,
        font: &gpui::Font,
    ) -> Vec<(gpui::FontId, ShapedGlyph)> {
        let id = system.font_id(font).unwrap();
        let layout = system.layout_line(
            text,
            px(32.),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        );
        layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| (run.font_id, glyph.clone())))
            .collect()
    }

    /// The glyph as the atlas receives it: BGRA for an emoji, one alpha byte
    /// per pixel otherwise.
    fn raster(
        system: &dyn PlatformTextSystem,
        font_id: gpui::FontId,
        glyph: &ShapedGlyph,
    ) -> Vec<u8> {
        let params = gpui::RenderGlyphParams {
            font_id,
            glyph_id: glyph.id,
            font_size: px(32.),
            subpixel_variant: gpui::Point::default(),
            scale_factor: 2.,
            is_emoji: glyph.is_emoji,
            subpixel_rendering: false,
            dilation: 0,
        };
        let bounds = system.glyph_raster_bounds(&params).unwrap();
        system.rasterize_glyph(&params, bounds).unwrap().1
    }

    /// Whether some opaque pixel of a BGRA image is far from gray.
    fn has_color(bgra: &[u8]) -> bool {
        bgra.as_chunks::<4>().0.iter().any(|&[b, g, r, a]| {
            let high = r.max(g).max(b);
            let low = r.min(g).min(b);
            a > 200 && high - low > 100
        })
    }

    #[test]
    fn a_colrv1_emoji_draws_in_color() {
        let system = system();
        let glyphs = shape(&*system, "👍", &inter_then(&[COLOR_EMOJI]));
        assert_eq!(glyphs.len(), 1);
        let (font_id, glyph) = &glyphs[0];
        assert!(glyph.is_emoji, "a face with color glyphs draws emoji");
        let pixels = raster(&*system, *font_id, glyph);
        assert!(
            has_color(&pixels),
            "the thumb is drawn in its palette colors"
        );
        // The thumb of Noto Color Emoji is yellow, from a gradient.
        let yellow = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|&&[b, g, r, a]| a > 200 && r > 200 && g > 140 && b < 90);
        assert!(yellow.count() > 200);
    }

    #[test]
    fn emoji_fallbacks_with_no_latin_glyph_are_used_in_the_order_named() {
        let system = system();
        for (chain, color) in [
            ([COLOR_EMOJI, TEXT_EMOJI], true),
            ([TEXT_EMOJI, COLOR_EMOJI], false),
        ] {
            let glyphs = shape(&*system, "👍", &inter_then(&chain));
            assert_eq!(glyphs.len(), 1, "{chain:?}");
            assert_eq!(glyphs[0].1.is_emoji, color, "{chain:?}");
        }
    }

    #[test]
    fn an_arabic_fallback_with_no_latin_glyph_comes_before_a_later_one() {
        let system = system();
        let latin_arabic = system
            .font_metrics(system.font_id(&gpui::font(LATIN_ARABIC)).unwrap())
            .units_per_em;
        let arabic_upem = |chain: [&str; 2]| {
            let glyphs = shape(&*system, "مرحبا", &inter_then(&chain));
            assert!(!glyphs.is_empty());
            let ems = glyphs
                .iter()
                .map(|(font_id, _)| system.font_metrics(*font_id).units_per_em)
                .collect::<Vec<_>>();
            assert!(ems.windows(2).all(|pair| pair[0] == pair[1]), "{chain:?}");
            ems[0]
        };
        // Noto Sans Arabic has 1000 units per em and Vazirmatn 2048: the
        // em size tells which face drew the word.
        assert_ne!(arabic_upem([ARABIC, LATIN_ARABIC]), latin_arabic);
        assert_eq!(arabic_upem([LATIN_ARABIC, ARABIC]), latin_arabic);
    }

    #[test]
    fn a_family_with_no_latin_glyph_never_becomes_a_primary_font() {
        let system = system();
        // Named as a fallback first, then asked for as the font itself: it
        // has no `m` to measure, so it is not one.
        let _ = shape(&*system, "👍", &inter_then(&[COLOR_EMOJI]));
        assert!(system.font_id(&gpui::font(COLOR_EMOJI)).is_err());
    }

    #[test]
    fn a_variation_selector_picks_the_presentation() {
        let system = system();
        let font = inter_then(&[TEXT_EMOJI, COLOR_EMOJI]);
        let emoji = shape(&*system, "\u{2764}\u{FE0F}", &font);
        assert!(emoji.iter().any(|(_, glyph)| glyph.is_emoji));
        let text = shape(&*system, "\u{2764}", &font);
        assert!(text.iter().all(|(_, glyph)| !glyph.is_emoji));
    }
}
