//! Where Crow Bot keeps its files when no flag says otherwise. The daemon and
//! the client both read them here, so they agree on the endpoint and on the
//! place of the log.
//!
//! On Linux the folders follow the XDG Base Directory specification (0.8):
//! the database, the runtimes, and the provider homes under
//! `$XDG_DATA_HOME/crowbot`, the log under `$XDG_STATE_HOME/crowbot`, and the
//! socket and its lock under `$XDG_RUNTIME_DIR/crowbot`. A variable that is
//! unset, empty, or relative is ignored, as the specification says. With no
//! runtime folder, the socket goes in `run` under the data folder, and the
//! daemon warns about it.
//!
//! Elsewhere everything stays in `~/.crowbot`.

use std::path::{Path, PathBuf};

/// The name of Crow Bot's folder inside each XDG base folder.
#[cfg(target_os = "linux")]
const NAME: &str = "crowbot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    /// The database, the runtimes, and the provider homes.
    pub data: PathBuf,
    /// The log of a daemon that the client started.
    pub state: PathBuf,
    /// The socket and its lock.
    pub runtime: PathBuf,
    /// The system gave no runtime folder, so `runtime` is a folder of Crow
    /// Bot's own.
    pub runtime_missing: bool,
}

impl Folders {
    /// The folders of this user, or `None` when the home folder is unknown.
    pub fn local() -> Option<Self> {
        Self::from_env(std::env::home_dir()?, |name| std::env::var_os(name))
    }

    /// The folders for a home folder and a source of environment variables.
    #[cfg(target_os = "linux")]
    pub fn from_env(
        home: PathBuf,
        var: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Option<Self> {
        let base = |name: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let data = base("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join(NAME);
        let state = base("XDG_STATE_HOME")
            .unwrap_or_else(|| home.join(".local").join("state"))
            .join(NAME);
        let (runtime, runtime_missing) = match base("XDG_RUNTIME_DIR") {
            Some(folder) => (folder.join(NAME), false),
            None => (data.join("run"), true),
        };
        Some(Self {
            data,
            state,
            runtime,
            runtime_missing,
        })
    }

    /// The folders for a home folder: all of them are `~/.crowbot`.
    #[cfg(not(target_os = "linux"))]
    pub fn from_env(
        home: PathBuf,
        _var: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Option<Self> {
        let folder = home.join(".crowbot");
        Some(Self {
            data: folder.clone(),
            state: folder.clone(),
            runtime: folder,
            runtime_missing: false,
        })
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("crowbot.db")
    }

    pub fn log(&self) -> PathBuf {
        self.state.join("crowbotd.log")
    }

    pub fn socket(&self) -> PathBuf {
        self.runtime.join("crowbotd.sock")
    }

    /// Whether `path` lies in one of these folders.
    pub fn holds(&self, path: &Path) -> bool {
        [&self.data, &self.state, &self.runtime]
            .iter()
            .any(|folder| path.starts_with(folder))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;

    use super::*;

    fn folders(vars: &[(&str, &str)]) -> Option<Folders> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        Folders::from_env(PathBuf::from("/home/someone"), |name| {
            vars.get(name).cloned()
        })
    }

    #[test]
    fn with_no_variable_the_folders_are_the_defaults_of_the_specification() {
        let found = folders(&[]).unwrap();
        assert_eq!(found.data, Path::new("/home/someone/.local/share/crowbot"));
        assert_eq!(found.state, Path::new("/home/someone/.local/state/crowbot"));
        assert_eq!(
            found.runtime,
            Path::new("/home/someone/.local/share/crowbot/run")
        );
        assert!(found.runtime_missing);
    }

    #[test]
    fn the_variables_place_each_folder() {
        let found = folders(&[
            ("XDG_DATA_HOME", "/data"),
            ("XDG_STATE_HOME", "/state"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ])
        .unwrap();
        assert_eq!(found.database(), Path::new("/data/crowbot/crowbot.db"));
        assert_eq!(found.log(), Path::new("/state/crowbot/crowbotd.log"));
        assert_eq!(
            found.socket(),
            Path::new("/run/user/1000/crowbot/crowbotd.sock")
        );
        assert!(!found.runtime_missing);
    }

    #[test]
    fn a_relative_or_empty_variable_is_ignored() {
        let found = folders(&[
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", ""),
            ("XDG_RUNTIME_DIR", "./run"),
        ])
        .unwrap();
        assert_eq!(found, folders(&[]).unwrap());
    }
}
