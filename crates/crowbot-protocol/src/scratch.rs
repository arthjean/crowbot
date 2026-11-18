//! Folders for tests, shared by the tests of the daemon and of the client.
//!
//! A test folder often holds a Unix socket, and a socket path holds at most
//! `SOCKET_PATH_LIMIT` bytes. Under a long `TMPDIR` (a sandbox, a CI runner,
//! macOS) a socket in the temporary folder would not fit, so a test folder
//! sits in the temporary folder when a socket inside it fits, and in `/tmp`
//! otherwise. A folder removes itself when it is dropped, also when its test
//! panics, so a run leaves nothing behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{Endpoint, SOCKET_PATH_LIMIT};

/// The bytes a scratch folder keeps free for its own name and a socket path
/// inside it: `<prefix>-<pid>-<n>/crowbotd.sock` with a prefix of up to 32
/// bytes takes 66, and a test that nests the socket in XDG folders a few more.
const NAME_ROOM: usize = 80;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The folder that test folders are made in.
pub fn root() -> PathBuf {
    let temp = std::env::temp_dir();
    if cfg!(unix) && temp.as_os_str().len() + NAME_ROOM > SOCKET_PATH_LIMIT {
        PathBuf::from("/tmp")
    } else {
        temp
    }
}

/// A folder of a test, removed with everything in it when dropped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A new, empty folder named after `prefix`, the process, and a counter,
    /// so that tests that run at once, in one process or in several, never
    /// share one. On Unix only its owner may enter it.
    #[expect(clippy::panic, reason = "a test cannot run without its folder")]
    pub fn new(prefix: &str) -> Self {
        let name = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = root().join(name);
        // A folder of the same name was left by an earlier process of the
        // same id that did not end cleanly.
        std::fs::remove_dir_all(&path).ok();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        if let Err(error) = builder.create(&path) {
            panic!("cannot create the test folder {}: {error}", path.display());
        }
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// An endpoint of its own for this folder: a socket inside it on Unix,
    /// a pipe named after it on Windows.
    pub fn endpoint(&self) -> Endpoint {
        if cfg!(windows) {
            let name = self.path.file_name().unwrap_or_default().to_string_lossy();
            Endpoint::new(format!(r"\\.\pipe\{name}"))
        } else {
            Endpoint::new(
                self.path
                    .join("crowbotd.sock")
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.path.as_os_str()
    }
}

/// A program the test started may still write in the folder while it goes:
/// the removal is tried again until the folder is gone, for a second at most.
impl Drop for Scratch {
    fn drop(&mut self) {
        #[cfg(unix)]
        restore_permissions(&self.path);
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.path).is_ok() || !self.path.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Gives the owner back the right to change every folder of the tree, which
/// a test may have taken away, so that the tree can be removed.
#[cfg(unix)]
fn restore_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700)).ok();
    }
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            restore_permissions(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_made_private_and_removed_with_its_contents() {
        let scratch = Scratch::new("crowbot-scratch-test");
        let path = scratch.to_path_buf();
        std::fs::create_dir_all(path.join("inner")).unwrap();
        std::fs::write(path.join("inner").join("file"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
            std::fs::set_permissions(path.join("inner"), std::fs::Permissions::from_mode(0o555))
                .unwrap();
        }
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn a_socket_in_a_folder_always_fits() {
        let scratch = Scratch::new("a-prefix-of-thirty-two-bytes-xyz");
        let endpoint = scratch.endpoint();
        assert_eq!(endpoint.too_long(), None, "{endpoint}");
    }
}
