use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// The SVG files of one folder of `assets`, as an enum with one variant each.
macro_rules! svgs {
    ($(#[$meta:meta])* $name:ident in $folder:literal: $($variant:ident => $file:literal),+ $(,)?) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant),+];

            pub fn path(self) -> &'static str {
                match self {
                    $($name::$variant => concat!($folder, $file)),+
                }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $($name::$variant => include_bytes!(concat!("../assets/", $folder, $file))),+
                }
            }
        }
    };
}

svgs! {
    Icon in "icons/":
    ArrowDown => "arrow_down.svg",
    ArrowLeft => "arrow_left.svg",
    ArrowUp => "arrow_up.svg",
    Bird => "bird.svg",
    Bot => "bot.svg",
    Check => "check.svg",
    ChevronDown => "chevron_down.svg",
    ChevronUpDown => "chevrons_up_down.svg",
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
    ProviderClaude => "providers/claude.svg",
    ProviderGrok => "providers/grok.svg",
    ProviderOpenAi => "providers/openai.svg",
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

svgs! {
    /// A layer of the mascot of a bot: a filled shape of one color, drawn on a
    /// box of 100 by 100 that all the layers share. The layers of `small/` are
    /// the eyes of a small mascot, drawn larger.
    MascotLayer in "mascot/":
    HeadSmooth => "head_smooth.svg",
    HeadCrest => "head_crest.svg",
    HeadSpikes => "head_spikes.svg",
    HeadCurl => "head_curl.svg",
    HeadPlume => "head_plume.svg",
    HeadFlat => "head_flat.svg",
    Beak => "beak.svg",
    Eyes => "eyes.svg",
    EyesWide => "eyes_wide.svg",
    EyesHappy => "eyes_happy.svg",
    PupilsRest => "pupils_rest.svg",
    PupilsWorking => "pupils_working.svg",
    PupilsWaiting => "pupils_waiting.svg",
    PupilsFailed => "pupils_failed.svg",
    Lids => "lids.svg",
    Blink => "blink.svg",
    Dot1 => "dot_1.svg",
    Dot2 => "dot_2.svg",
    Dot3 => "dot_3.svg",
    SmallEyes => "small/eyes.svg",
    SmallEyesWide => "small/eyes_wide.svg",
    SmallEyesHappy => "small/eyes_happy.svg",
    SmallPupilsRest => "small/pupils_rest.svg",
    SmallPupilsWorking => "small/pupils_working.svg",
    SmallPupilsWaiting => "small/pupils_waiting.svg",
    SmallPupilsFailed => "small/pupils_failed.svg",
    SmallLids => "small/lids.svg",
    SmallBlink => "small/blink.svg",
}

/// Every bundled SVG: its path and its bytes.
fn svgs() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    let icons = Icon::ALL.iter().map(|icon| (icon.path(), icon.bytes()));
    let layers = MascotLayer::ALL
        .iter()
        .map(|layer| (layer.path(), layer.bytes()));
    icons.chain(layers)
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(svgs()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, bytes)| Cow::Borrowed(bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(svgs()
            .map(|(candidate, _)| candidate)
            .filter(|candidate| candidate.starts_with(path))
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
    fn every_stroke_icon_has_the_light_stroke_and_every_mark_is_filled() {
        for icon in Icon::ALL {
            let text = std::str::from_utf8(icon.bytes()).unwrap();
            if icon.path().starts_with("icons/providers/") {
                assert!(text.contains("fill=\"black\""), "{}", icon.path());
                assert!(!text.contains("stroke"), "{}", icon.path());
            } else {
                assert!(text.contains("stroke-width=\"1.75\""), "{}", icon.path());
            }
        }
    }

    #[test]
    fn every_mascot_layer_loads_as_a_filled_svg_on_the_shared_box() {
        for layer in MascotLayer::ALL {
            let bytes = Assets.load(layer.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg"), "{}", layer.path());
            assert!(text.contains("viewBox=\"0 0 100 100\""), "{}", layer.path());
            assert!(text.contains("fill=\"black\""), "{}", layer.path());
            assert!(!text.contains("stroke"), "{}", layer.path());
        }
        assert_eq!(
            Assets.list("mascot/").unwrap().len(),
            MascotLayer::ALL.len()
        );
    }

    #[test]
    fn every_file_of_the_mascot_folder_is_a_layer() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/mascot");
        let count = |folder: &std::path::Path| {
            std::fs::read_dir(folder)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().path().extension().is_some())
                .count()
        };
        assert_eq!(
            count(&folder) + count(&folder.join("small")),
            MascotLayer::ALL.len()
        );
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
