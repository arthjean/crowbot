use serde::{Deserialize, Serialize};

pub const SIDEBAR_WIDTH_LIMIT: u32 = 4096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    Light,
    Dark,
    #[default]
    #[serde(other)]
    System,
}

impl Appearance {
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendShortcut {
    ModEnter,
    #[default]
    #[serde(other)]
    Enter,
}

impl SendShortcut {
    pub fn name(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::ModEnter => "mod_enter",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "mod_enter" => Self::ModEnter,
            _ => Self::Enter,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub sidebar_width: Option<u32>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub send_shortcut: SendShortcut,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<Appearance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_collapsed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_shortcut: Option<SendShortcut>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_with_no_field_are_the_defaults() {
        let preferences: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(preferences, Preferences::default());
        assert_eq!(preferences.appearance, Appearance::System);
        assert_eq!(preferences.sidebar_width, None);
        assert!(!preferences.sidebar_collapsed);
        assert_eq!(preferences.send_shortcut, SendShortcut::Enter);
    }

    #[test]
    fn a_send_shortcut_keeps_its_name_and_an_unknown_one_is_enter() {
        for shortcut in [SendShortcut::Enter, SendShortcut::ModEnter] {
            let wire = serde_json::to_string(&shortcut).unwrap();
            assert_eq!(wire, format!("\"{}\"", shortcut.name()));
            assert_eq!(SendShortcut::from_name(shortcut.name()), shortcut);
        }
        let preferences: Preferences =
            serde_json::from_str(r#"{"send_shortcut":"double_tap"}"#).unwrap();
        assert_eq!(preferences.send_shortcut, SendShortcut::Enter);
        assert_eq!(SendShortcut::from_name(""), SendShortcut::Enter);
    }

    #[test]
    fn an_appearance_this_version_does_not_know_follows_the_system() {
        let preferences: Preferences = serde_json::from_str(r#"{"appearance":"sepia"}"#).unwrap();
        assert_eq!(preferences.appearance, Appearance::System);
        assert_eq!(Appearance::from_name("sepia"), Appearance::System);
        assert_eq!(Appearance::from_name(""), Appearance::System);
    }

    #[test]
    fn an_appearance_keeps_its_name_on_the_wire_and_in_storage() {
        for appearance in [Appearance::System, Appearance::Light, Appearance::Dark] {
            let wire = serde_json::to_string(&appearance).unwrap();
            assert_eq!(wire, format!("\"{}\"", appearance.name()));
            assert_eq!(Appearance::from_name(appearance.name()), appearance);
        }
    }

    #[test]
    fn a_patch_sends_only_the_fields_it_holds() {
        let patch = PreferencesPatch {
            sidebar_width: Some(320),
            ..PreferencesPatch::default()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"sidebar_width":320}"#
        );
        let read: PreferencesPatch = serde_json::from_str(r#"{"sidebar_collapsed":true}"#).unwrap();
        assert_eq!(read.sidebar_collapsed, Some(true));
        assert_eq!(read.appearance, None);
    }
}
