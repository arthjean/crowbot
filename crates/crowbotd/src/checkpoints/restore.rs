//! Writes files of a folder back as a record holds them. The removals go
//! first, the deepest first, so a file that takes the place of a folder or
//! a link finds its place free; then each file and link is written. A file
//! is written to a file of its own in the same folder, then moved into
//! place. Every path is reached from a handle on the bot's folder, one
//! folder at a time, and no link is followed (`beneath.rs`): a link an agent
//! left, or swaps in while the restore runs, cannot send it outside.

use std::collections::BTreeSet;
use std::path::Path;

use crowbot_protocol::{LeftCause, LeftFile};

use super::beneath::{Root, split};
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

/// Applies the differences: each path takes the state `before` holds, the
/// record's. `folders` are the folders of that record, which stay when
/// they are left empty, and `missing` those of them the folder lacks, made
/// once the files are written. `extra` are folders the record lacks, which
/// go when nothing is left in them.
pub fn apply(
    folder: &Path,
    differences: &[Difference],
    folders: &BTreeSet<String>,
    missing: &[String],
    extra: &[String],
    objects: &Objects,
) -> Done {
    let mut done = Done::default();
    let root = match Root::open(folder) {
        Ok(root) => root,
        Err(error) => {
            done.left = differences
                .iter()
                .map(|difference| failed(&difference.path, &error))
                .collect();
            return done;
        }
    };
    let mut removals: Vec<&Difference> = differences
        .iter()
        .filter(|difference| difference.before.is_none())
        .collect();
    removals.sort_by_key(|difference| std::cmp::Reverse(difference.path.matches('/').count()));
    let mut emptied: BTreeSet<String> = extra.iter().cloned().collect();
    for difference in removals {
        let path = &difference.path;
        match remove(&root, path) {
            Ok(()) => {
                done.removed.push(path.clone());
                if let Some((parent, _)) = path.rsplit_once('/') {
                    emptied.insert(parent.to_owned());
                }
            }
            Err(error) => done.left.push(failed(path, error)),
        }
    }
    prune(&root, emptied, folders);
    for difference in differences {
        let Some(node) = &difference.before else {
            continue;
        };
        let path = &difference.path;
        if same_content(node, difference.after.as_ref()) && set_mode(&root, path, node).is_ok() {
            done.restored.push(path.clone());
            continue;
        }
        match write(&root, path, node, objects) {
            Ok(()) => done.restored.push(path.clone()),
            Err(left) => done.left.push(left),
        }
    }
    for folder in missing {
        let parts: Vec<&str> = folder.split('/').collect();
        if let Err(error) = root.dir(&parts, true) {
            done.left.push(failed(folder, error));
        }
    }
    done.restored.sort();
    done.removed.sort();
    done
}

fn remove(root: &Root, path: &str) -> std::io::Result<()> {
    let (parents, name) = split(path);
    match root.dir(&parents, false) {
        Ok(dir) => dir.remove_file(name),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Removes the folders that a removal left empty and that the record did
/// not hold, up toward the bot's folder.
fn prune(root: &Root, emptied: BTreeSet<String>, kept: &BTreeSet<String>) {
    let mut emptied: Vec<String> = emptied.into_iter().collect();
    emptied.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    for mut path in emptied {
        loop {
            if kept.contains(&path) {
                break;
            }
            let (parents, name) = split(&path);
            let removed = root
                .dir(&parents, false)
                .and_then(|dir| dir.remove_dir(name));
            if removed.is_err() {
                break;
            }
            match path.rsplit_once('/') {
                Some((parent, _)) => path = parent.to_owned(),
                None => break,
            }
        }
    }
}

/// The permission bits to write a file with: those the record holds, never
/// set-user-id, set-group-id, or sticky. A record taken before the daemon
/// kept them gives those of the file it replaces, or `0o644`, with the
/// execute bits where it reads when the record says it is executable.
fn mode_for(recorded: Option<u32>, existing: Option<u32>, executable: bool) -> u32 {
    if let Some(mode) = recorded {
        return mode & 0o777;
    }
    let mode = existing.unwrap_or(0o644);
    if executable {
        mode | ((mode & 0o444) >> 2)
    } else {
        mode & !0o111
    }
}

/// Whether a file holds the content the record names, and differs in its
/// permission bits alone.
fn same_content(recorded: &Node, now: Option<&Node>) -> bool {
    matches!(
        (recorded, now),
        (
            Node::File { sha256, mode: Some(_), .. },
            Some(Node::File { sha256: now_sha256, .. }),
        ) if sha256 == now_sha256
    )
}

fn set_mode(root: &Root, path: &str, node: &Node) -> std::io::Result<()> {
    let Node::File {
        mode: Some(mode), ..
    } = node
    else {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    };
    let (parents, name) = split(path);
    root.dir(&parents, false)?.set_mode(name, *mode)
}

fn write(root: &Root, path: &str, node: &Node, objects: &Objects) -> Result<(), LeftFile> {
    let (parents, name) = split(path);
    match node {
        Node::File {
            sha256,
            executable,
            mode,
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
            let dir = root
                .dir(&parents, true)
                .map_err(|error| failed(path, error))?;
            let mode = mode_for(*mode, dir.mode(name), *executable);
            dir.write_file(name, &content, mode)
                .map_err(|error| failed(path, error))
        }
        Node::Link { target } => {
            let dir = root
                .dir(&parents, true)
                .map_err(|error| failed(path, error))?;
            dir.remove_file(name).map_err(|error| failed(path, error))?;
            dir.symlink(target, name)
                .map_err(|error| failed(path, error))
        }
        Node::Folder { .. } => Ok(()),
    }
}
