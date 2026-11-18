//! The client's command line, run as the built binary: `--version` and
//! `--help` print on standard output and exit 0 before any window or daemon,
//! and a wrong argument exits 2 on the error stream.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output};

fn crowbot(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crowbot"))
        .args(args)
        // No display: the binary must print and exit before it looks for one.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .unwrap()
}

#[test]
fn the_version_prints_on_standard_output() {
    let output = crowbot(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("crowbot {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn the_usage_prints_on_standard_output() {
    for flag in ["--help", "-h"] {
        let output = crowbot(&[flag]);
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .starts_with("usage: crowbot"),
            "{flag}"
        );
        assert!(output.stderr.is_empty(), "{flag}");
    }
}

#[test]
fn a_wrong_argument_exits_2_on_the_error_stream() {
    let output = crowbot(&["--no-such-flag"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.starts_with("unexpected argument: --no-such-flag"),
        "{error}"
    );
    assert!(error.contains("usage: crowbot"), "{error}");
}
