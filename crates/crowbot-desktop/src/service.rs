//! The daemon as a systemd user unit, on Linux. When the unit is installed
//! (`scripts/desktop-files.sh`), the client asks the user's systemd to start
//! it instead of starting the daemon itself: the daemon then lives in the
//! unit's control group, not in the one of the program that launched the
//! client, so it outlives that program, and systemd starts it again after a
//! crash. With no unit, or no systemd, the client starts the daemon detached.
//!
//! The client talks to systemd through `systemctl` and reads the daemon's log
//! through `journalctl`, the interfaces systemd documents for its users.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// The name of the unit, which `scripts/desktop-files.sh` installs.
pub const UNIT: &str = "crowbotd.service";

/// The user unit that runs the default daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// The name of the unit: `UNIT`, or one of a test.
    unit: String,
    /// Where the unit file is when it is installed.
    unit_file: PathBuf,
    /// The runtime folder of the user, where the user's systemd keeps its
    /// own socket while it runs.
    runtime_dir: Option<PathBuf>,
}

/// Why the unit did not start.
#[derive(Debug, PartialEq, Eq)]
pub struct StartFailure {
    /// The exit status of the daemon's last run, when it ran and exited.
    pub status: Option<i32>,
    /// What systemd says of the failure.
    pub detail: String,
}

impl Service {
    /// The unit of this user, under `$XDG_CONFIG_HOME/systemd/user`.
    pub fn of_user() -> Option<Self> {
        let absolute = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let config =
            absolute("XDG_CONFIG_HOME").or_else(|| Some(std::env::home_dir()?.join(".config")))?;
        Some(Self {
            unit: UNIT.to_owned(),
            unit_file: config.join("systemd").join("user").join(UNIT),
            runtime_dir: absolute("XDG_RUNTIME_DIR"),
        })
    }

    /// Whether the unit is installed and the user's systemd runs.
    pub fn available(&self) -> bool {
        self.unit_file.is_file()
            && self
                .runtime_dir
                .as_deref()
                .is_some_and(|folder| folder.join("systemd").join("private").exists())
    }

    /// Asks systemd to start the unit, and returns once the daemon said it is
    /// ready (`Type=notify`) or failed. It blocks: run it off the async thread.
    pub fn start(&self) -> Result<(), StartFailure> {
        let output = systemctl(&["start", &self.unit]).map_err(|error| StartFailure {
            status: None,
            detail: format!("cannot run systemctl: {error}"),
        })?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let shown = systemctl(&[
            "show",
            &self.unit,
            "--property=ExecMainStatus,ExecMainCode,Result",
        ])
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
        Err(read_failure(&shown, said))
    }

    /// The last line the daemon of the unit logged, which states why it stopped.
    pub fn last_log_line(&self) -> Option<String> {
        // The daemon's own lines: what its standard error said, not systemd's.
        let output = Command::new("journalctl")
            .args(["--user", "--unit", &self.unit, "--identifier=crowbotd"])
            .args(["--lines=20", "--output=cat", "--no-pager"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    }

    #[cfg(test)]
    pub fn at(unit_file: &Path, runtime_dir: &Path) -> Self {
        Self {
            unit: unit_file.file_name().map_or_else(
                || UNIT.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
            unit_file: unit_file.to_owned(),
            runtime_dir: Some(runtime_dir.to_owned()),
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub fn unit(&self) -> &str {
        &self.unit
    }
}

fn systemctl(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("systemctl").arg("--user").args(args).output()
}

/// Reads the properties `systemctl show` printed. `ExecMainCode=1` means
/// that the daemon exited (CLD_EXITED), so `ExecMainStatus` is its exit
/// status; otherwise a signal ended it, or it never ran.
fn read_failure(shown: &str, said: String) -> StartFailure {
    let property = |name: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
    };
    let exited = property("ExecMainCode") == Some("1");
    let status = property("ExecMainStatus")
        .and_then(|status| status.parse::<i32>().ok())
        .filter(|_| exited);
    let detail = match property("Result") {
        Some(result) if !result.is_empty() && result != "success" => {
            format!("systemd reports {result}")
        }
        _ if !said.is_empty() => said,
        _ => "systemd did not start it".to_owned(),
    };
    StartFailure { status, detail }
}

/// The unit file and the runtime folder of a test, for `Service::at`.
#[cfg(test)]
pub fn unit_path(config: &Path) -> PathBuf {
    config.join("systemd").join("user").join(UNIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_counts_only_when_its_file_and_the_users_systemd_are_there() {
        let dir = crowbot_protocol::scratch::Scratch::new("crowbot-service");
        let unit = unit_path(&dir.join("config"));
        let runtime = dir.join("runtime");
        let service = Service::at(&unit, &runtime);
        assert!(!service.available());
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, "[Service]\n").unwrap();
        assert!(!service.available(), "no systemd runs for the user");
        std::fs::create_dir_all(runtime.join("systemd")).unwrap();
        std::fs::write(runtime.join("systemd").join("private"), "").unwrap();
        assert!(service.available());
    }

    #[test]
    fn a_failure_gives_the_exit_status_only_when_the_daemon_exited() {
        let exited = read_failure(
            "ExecMainStatus=5\nExecMainCode=1\nResult=exit-code\n",
            String::new(),
        );
        assert_eq!(exited.status, Some(5));
        assert_eq!(exited.detail, "systemd reports exit-code");
        let killed = read_failure(
            "ExecMainStatus=9\nExecMainCode=2\nResult=signal\n",
            String::new(),
        );
        assert_eq!(killed.status, None);
        let unknown = read_failure("", "Unit crowbotd.service not found.".to_owned());
        assert_eq!(unknown.status, None);
        assert_eq!(unknown.detail, "Unit crowbotd.service not found.");
    }
}
