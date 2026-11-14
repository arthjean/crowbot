#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod composer;
mod connection;
mod theme;
mod transcript;
mod workspace;

use std::process::ExitCode;

use crowbot_protocol::Endpoint;
use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_platform::application;

use crate::workspace::Workspace;

const USAGE: &str = "usage: crowbot [--endpoint <address>]";

fn parse_args(mut raw: impl Iterator<Item = String>) -> Result<Option<Endpoint>, String> {
    let mut endpoint = None;
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--endpoint" => {
                endpoint = Some(Endpoint::new(
                    raw.next().ok_or("--endpoint needs an address")?,
                ));
            }
            "--help" | "-h" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unexpected argument: {arg}\n\n{USAGE}")),
        }
    }
    Ok(endpoint)
}

fn main() -> ExitCode {
    let endpoint = match parse_args(std::env::args().skip(1)) {
        Ok(endpoint) => endpoint.or_else(Endpoint::local),
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let Some(endpoint) = endpoint else {
        eprintln!("cannot choose a local endpoint; pass --endpoint <address>");
        return ExitCode::FAILURE;
    };
    let (connection, events) = match connection::start(endpoint) {
        Ok(started) => started,
        Err(error) => {
            eprintln!("cannot start the daemon connection: {error}");
            return ExitCode::FAILURE;
        }
    };

    application().run(move |cx: &mut App| {
        composer::bind_keys(cx);
        workspace::bind_keys(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1080.), px(760.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Crow Bot".into()),
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
