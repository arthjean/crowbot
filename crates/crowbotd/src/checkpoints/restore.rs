//! Writes files of a folder back as a record holds them. The removals go
//! first, the deepest first, so a file that takes the place of a folder or
//! a link finds its place free; then each file and link is written. A file
//! is written to a file of its own in the same folder, then moved into
//! place. No write goes through a link: every folder above a path must be a
//! folder of the bot's folder, so a link an agent left cannot send a
//! restore outside it.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use crowbot_protocol::{LeftCause, LeftFile};

use super::objects::Objects;
use super::tree::{Difference, Node};

/// What a restore did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    pub left: Vec<LeftFile>,
}

fn failed(path: &str, error: impl std::fmt::Display) -> LeftFile {
    LeftFile {
        path: path.to_owned(),
        cause: LeftCause::Failed,
        message: Some(error.to_string()),
    }
}

pub(super) fn place(folder: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(folder.to_owned(), |place, part| place.join(part))
}

/// Checks that each folder above `path` is a folder and no link, creating
/// the ones that are missing when `create` is set.
fn parents(folder: &Path, path: &str, create: bool) -> std::io::Result<()> {
    let mut place = folder.to_owned();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        place.push(part);
        match place.symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is not a folder",
                    place.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&place)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Applies the differences: each path takes the state `before` holds, the
/// record's. `folders` are the folders of that record, which stay when
/// they are left empty.
pub fn apply(
    folder: &Path,
    differences: &[Difference],
    folders: &BTreeSet<String>,
    objects: &Objects,
) -> Done {
    let mut done = Done::default();
    let mut removals: Vec<&Difference> = differences
        .iter()
        .filter(|difference| difference.before.is_none())
        .collect();
    removals.sort_by_key(|difference| std::cmp::Reverse(difference.path.matches('/').count()));
    let mut emptied = BTreeSet::new();
    for difference in removals {
        let path = &difference.path;
        let removed = parents(folder, path, false).and_then(|()| remove(&place(folder, path)));
        match removed {
            Ok(()) => {
                done.removed.push(path.clone());
                if let Some((parent, _)) = path.rsplit_once('/') {
                    emptied.insert(parent.to_owned());
                }
            }
            Err(error) => done.left.push(failed(path, error)),
        }
    }
    prune(folder, emptied, folders);
    for difference in differences {
        let Some(node) = &difference.before else {
            continue;
        };
        let path = &difference.path;
        match write(folder, path, node, objects) {
            Ok(()) => done.restored.push(path.clone()),
            Err(left) => done.left.push(left),
        }
    }
    done.restored.sort();
    done.removed.sort();
    done
}

fn remove(place: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(place) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// Removes the folders that a removal left empty and that the record did
/// not hold, up toward the bot's folder.
fn prune(folder: &Path, emptied: BTreeSet<String>, kept: &BTreeSet<String>) {
    let mut emptied: Vec<String> = emptied.into_iter().collect();
    emptied.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    for mut path in emptied {
        loop {
            if kept.contains(&path) || std::fs::remove_dir(place(folder, &path)).is_err() {
                break;
            }
            match path.rsplit_once('/') {
                Some((parent, _)) => path = parent.to_owned(),
                None => break,
            }
        }
    }
}

fn write(folder: &Path, path: &str, node: &Node, objects: &Objects) -> Result<(), LeftFile> {
    let target = place(folder, path);
    match node {
        Node::File {
            sha256,
            executable,
            kept,
            ..
        } => {
            if !kept {
                return Err(LeftFile {
                    path: path.to_owned(),
                    cause: LeftCause::NotKept,
                    message: None,
                });
            }
            let content = objects.get(sha256).ok_or_else(|| LeftFile {
                path: path.to_owned(),
                cause: LeftCause::ContentLost,
                message: None,
            })?;
            parents(folder, path, true).map_err(|error| failed(path, error))?;
            write_file(&target, &content, *executable).map_err(|error| failed(path, error))
        }
        Node::Link { target: link } => {
            parents(folder, path, true).map_err(|error| failed(path, error))?;
            remove(&target).map_err(|error| failed(path, error))?;
            make_link(link, &target).map_err(|error| failed(path, error))
        }
        Node::Folder { .. } => Ok(()),
    }
}

fn write_file(target: &Path, content: &[u8], executable: bool) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let partial = target.with_file_name(format!(".{name}.crowbot-restore-{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial)?;
        file.write_all(content)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = file.metadata()?.permissions().mode();
            let mode = if executable {
                mode | ((mode & 0o444) >> 2)
            } else {
                mode & !0o111
            };
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        drop(file);
        std::fs::rename(&partial, target)
    })();
    if written.is_err() {
        std::fs::remove_file(&partial).ok();
    }
    written
}

#[cfg(unix)]
fn make_link(link: &str, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, target)
}

#[cfg(windows)]
fn make_link(link: &str, target: &Path) -> std::io::Result<()> {
    let resolved = target
        .parent()
        .map_or_else(|| PathBuf::from(link), |parent| parent.join(link));
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(link, target)
    } else {
        std::os::windows::fs::symlink_file(link, target)
    }
}
