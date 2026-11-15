use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$variant),+];

            pub fn path(self) -> &'static str {
                match self {
                    $(Icon::$variant => concat!("icons/", $file)),+
                }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!("../assets/icons/", $file))),+
                }
            }
        }
    };
}

icons! {
    ArrowDown => "arrow_down.svg",
    ArrowUp => "arrow_up.svg",
    Bird => "bird.svg",
    Check => "check.svg",
    ChevronDown => "chevron_down.svg",
    ChevronRight => "chevron_right.svg",
    CircleAlert => "circle_alert.svg",
    Clock => "clock.svg",
    Copy => "copy.svg",
    ExternalLink => "external_link.svg",
    FileText => "file_text.svg",
    Folder => "folder.svg",
    Globe => "globe.svg",
    History => "history.svg",
    Info => "info.svg",
    Lightbulb => "lightbulb.svg",
    Loader => "loader_circle.svg",
    MessageSquare => "message_square.svg",
    PanelLeft => "panel_left.svg",
    PanelRight => "panel_right.svg",
    Pencil => "pencil.svg",
    Plug => "plug.svg",
    Plus => "plus.svg",
    Refresh => "refresh_cw.svg",
    Search => "search.svg",
    Settings => "settings.svg",
    ShieldCheck => "shield_check.svg",
    Sliders => "sliders_horizontal.svg",
    Square => "square.svg",
    SquarePen => "square_pen.svg",
    Terminal => "terminal.svg",
    Trash => "trash.svg",
    TriangleAlert => "triangle_alert.svg",
    Wrench => "wrench.svg",
    X => "x.svg",
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL
            .iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Borrowed(icon.bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|icon| icon.path())
            .filter(|icon| icon.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-Medium.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-SemiBold.ttf").as_slice()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_as_an_svg_from_its_path() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg"), "{}", icon.path());
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
    }

    #[test]
    fn the_bundled_fonts_are_the_three_weights_of_inter() {
        let fonts = fonts();
        assert_eq!(fonts.len(), 3);
        for font in fonts {
            assert!(font.len() > 100_000);
            assert_eq!(&font[..4], &[0, 1, 0, 0]);
        }
    }
}
