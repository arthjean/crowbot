//! Why a start cannot show a window, in one line.
//!
//! GPUI panics when the display server its environment names cannot be
//! reached, and opening the window fails when no GPU adapter can draw it.
//! The client checks the display before it starts the application, and turns
//! a window that fails to open into a line that names what is missing.

/// The line printed when no driver can draw the window.
pub const NO_DRIVER: &str = "Crow Bot found no graphics driver that can draw its window \
    (Vulkan or OpenGL). On Fedora, install mesa-vulkan-drivers.";

/// Checks that the display server the environment names answers. On other
/// systems than Linux the platform has a display of its own.
pub fn check() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::check(&linux::Environment::detect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Whether the window opens on X11: GPUI takes Wayland when
/// `WAYLAND_DISPLAY` is set, else X11 when `DISPLAY` is.
pub fn x11() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::on_x11(&linux::Environment::detect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// The line for a window that did not open. GPUI names no adapter when no
/// device can draw the surface, and no backend when neither Vulkan nor GL
/// can create one.
pub fn open_failure(error: &str) -> String {
    if error.contains("No GPU adapter") || error.contains("create surface for any enabled backend")
    {
        NO_DRIVER.to_owned()
    } else {
        format!("Crow Bot cannot open its window: {error}")
    }
}

/// The line written when the only adapter renders on the CPU, or `None`.
pub fn software_notice(specs: Option<&gpui::GpuSpecs>) -> Option<String> {
    let specs = specs.filter(|specs| specs.is_software_emulated)?;
    Some(if specs.device_name.is_empty() {
        "Rendering runs on the CPU.".to_owned()
    } else {
        format!("Rendering runs on the CPU ({}).", specs.device_name)
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::OsString;
    use std::net::TcpStream;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixStream};
    use std::path::PathBuf;

    /// The display variables GPUI reads to pick its backend.
    pub struct Environment {
        pub wayland_display: Option<OsString>,
        /// A socket the parent passed already connected: nothing to check.
        pub wayland_socket: bool,
        pub x11_display: Option<OsString>,
        pub xdg_runtime_dir: Option<OsString>,
    }

    impl Environment {
        pub fn detect() -> Self {
            Self {
                wayland_display: std::env::var_os("WAYLAND_DISPLAY"),
                wayland_socket: std::env::var_os("WAYLAND_SOCKET").is_some(),
                x11_display: std::env::var_os("DISPLAY"),
                xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            }
        }
    }

    fn set(value: &Option<OsString>) -> Option<String> {
        value
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string_lossy().into_owned())
    }

    /// GPUI's choice, in its order: Wayland when `WAYLAND_DISPLAY` is set,
    /// else X11 when `DISPLAY` is set.
    pub fn check(environment: &Environment) -> Result<(), String> {
        if let Some(name) = set(&environment.wayland_display) {
            if environment.wayland_socket {
                return Ok(());
            }
            return wayland(&name, environment);
        }
        if let Some(display) = set(&environment.x11_display) {
            return x11(&display);
        }
        Err("Crow Bot found no display: neither WAYLAND_DISPLAY nor DISPLAY is set.".to_owned())
    }

    pub fn on_x11(environment: &Environment) -> bool {
        set(&environment.wayland_display).is_none() && set(&environment.x11_display).is_some()
    }

    fn wayland(name: &str, environment: &Environment) -> Result<(), String> {
        let path = if name.starts_with('/') {
            PathBuf::from(name)
        } else {
            let Some(runtime) = set(&environment.xdg_runtime_dir) else {
                return Err(format!(
                    "Crow Bot cannot reach the Wayland compositor {name}: XDG_RUNTIME_DIR is not set."
                ));
            };
            PathBuf::from(runtime).join(name)
        };
        UnixStream::connect(&path).map(drop).map_err(|error| {
            format!(
                "Crow Bot cannot reach the Wayland compositor {name} ({}): {error}.",
                path.display()
            )
        })
    }

    /// `DISPLAY` is `[host]:number[.screen]`. With no host the server listens
    /// on `/tmp/.X11-unix/X<number>`, and on Linux also on the abstract socket
    /// of that name; with a host, on TCP port 6000 + number.
    fn x11(display: &str) -> Result<(), String> {
        let unreachable =
            |reason: String| format!("Crow Bot cannot reach the X server {display}: {reason}.");
        let Some((host, rest)) = display.rsplit_once(':') else {
            return Err(unreachable("the address has no display number".to_owned()));
        };
        let number = rest.split('.').next().unwrap_or_default();
        let Ok(number) = number.parse::<u16>() else {
            return Err(unreachable(format!("{number:?} is not a display number")));
        };
        if host.is_empty() || host == "unix" {
            let path = format!("/tmp/.X11-unix/X{number}");
            let abstract_socket = SocketAddr::from_abstract_name(path.as_bytes())
                .and_then(|address| UnixStream::connect_addr(&address));
            return match abstract_socket {
                Ok(_) => Ok(()),
                Err(_) => UnixStream::connect(&path)
                    .map(drop)
                    .map_err(|error| unreachable(format!("{path}: {error}"))),
            };
        }
        let port = 6000u16.saturating_add(number);
        TcpStream::connect((host, port))
            .map(drop)
            .map_err(|error| unreachable(format!("{host}:{port}: {error}")))
    }

    #[cfg(test)]
    mod tests {
        use std::os::unix::net::UnixListener;

        use super::*;

        fn environment(wayland: Option<&str>, x11: Option<&str>, runtime: &str) -> Environment {
            Environment {
                wayland_display: wayland.map(OsString::from),
                wayland_socket: false,
                x11_display: x11.map(OsString::from),
                xdg_runtime_dir: Some(runtime.into()),
            }
        }

        #[test]
        fn the_backend_is_x11_only_when_no_wayland_display_is_named() {
            assert!(!on_x11(&environment(Some("wayland-0"), Some(":0"), "/run")));
            assert!(on_x11(&environment(Some(""), Some(":0"), "/run")));
            assert!(on_x11(&environment(None, Some(":0"), "/run")));
            assert!(!on_x11(&environment(None, None, "/run")));
        }

        #[test]
        fn no_display_variable_names_both_missing_variables() {
            let error = check(&environment(None, Some(""), "/run")).unwrap_err();
            assert_eq!(
                error,
                "Crow Bot found no display: neither WAYLAND_DISPLAY nor DISPLAY is set."
            );
            assert!(check(&environment(Some(""), None, "/run")).is_err());
        }

        #[test]
        fn a_compositor_that_does_not_answer_is_named() {
            let folder = crowbot_protocol::scratch::Scratch::new("crowbot-display");
            let runtime = folder.to_string_lossy().into_owned();
            let error = check(&environment(Some("wayland-gone"), None, &runtime)).unwrap_err();
            assert!(
                error.starts_with("Crow Bot cannot reach the Wayland compositor wayland-gone ("),
                "{error}"
            );
            assert!(!error.contains('\n'));

            let listener = UnixListener::bind(folder.join("wayland-here")).unwrap();
            assert_eq!(
                check(&environment(Some("wayland-here"), None, &runtime)),
                Ok(())
            );
            drop(listener);
        }

        #[test]
        fn wayland_comes_before_x11_and_an_unreachable_x_server_is_named() {
            let error = check(&environment(
                Some("wayland-gone"),
                Some(":0"),
                "/nonexistent",
            ))
            .unwrap_err();
            assert!(error.contains("Wayland compositor wayland-gone"), "{error}");
            let error = check(&environment(None, Some(":9999.0"), "/run")).unwrap_err();
            assert!(
                error.starts_with(
                    "Crow Bot cannot reach the X server :9999.0: /tmp/.X11-unix/X9999"
                ),
                "{error}"
            );
            assert!(check(&environment(None, Some("nonsense"), "/run")).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_with_no_adapter_names_the_driver_and_the_package() {
        let line = open_failure("No GPU adapter found that can configure the display surface");
        assert_eq!(line, NO_DRIVER);
        assert_eq!(
            open_failure(
                "Failed to create surface: Failed to create surface for any enabled backend: {}"
            ),
            NO_DRIVER
        );
        assert!(line.contains("Vulkan") && line.contains("mesa-vulkan-drivers"));
        assert_eq!(
            open_failure("something else"),
            "Crow Bot cannot open its window: something else"
        );
    }

    #[test]
    fn cpu_rendering_is_said_once_and_a_gpu_says_nothing() {
        let specs = |software| gpui::GpuSpecs {
            is_software_emulated: software,
            device_name: "llvmpipe".to_owned(),
            driver_name: String::new(),
            driver_info: String::new(),
        };
        assert_eq!(
            software_notice(Some(&specs(true))).as_deref(),
            Some("Rendering runs on the CPU (llvmpipe).")
        );
        assert_eq!(software_notice(Some(&specs(false))), None);
        assert_eq!(software_notice(None), None);
    }
}
