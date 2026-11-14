use gpui::{Rgba, WindowAppearance};

#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Rgba,
    pub sidebar: Rgba,
    pub raised: Rgba,
    pub hover: Rgba,
    pub selected: Rgba,
    pub separator: Rgba,
    pub control_border: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    pub accent_solid: Rgba,
    pub accent_solid_hover: Rgba,
    pub on_accent: Rgba,
    pub focus_ring: Rgba,
    pub text_selection: Rgba,
    pub attention_surface: Rgba,
    pub attention_border: Rgba,
    pub attention_text: Rgba,
    pub danger_text: Rgba,
    pub success_text: Rgba,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    fn light() -> Self {
        Self {
            background: oklch(0.985, 0.002, 250.),
            sidebar: oklch(0.962, 0.004, 250.),
            raised: oklch(1., 0., 0.),
            hover: oklch(0.935, 0.005, 250.),
            selected: oklch(0.905, 0.008, 250.),
            separator: oklch(0.90, 0.005, 250.),
            control_border: oklch(0.62, 0.012, 250.),
            text: oklch(0.21, 0.012, 250.),
            text_secondary: oklch(0.46, 0.012, 250.),
            accent_solid: oklch(0.50, 0.16, 255.),
            accent_solid_hover: oklch(0.44, 0.16, 255.),
            on_accent: oklch(1., 0., 0.),
            focus_ring: oklch(0.55, 0.18, 255.),
            text_selection: with_alpha(oklch(0.55, 0.18, 255.), 0.25),
            attention_surface: oklch(0.965, 0.035, 85.),
            attention_border: oklch(0.58, 0.13, 70.),
            attention_text: oklch(0.40, 0.09, 70.),
            danger_text: oklch(0.50, 0.18, 27.),
            success_text: oklch(0.46, 0.12, 150.),
        }
    }

    fn dark() -> Self {
        Self {
            background: oklch(0.175, 0.004, 250.),
            sidebar: oklch(0.148, 0.004, 250.),
            raised: oklch(0.215, 0.005, 250.),
            hover: oklch(0.245, 0.006, 250.),
            selected: oklch(0.29, 0.010, 250.),
            separator: oklch(0.28, 0.006, 250.),
            control_border: oklch(0.56, 0.012, 250.),
            text: oklch(0.93, 0.004, 250.),
            text_secondary: oklch(0.72, 0.008, 250.),
            accent_solid: oklch(0.52, 0.15, 255.),
            accent_solid_hover: oklch(0.46, 0.15, 255.),
            on_accent: oklch(1., 0., 0.),
            focus_ring: oklch(0.72, 0.13, 255.),
            text_selection: with_alpha(oklch(0.72, 0.13, 255.), 0.30),
            attention_surface: oklch(0.26, 0.04, 80.),
            attention_border: oklch(0.66, 0.11, 80.),
            attention_text: oklch(0.87, 0.09, 85.),
            danger_text: oklch(0.74, 0.14, 25.),
            success_text: oklch(0.76, 0.12, 150.),
        }
    }
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

fn oklch(lightness: f32, chroma: f32, hue_degrees: f32) -> Rgba {
    let hue = hue_degrees.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    Rgba {
        r: encode(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s),
        g: encode(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
        b: encode(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        a: 1.,
    }
}

fn encode(linear: f32) -> f32 {
    let linear = linear.clamp(0., 1.);
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn contrast(first: Rgba, second: Rgba) -> f32 {
        let (first, second) = (luminance(first), luminance(second));
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    #[test]
    fn white_and_black_convert_exactly() {
        let white = oklch(1., 0., 0.);
        let black = oklch(0., 0., 0.);
        assert!((white.r - 1.).abs() < 0.001 && (white.g - 1.).abs() < 0.001);
        assert!(black.r < 0.001 && black.b < 0.001);
    }

    #[test]
    fn a_known_blue_converts_to_its_srgb_value() {
        let blue = oklch(0.452, 0.313, 264.05);
        assert!(blue.r < 0.02 && blue.g < 0.02 && (blue.b - 1.).abs() < 0.02);
    }

    #[test]
    fn text_meets_wcag_contrast_on_every_surface_it_sits_on() {
        for theme in [Theme::light(), Theme::dark()] {
            for surface in [
                theme.background,
                theme.sidebar,
                theme.raised,
                theme.hover,
                theme.selected,
            ] {
                assert!(contrast(theme.text, surface) >= 4.5);
                assert!(contrast(theme.text_secondary, surface) >= 4.5);
            }
            for text in [theme.danger_text, theme.success_text] {
                assert!(contrast(text, theme.background) >= 4.5);
                assert!(contrast(text, theme.raised) >= 4.5);
            }
            assert!(contrast(theme.on_accent, theme.accent_solid) >= 4.5);
            assert!(contrast(theme.on_accent, theme.accent_solid_hover) >= 4.5);
            assert!(contrast(theme.attention_text, theme.attention_surface) >= 4.5);
            assert!(contrast(theme.text, theme.attention_surface) >= 4.5);
        }
    }

    #[test]
    fn control_edges_and_focus_rings_meet_non_text_contrast() {
        for theme in [Theme::light(), Theme::dark()] {
            assert!(contrast(theme.control_border, theme.raised) >= 3.);
            assert!(contrast(theme.control_border, theme.background) >= 3.);
            assert!(contrast(theme.focus_ring, theme.background) >= 3.);
            assert!(contrast(theme.focus_ring, theme.sidebar) >= 3.);
            assert!(contrast(theme.focus_ring, theme.raised) >= 3.);
            assert!(contrast(theme.attention_border, theme.attention_surface) >= 3.);
            assert!(contrast(theme.accent_solid, theme.background) >= 3.);
        }
    }
}
