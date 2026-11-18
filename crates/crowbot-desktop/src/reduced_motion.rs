//! Whether the system asks for reduced motion.
//!
//! GPUI holds the flag that `cx.reduce_motion()` reads, but no platform of it
//! sets the flag from the system: the client does. On Linux the desktop portal
//! sends each change. Windows sends none that GPUI passes on, so the client
//! reads the setting again each time its window becomes active.

use gpui::App;

/// Sets the flag from the system now, and keeps it current.
pub fn follow(cx: &mut App) {
    #[cfg(target_os = "linux")]
    linux::follow(cx);
    #[cfg(target_os = "windows")]
    refresh(cx);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = cx;
}

/// Reads the setting again where the system sends no change.
pub fn refresh(cx: &mut App) {
    #[cfg(target_os = "windows")]
    if let Some(reduce) = windows::reduce() {
        cx.set_reduce_motion(reduce);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cx;
}

/// What the desktop portal reports. Either answer asks for reduced motion:
/// the `reduced-motion` preference of `org.freedesktop.appearance` (1 means
/// reduce), and GNOME's switch that turns every animation off, which GTK
/// honors as well.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Wish {
    reduced_motion: Option<u32>,
    animations: Option<bool>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Wish {
    const REDUCE: u32 = 1;

    fn reduces(self) -> bool {
        self.reduced_motion == Some(Self::REDUCE) || self.animations == Some(false)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use ashpd::desktop::settings::Settings;
    use ashpd::zvariant::OwnedValue;
    use futures_util::stream::{self, LocalBoxStream, StreamExt};
    use gpui::{App, AsyncApp};

    use super::Wish;

    const APPEARANCE: &str = "org.freedesktop.appearance";
    const REDUCED_MOTION: &str = "reduced-motion";
    const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";
    const ENABLE_ANIMATIONS: &str = "enable-animations";

    enum Change {
        ReducedMotion(u32),
        Animations(bool),
    }

    pub(super) fn follow(cx: &mut App) {
        cx.spawn(async |cx| listen(cx).await).detach();
    }

    async fn listen(cx: &mut AsyncApp) {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        let mut changes = stream::select(
            watch(&settings, APPEARANCE, REDUCED_MOTION, Change::ReducedMotion).await,
            watch(
                &settings,
                GNOME_INTERFACE,
                ENABLE_ANIMATIONS,
                Change::Animations,
            )
            .await,
        );
        let mut wish = Wish {
            reduced_motion: settings.read(APPEARANCE, REDUCED_MOTION).await.ok(),
            animations: settings.read(GNOME_INTERFACE, ENABLE_ANIMATIONS).await.ok(),
        };
        apply(wish, cx);
        while let Some(change) = changes.next().await {
            match change {
                Change::ReducedMotion(value) => wish.reduced_motion = Some(value),
                Change::Animations(value) => wish.animations = Some(value),
            }
            apply(wish, cx);
        }
    }

    async fn watch<T>(
        settings: &Settings,
        namespace: &str,
        key: &str,
        change: fn(T) -> Change,
    ) -> LocalBoxStream<'static, Change>
    where
        T: TryFrom<OwnedValue> + 'static,
        ashpd::Error: From<<T as TryFrom<OwnedValue>>::Error>,
    {
        match settings
            .receive_setting_changed_with_args::<T>(namespace, key)
            .await
        {
            Ok(values) => values
                .filter_map(move |value| async move { value.ok().map(change) })
                .boxed_local(),
            Err(_) => stream::empty().boxed_local(),
        }
    }

    fn apply(wish: Wish, cx: &mut AsyncApp) {
        cx.update(|cx| cx.set_reduce_motion(wish.reduces()));
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;

    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    /// Whether "Animation effects" is off in the settings of Windows.
    pub(super) fn reduce() -> Option<bool> {
        let mut animations: i32 = 1;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&raw mut animations).cast::<c_void>()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        }
        .ok()?;
        Some(animations == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::Wish;

    #[test]
    fn the_portal_preference_or_animations_off_asks_for_reduced_motion() {
        let wish = |reduced_motion, animations| {
            Wish {
                reduced_motion,
                animations,
            }
            .reduces()
        };
        assert!(!wish(None, None));
        assert!(!wish(Some(0), Some(true)));
        assert!(wish(Some(1), None));
        assert!(wish(Some(1), Some(true)));
        assert!(wish(None, Some(false)));
        assert!(wish(Some(0), Some(false)));
        assert!(!wish(Some(2), Some(true)));
    }
}
