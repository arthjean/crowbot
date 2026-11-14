#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod bots;
mod composer;
mod connection;
mod layout;
mod menu;
mod sidebar;
mod theme;
mod transcript;
mod ui;
mod workspace;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crowbot_protocol::Endpoint;
use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, size};
use gpui_platform::application;

use crate::connection::DaemonLauncher;
use crate::theme::layout::{WINDOW_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH, WINDOW_WIDTH};
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

fn main() -> ExitCode {
    let mut args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
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

    application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            if let Err(error) = cx.text_system().add_fonts(assets::fonts()) {
                eprintln!("cannot load the interface font: {error}");
            }
            composer::bind_keys(cx);
            workspace::bind_keys(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(WINDOW_WIDTH, WINDOW_HEIGHT), cx);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Crow Bot".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(connection, events, window, cx)),
            );
            if opened.is_err() {
                cx.quit();
                return;
            }
            cx.activate(true);
        });
    ExitCode::SUCCESS
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
