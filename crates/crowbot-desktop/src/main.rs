#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod bots;
mod chrome;
mod composer;
mod connection;
mod display;
mod frames;
#[cfg(test)]
mod gpui_patches;
mod identity;
#[cfg(target_os = "linux")]
mod kwin;
mod layout;
mod markdown;
mod mascot;
mod menu;
mod reduced_motion;
mod reveal;
mod service;
mod sidebar;
mod theme;
mod transcript;
mod truncated;
mod ui;
mod workspace;

use std::cell::Cell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use crowbot_protocol::Endpoint;
use gpui::{App, Bounds, Pixels, TitlebarOptions, WindowBounds, WindowOptions, prelude::*};
use gpui_platform::application;

use crate::connection::DaemonLauncher;
use crate::service::Service;
use crate::workspace::Workspace;

const USAGE: &str =
    "usage: crowbot [--endpoint <address>] [--daemon <path>] [-- <daemon arguments>]
       crowbot --help | --version

The client attaches to the daemon on the endpoint. When none listens, it starts
`crowbotd` from the folder of its own executable, or from --daemon <path>, and
passes it the endpoint and the arguments that follow `--`. On Linux, with none of
these options and the systemd user unit crowbotd.service installed, it asks the
user's systemd to start that unit instead.";

/// What the arguments ask for: a window to open, or a text to print.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run(Args),
    Print(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    endpoint: Option<Endpoint>,
    daemon: Option<PathBuf>,
    daemon_args: Vec<OsString>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = Args::default();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--endpoint" => {
                args.endpoint = Some(Endpoint::new(
                    raw.next().ok_or("--endpoint needs an address")?,
                ));
            }
            "--daemon" => {
                args.daemon = Some(PathBuf::from(raw.next().ok_or("--daemon needs a path")?));
            }
            "--" => {
                args.daemon_args = raw.map(OsString::from).collect();
                break;
            }
            "--help" | "-h" => return Ok(Command::Print(USAGE.to_owned())),
            "--version" | "-V" => {
                return Ok(Command::Print(format!(
                    "crowbot {}",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            _ => return Err(format!("unexpected argument: {arg}\n\n{USAGE}")),
        }
    }
    Ok(Command::Run(args))
}

/// How to start the daemon that serves `endpoint`, when its binary and a place
/// for its log can be found. A client given an endpoint, a daemon, or daemon
/// arguments runs a daemon of its own, such as a sandbox: it starts it
/// detached and never touches the systemd unit, which runs the default one.
fn launcher(args: Args, endpoint: &Endpoint, service: Option<Service>) -> Option<DaemonLauncher> {
    let default = args.endpoint.is_none() && args.daemon.is_none() && args.daemon_args.is_empty();
    let log = connection::daemon_log(&args.daemon_args)?;
    let mut daemon_args: Vec<OsString> = vec!["--endpoint".into(), endpoint.address().into()];
    daemon_args.extend(args.daemon_args);
    Some(DaemonLauncher {
        program: args.daemon.or_else(connection::bundled_daemon)?,
        args: daemon_args,
        log,
        service: service.filter(|_| default),
    })
}

/// The user unit of the daemon, on Linux.
fn user_service() -> Option<Service> {
    if cfg!(target_os = "linux") {
        Service::of_user()
    } else {
        None
    }
}

fn window_options(bounds: Bounds<Pixels>, x11: bool) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(chrome::min_size(x11)),
        window_decorations: chrome::requested_decorations(),
        titlebar: Some(TitlebarOptions {
            title: Some(identity::APP_NAME.into()),
            appears_transparent: true,
            ..Default::default()
        }),
        app_id: Some(identity::APP_ID.to_owned()),
        ..Default::default()
    }
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Command::Run(args)) => args,
        Ok(Command::Print(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let Err(reason) = display::check() {
        eprintln!("{reason}");
        return ExitCode::FAILURE;
    }
    let Some(endpoint) = args.endpoint.clone().or_else(Endpoint::local) else {
        eprintln!("cannot choose a local endpoint; pass --endpoint <address>");
        return ExitCode::FAILURE;
    };
    let launcher = launcher(args, &endpoint, user_service());
    let (connection, events) = match connection::start(endpoint, launcher) {
        Ok(started) => started,
        Err(error) => {
            eprintln!("cannot start the daemon connection: {error}");
            return ExitCode::FAILURE;
        }
    };

    let failed = Rc::new(Cell::new(false));
    let window_failed = failed.clone();
    application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            if let Err(error) = cx.text_system().add_fonts(assets::fonts()) {
                eprintln!("cannot load the interface font: {error}");
            }
            theme::font::init(cx);
            reduced_motion::follow(cx);
            #[cfg(target_os = "linux")]
            kwin::refresh(cx);
            composer::bind_keys(cx);
            workspace::bind_keys(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, chrome::default_size(), cx);
            let opened = cx.open_window(window_options(bounds, display::x11()), |window, cx| {
                if let Some(notice) = display::software_notice(window.gpu_specs().as_ref()) {
                    eprintln!("{notice}");
                }
                cx.new(|cx| Workspace::new(connection, events, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("{}", display::open_failure(&format!("{error:#}")));
                window_failed.set(true);
                // A quit asked before the event loop runs is lost: the loop
                // clears it when it starts. Ask it from a task of the loop.
                cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
                return;
            }
            cx.activate(true);
        });
    if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        match parse_args(args.iter().map(|arg| (*arg).to_owned()))? {
            Command::Run(args) => Ok(args),
            Command::Print(text) => Err(text),
        }
    }

    #[test]
    fn arguments_after_a_double_dash_go_to_the_daemon() {
        let args = parse(&[
            "--endpoint",
            "here",
            "--daemon",
            "bin/crowbotd",
            "--",
            "--db",
            "test.db",
            "--endpoint",
            "ignored",
        ])
        .unwrap();
        assert_eq!(args.endpoint, Some(Endpoint::new("here")));
        assert_eq!(args.daemon, Some(PathBuf::from("bin/crowbotd")));
        assert_eq!(
            args.daemon_args,
            ["--db", "test.db", "--endpoint", "ignored"]
        );
    }

    #[test]
    fn the_window_carries_the_application_id_and_the_name() {
        let bounds = Bounds::new(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            chrome::default_size(),
        );
        for x11 in [false, true] {
            let options = window_options(bounds, x11);
            assert_eq!(options.app_id.as_deref(), Some(identity::APP_ID));
            assert_eq!(
                options
                    .titlebar
                    .and_then(|titlebar| titlebar.title)
                    .as_deref(),
                Some(identity::APP_NAME)
            );
            assert_eq!(options.window_decorations, chrome::requested_decorations());
            assert_eq!(options.window_min_size, Some(chrome::min_size(x11)));
        }
    }

    /// Windows opens the window it opened before the Linux port. Its GPUI
    /// backend reads neither `app_id` nor `window_decorations`
    /// (`gpui_windows`, the default no-op `set_app_id` and
    /// `request_decorations` of `PlatformWindow`), so the options it reads are
    /// compared with the ones of that time, written out.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_opens_the_window_it_opened_before_the_linux_port() {
        use gpui::{px, size};

        let before_bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(1080.), px(760.)));
        let before = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(before_bounds)),
            window_min_size: Some(size(px(640.), px(420.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Crow Bot".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let bounds = Bounds::new(gpui::point(px(0.), px(0.)), chrome::default_size());
        let now = window_options(bounds, display::x11());

        assert_eq!(now.window_bounds, before.window_bounds);
        assert_eq!(now.window_min_size, before.window_min_size);
        let (now_bar, before_bar) = (now.titlebar.unwrap(), before.titlebar.unwrap());
        assert_eq!(now_bar.title, before_bar.title);
        assert_eq!(now_bar.appears_transparent, before_bar.appears_transparent);
        assert_eq!(
            now_bar.traffic_light_position,
            before_bar.traffic_light_position
        );
        assert_eq!(now.kind, before.kind);
        assert_eq!(now.is_movable, before.is_movable);
        assert_eq!(now.is_resizable, before.is_resizable);
        assert_eq!(now.is_minimizable, before.is_minimizable);
        assert_eq!(now.show, before.show);
        assert_eq!(now.focus, before.focus);
        assert_eq!(now.display_id, before.display_id);
        assert_eq!(now.window_background, before.window_background);
        assert_eq!(now.app_owns_titlebar_drag, before.app_owns_titlebar_drag);
        assert_eq!(now.inactive_frame_interval, before.inactive_frame_interval);
        assert_eq!(
            now.low_power_frame_interval,
            before.low_power_frame_interval
        );
        assert_eq!(now.thermal_frame_interval, before.thermal_frame_interval);
        // What Windows does not read stays as Linux needs it, and asks for no
        // decorations of the client's own.
        assert_eq!(now.window_decorations, None);
        assert!(!display::x11());
    }

    #[test]
    fn the_daemon_is_started_on_the_endpoint_of_the_client() {
        let endpoint = Endpoint::new("here");
        let args = parse(&["--daemon", "bin/crowbotd", "--", "--db", "test.db"]).unwrap();
        let launcher = launcher(args, &endpoint, None).unwrap();
        assert_eq!(launcher.program, PathBuf::from("bin/crowbotd"));
        assert_eq!(launcher.args, ["--endpoint", "here", "--db", "test.db"]);
    }

    #[test]
    fn the_log_of_a_daemon_with_its_own_database_sits_beside_that_database() {
        let sandbox = std::env::temp_dir().join("sandbox");
        let socket = sandbox.join("crowbotd.sock").to_string_lossy().into_owned();
        let db = sandbox.join("crowbot.db").to_string_lossy().into_owned();
        let endpoint = Endpoint::new(socket.clone());
        let args = parse(&["--endpoint", &socket, "--", "--db", &db]).unwrap();
        let launcher = launcher(args, &endpoint, None).unwrap();
        assert_eq!(launcher.log, sandbox.join("crowbotd.log"));

        let launcher = launcher_with_defaults(&[]);
        let folders = crowbot_protocol::Folders::local().unwrap();
        assert_eq!(launcher.log, folders.log());
    }

    fn launcher_with_defaults(raw: &[&str]) -> DaemonLauncher {
        let args = parse(raw).unwrap();
        let endpoint = args.endpoint.clone().or_else(Endpoint::local).unwrap();
        let service = Service::at(Path::new("/unit"), Path::new("/run"));
        launcher(args, &endpoint, Some(service)).unwrap()
    }

    #[test]
    fn only_the_default_daemon_is_started_through_the_unit() {
        assert!(launcher_with_defaults(&[]).service.is_some());
        for raw in [
            &["--endpoint", "/elsewhere.sock"][..],
            &["--daemon", "/bin/crowbotd"][..],
            &["--", "--db", "/sandbox/crowbot.db"][..],
        ] {
            assert!(launcher_with_defaults(raw).service.is_none(), "{raw:?}");
        }
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        assert!(parse(&["--db", "test.db"]).is_err());
        assert!(parse(&["--daemon"]).is_err());
    }
}
