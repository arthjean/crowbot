#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod bots;
mod chrome;
mod composer;
mod connection;
mod display;
mod frames;
mod identity;
mod layout;
mod markdown;
mod mascot;
mod menu;
mod reveal;
mod sidebar;
mod theme;
mod transcript;
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
use crate::workspace::Workspace;

const USAGE: &str =
    "usage: crowbot [--endpoint <address>] [--daemon <path>] [-- <daemon arguments>]

The client attaches to the daemon on the endpoint. When none listens, it starts
`crowbotd` from the folder of its own executable, or from --daemon <path>, and
passes it the endpoint and the arguments that follow `--`.";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    endpoint: Option<Endpoint>,
    daemon: Option<PathBuf>,
    daemon_args: Vec<OsString>,
}

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
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
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unexpected argument: {arg}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

/// How to start the daemon that serves `endpoint`, when its binary and a place
/// for its log can be found.
fn launcher(args: Args, endpoint: &Endpoint) -> Option<DaemonLauncher> {
    let mut daemon_args: Vec<OsString> = vec!["--endpoint".into(), endpoint.address().into()];
    daemon_args.extend(args.daemon_args);
    Some(DaemonLauncher {
        program: args.daemon.or_else(connection::bundled_daemon)?,
        args: daemon_args,
        log: connection::default_daemon_log()?,
    })
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
    let mut args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let Err(reason) = display::check() {
        eprintln!("{reason}");
        return ExitCode::FAILURE;
    }
    let Some(endpoint) = args.endpoint.take().or_else(Endpoint::local) else {
        eprintln!("cannot choose a local endpoint; pass --endpoint <address>");
        return ExitCode::FAILURE;
    };
    let launcher = launcher(args, &endpoint);
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
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
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

    #[test]
    fn the_daemon_is_started_on_the_endpoint_of_the_client() {
        let endpoint = Endpoint::new("here");
        let args = parse(&["--daemon", "bin/crowbotd", "--", "--db", "test.db"]).unwrap();
        let launcher = launcher(args, &endpoint).unwrap();
        assert_eq!(launcher.program, PathBuf::from("bin/crowbotd"));
        assert_eq!(launcher.args, ["--endpoint", "here", "--db", "test.db"]);
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        assert!(parse(&["--db", "test.db"]).is_err());
        assert!(parse(&["--daemon"]).is_err());
    }
}
