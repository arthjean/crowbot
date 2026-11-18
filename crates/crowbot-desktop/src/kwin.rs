//! The window buttons KWin is set to draw, read from its own configuration.
//!
//! GPUI takes the button layout from the GNOME key the desktop portal serves
//! (`org.gnome.desktop.wm.preferences button-layout`). On KDE that key holds
//! KWin's setting only when kde-gtk-config copies it there, and the KDE portal
//! does not serve KWin's configuration. So on KDE the client reads the group
//! `org.kde.kdecoration2` of `kwinrc` the way KWin does, and reads it again
//! each time its window becomes active, which is when a change made in the
//! system settings can show.

use std::path::{Path, PathBuf};

use gpui::{App, Global, WindowButtonLayout};

const GROUP: &str = "[org.kde.kdecoration2]";
const LEFT: &str = "ButtonsOnLeft";
const RIGHT: &str = "ButtonsOnRight";
/// KWin's own defaults (`src/decorations/settings.cpp`): menu, on all
/// desktops, and exclude from capture on the left; help, minimize, maximize,
/// and close on the right.
const DEFAULT_LEFT: &str = "MSE";
const DEFAULT_RIGHT: &str = "HIAX";

struct Layout(Option<WindowButtonLayout>);

impl Global for Layout {}

/// The layout KWin draws, on a KDE desktop.
pub fn button_layout(cx: &App) -> Option<WindowButtonLayout> {
    cx.try_global::<Layout>().and_then(|layout| layout.0)
}

/// Reads KWin's layout, on a KDE desktop.
pub fn refresh(cx: &mut App) {
    if on_kde(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()) {
        let (left, right) = read(&files());
        cx.set_global(Layout(layout(&left, &right)));
    }
}

fn on_kde(desktops: Option<&str>) -> bool {
    desktops.is_some_and(|desktops| desktops.split(':').any(|desktop| desktop == "KDE"))
}

/// The `kwinrc` files in the order KConfig reads them: the user's own, then
/// the defaults Plasma writes for the user, then each system folder.
fn files() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")));
    let system = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/etc/xdg".to_owned());
    config
        .iter()
        .flat_map(|config| [config.join("kwinrc"), config.join("kdedefaults/kwinrc")])
        .chain(
            system
                .split(':')
                .map(Path::new)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("kwinrc")),
        )
        .collect()
}

/// The left and right buttons, each from the first file that sets it, else
/// KWin's default.
fn read(files: &[PathBuf]) -> (String, String) {
    let contents: Vec<String> = files
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .collect();
    let find = |key: &str, default: &str| {
        contents
            .iter()
            .find_map(|text| entry(text, key))
            .unwrap_or_else(|| default.to_owned())
    };
    (find(LEFT, DEFAULT_LEFT), find(RIGHT, DEFAULT_RIGHT))
}

/// The value of `key` in the decoration group of one file. A key may carry
/// KConfig's `[$i]` flag; a localized key (`Key[fr]`) is not this one.
fn entry(text: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && !line.contains('=') {
            inside = line == GROUP;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let flags = name.strip_prefix(key).map(str::trim);
        if matches!(flags, Some("" | "[$i]")) {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// GPUI's layout for KWin's letters: `I` minimize, `A` maximize, `X` close.
/// The other buttons KWin draws have no match in the client.
fn layout(left: &str, right: &str) -> Option<WindowButtonLayout> {
    let names = |buttons: &str| {
        buttons
            .chars()
            .filter_map(|letter| match letter {
                'I' => Some("minimize"),
                'A' => Some("maximize"),
                'X' => Some("close"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    WindowButtonLayout::parse(&format!("{}:{}", names(left), names(right))).ok()
}

#[cfg(test)]
mod tests {
    use gpui::{WindowButton, WindowButtonLayout};

    use super::*;

    fn sides(layout: WindowButtonLayout) -> (Vec<WindowButton>, Vec<WindowButton>) {
        (
            layout.left.iter().flatten().copied().collect(),
            layout.right.iter().flatten().copied().collect(),
        )
    }

    #[test]
    fn kwin_letters_become_the_buttons_the_client_draws_in_their_order() {
        use WindowButton::{Close, Maximize, Minimize};
        assert_eq!(
            sides(layout("XAI", "").unwrap()),
            (vec![Close, Maximize, Minimize], vec![])
        );
        assert_eq!(
            sides(layout(DEFAULT_LEFT, DEFAULT_RIGHT).unwrap()),
            (vec![], vec![Minimize, Maximize, Close])
        );
        assert_eq!(
            sides(layout("M", "F_IX").unwrap()),
            (vec![], vec![Minimize, Close])
        );
        assert_eq!(sides(layout("", "").unwrap()), (vec![], vec![]));
    }

    #[test]
    fn the_first_file_that_sets_a_side_wins_and_a_missing_side_is_kwins_default() {
        let dir = std::env::temp_dir().join(format!("crowbot-kwinrc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user = dir.join("user");
        let defaults = dir.join("defaults");
        std::fs::write(
            &user,
            "[General]\nButtonsOnLeft=X\n\n[org.kde.kdecoration2]\nButtonsOnLeft[$i]=XA\nButtonsOnLeft[fr]=I\n",
        )
        .unwrap();
        std::fs::write(
            &defaults,
            "[org.kde.kdecoration2]\nButtonsOnLeft=I\nButtonsOnRight=\n",
        )
        .unwrap();
        let read = read(&[user, defaults, dir.join("missing")]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read, ("XA".to_owned(), String::new()));
        assert_eq!(
            super::read(&[]),
            (DEFAULT_LEFT.to_owned(), DEFAULT_RIGHT.to_owned())
        );
    }

    #[test]
    fn only_a_kde_desktop_follows_kwin() {
        assert!(on_kde(Some("KDE")));
        assert!(on_kde(Some("ubuntu:KDE")));
        assert!(!on_kde(Some("GNOME")));
        assert!(!on_kde(Some("KDEish")));
        assert!(!on_kde(None));
    }
}
