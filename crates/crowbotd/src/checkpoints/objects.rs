//! The contents records keep: one file per content, named by its SHA-256,
//! under `<data>/checkpoints/<database>/objects/<first two digits>/<the rest>`. A
//! content is written once, to a file of its own first and then moved into
//! place, so a reader never meets half a content. On Unix the folders are
//! 0700 and the files 0600 (`private.rs`).

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::digest::sha256_hex;
use crate::private;

pub struct Objects {
    root: PathBuf,
    next: AtomicU64,
}

/// Where a content that is being written waits: a name no hash takes.
const PARTIAL: &str = ".partial-";

impl Objects {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            next: AtomicU64::new(0),
        }
    }

    fn path(&self, sha256: &str) -> Option<PathBuf> {
        let valid = sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
        valid.then(|| self.root.join(&sha256[..2]).join(&sha256[2..]))
    }

    pub fn contains(&self, sha256: &str) -> bool {
        self.path(sha256)
            .is_some_and(|path| path.symlink_metadata().is_ok_and(|meta| meta.is_file()))
    }

    /// Stores a content under its hash, unless it is stored already.
    /// `true` when this call wrote it.
    pub fn put(&self, sha256: &str, content: &[u8]) -> std::io::Result<bool> {
        let path = self
            .path(sha256)
            .ok_or_else(|| std::io::Error::other(format!("{sha256} is not a SHA-256")))?;
        if path.symlink_metadata().is_ok() {
            return Ok(false);
        }
        let folder = path.parent().unwrap_or(&self.root);
        private::create_folder(folder)?;
        let partial = folder.join(format!(
            "{PARTIAL}{}-{}",
            std::process::id(),
            self.next.fetch_add(1, Ordering::Relaxed)
        ));
        let written = private::options()
            .create_new(true)
            .write(true)
            .open(&partial)
            // No sync: a content lost to a power cut no longer matches its
            // hash, and a restore names it instead of writing it.
            .and_then(|mut file| file.write_all(content))
            .and_then(|()| std::fs::rename(&partial, &path));
        if written.is_err() {
            std::fs::remove_file(&partial).ok();
        }
        written.map(|()| true)
    }

    /// The content stored under a hash, when it is there and still matches it.
    pub fn get(&self, sha256: &str) -> Option<Vec<u8>> {
        let content = std::fs::read(self.path(sha256)?).ok()?;
        (sha256_hex(&content) == sha256).then_some(content)
    }

    /// Removes every content whose hash is not in `kept`, and what a write
    /// that did not finish left. The count of files removed.
    pub fn sweep(&self, kept: &HashSet<String>) -> usize {
        let Ok(folders) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for folder in folders.flatten() {
            let prefix = folder.file_name().to_string_lossy().into_owned();
            let Ok(files) = std::fs::read_dir(folder.path()) else {
                continue;
            };
            for file in files.flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                let hash = format!("{prefix}{name}");
                if !kept.contains(&hash) && std::fs::remove_file(file.path()).is_ok() {
                    removed += 1;
                }
            }
            // A folder left empty goes too; one that still holds a content stays.
            std::fs::remove_dir(folder.path()).ok();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::scratch::Scratch;

    #[test]
    fn a_content_is_stored_once_under_its_hash() {
        let dir = Scratch::new("objects");
        let objects = Objects::new(dir.path().join("objects"));
        let hash = sha256_hex(b"one");
        assert!(objects.put(&hash, b"one").unwrap());
        assert!(!objects.put(&hash, b"one").unwrap());
        assert!(objects.contains(&hash));
        assert_eq!(objects.get(&hash).as_deref(), Some(&b"one"[..]));
        let stored = dir.path().join("objects").join(&hash[..2]).join(&hash[2..]);
        assert!(stored.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |path: &std::path::Path| path.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&stored), 0o600);
            assert_eq!(mode(stored.parent().unwrap()), 0o700);
        }
    }

    #[test]
    fn a_content_that_no_longer_matches_its_hash_is_not_read() {
        let dir = Scratch::new("objects");
        let objects = Objects::new(dir.path().join("objects"));
        let hash = sha256_hex(b"one");
        objects.put(&hash, b"one").unwrap();
        std::fs::write(objects.path(&hash).unwrap(), b"two").unwrap();
        assert_eq!(objects.get(&hash), None);
        assert_eq!(objects.get("not a hash"), None);
    }

    #[test]
    fn a_sweep_keeps_only_the_contents_named() {
        let dir = Scratch::new("objects");
        let objects = Objects::new(dir.path().join("objects"));
        let (kept, gone) = (sha256_hex(b"kept"), sha256_hex(b"gone"));
        objects.put(&kept, b"kept").unwrap();
        objects.put(&gone, b"gone").unwrap();
        assert_eq!(objects.sweep(&HashSet::from([kept.clone()])), 1);
        assert!(objects.contains(&kept));
        assert!(!objects.contains(&gone));
    }
}
