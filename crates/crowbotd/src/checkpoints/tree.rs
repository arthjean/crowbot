//! A record of a folder is a tree of trees, one per folder, as git stores
//! them: each tree lists the entries of one folder by name and is stored by
//! the SHA-256 of its body. A folder that did not change between two records
//! keeps its hash, so the records share it, and a comparison skips it whole.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use crowbot_protocol::{FileKind, FileState};
use serde::{Deserialize, Serialize};

use crate::digest::sha256_hex;
use crate::store::{Store, StoreError};

/// What an entry of a folder is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    File {
        size: u64,
        sha256: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        executable: bool,
        kept: bool,
    },
    /// A symbolic link, with its target as the link holds it.
    Link { target: String },
    /// A folder, with the hash of its tree.
    Folder { tree: String },
}

impl Node {
    /// The state a client reads of a file or a link. `None` for a folder.
    pub fn state(&self) -> Option<FileState> {
        match self {
            Self::File {
                size,
                sha256,
                executable,
                kept,
            } => Some(FileState {
                kind: FileKind::File,
                size: *size,
                sha256: sha256.clone(),
                target: None,
                executable: *executable,
                kept: *kept,
            }),
            Self::Link { target } => Some(FileState {
                kind: FileKind::Link,
                size: target.len() as u64,
                sha256: sha256_hex(target.as_bytes()),
                target: Some(target.clone()),
                executable: false,
                kept: true,
            }),
            Self::Folder { .. } => None,
        }
    }

    /// Whether two entries hold the same thing. The size follows the hash,
    /// and whether a content was kept says nothing of the file.
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::File {
                    sha256, executable, ..
                },
                Self::File {
                    sha256: other_sha256,
                    executable: other_executable,
                    ..
                },
            ) => sha256 == other_sha256 && executable == other_executable,
            _ => self == other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(flatten)]
    pub node: Node,
}

/// The body of a tree and its hash. The entries are sorted by name, so one
/// folder always gives the same body.
pub fn encode(entries: &mut [Entry]) -> (String, String) {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let body = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_owned());
    (sha256_hex(body.as_bytes()), body)
}

/// Reads trees from the store, each once, and from a record not stored yet.
pub struct Trees<'a> {
    store: &'a Store,
    pending: HashMap<&'a str, &'a str>,
    read: HashMap<String, Rc<Vec<Entry>>>,
}

impl<'a> Trees<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            pending: HashMap::new(),
            read: HashMap::new(),
        }
    }

    /// Reads the trees of a record taken and not stored, before the store's.
    pub fn with_pending(store: &'a Store, pending: &'a [(String, String)]) -> Self {
        Self {
            pending: pending
                .iter()
                .map(|(hash, body)| (hash.as_str(), body.as_str()))
                .collect(),
            ..Self::new(store)
        }
    }

    pub fn get(&mut self, hash: &str) -> Result<Rc<Vec<Entry>>, StoreError> {
        if let Some(entries) = self.read.get(hash) {
            return Ok(entries.clone());
        }
        let body = match self.pending.get(hash) {
            Some(body) => Some((*body).to_owned()),
            None => self.store.checkpoint_tree(hash)?,
        };
        let body = body.ok_or_else(|| {
            StoreError::Refused(format!("the record of the folder lacks its tree {hash}"))
        })?;
        let entries: Rc<Vec<Entry>> = Rc::new(serde_json::from_str(&body)?);
        self.read.insert(hash.to_owned(), entries.clone());
        Ok(entries)
    }
}

/// Everything a record holds, by path relative to the folder.
#[derive(Debug, Default)]
pub struct Listing {
    /// The files and links.
    pub files: BTreeMap<String, Node>,
    pub folders: BTreeSet<String>,
}

pub fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Lists a record whole.
pub fn list(trees: &mut Trees<'_>, root: &str) -> Result<Listing, StoreError> {
    let mut listing = Listing::default();
    list_into(trees, root, "", &mut listing)?;
    Ok(listing)
}

fn list_into(
    trees: &mut Trees<'_>,
    hash: &str,
    prefix: &str,
    listing: &mut Listing,
) -> Result<(), StoreError> {
    for entry in trees.get(hash)?.iter() {
        let path = join(prefix, &entry.name);
        match &entry.node {
            Node::Folder { tree } => {
                list_into(trees, tree, &path, listing)?;
                listing.folders.insert(path);
            }
            node => {
                listing.files.insert(path, node.clone());
            }
        }
    }
    Ok(())
}

/// One path whose file or link differs between two records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub before: Option<Node>,
    pub after: Option<Node>,
}

/// The files and links that differ between two records, by path. A folder
/// whose tree is the same in both is not read.
pub fn diff(
    trees: &mut Trees<'_>,
    before: &str,
    after: &str,
) -> Result<Vec<Difference>, StoreError> {
    let mut differences = Vec::new();
    diff_into(trees, Some(before), Some(after), "", &mut differences)?;
    Ok(differences)
}

/// The entry when it is a file or a link.
fn file(node: Option<&Node>) -> Option<&Node> {
    node.filter(|node| !matches!(node, Node::Folder { .. }))
}

fn diff_into(
    trees: &mut Trees<'_>,
    before: Option<&str>,
    after: Option<&str>,
    prefix: &str,
    out: &mut Vec<Difference>,
) -> Result<(), StoreError> {
    if before == after {
        return Ok(());
    }
    let read = |trees: &mut Trees<'_>, hash: Option<&str>| -> Result<_, StoreError> {
        Ok(match hash {
            Some(hash) => trees
                .get(hash)?
                .iter()
                .map(|entry| (entry.name.clone(), entry.node.clone()))
                .collect::<BTreeMap<_, _>>(),
            None => BTreeMap::new(),
        })
    };
    let old = read(trees, before)?;
    let new = read(trees, after)?;
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for name in names {
        let path = join(prefix, name);
        let (old, new) = (old.get(name), new.get(name));
        let folder = |node: Option<&Node>| match node {
            Some(Node::Folder { tree }) => Some(tree.clone()),
            _ => None,
        };
        // A folder on either side is compared as a folder; a file that took
        // its place, or that it replaced, is a difference of its own.
        let (old_folder, new_folder) = (folder(old), folder(new));
        if old_folder.is_some() || new_folder.is_some() {
            diff_into(
                trees,
                old_folder.as_deref(),
                new_folder.as_deref(),
                &path,
                out,
            )?;
        }
        let (old_file, new_file) = (file(old), file(new));
        let differs = match (old_file, new_file) {
            (Some(old), Some(new)) => !old.same(new),
            (None, None) => false,
            _ => true,
        };
        if differs {
            out.push(Difference {
                path,
                before: old_file.cloned(),
                after: new_file.cloned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, sha256: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            node: Node::File {
                size: 1,
                sha256: sha256.to_owned(),
                executable: false,
                kept: true,
            },
        }
    }

    #[test]
    fn one_folder_gives_one_body_whatever_the_order_of_its_entries() {
        let (first, _) = encode(&mut [file("b", "2"), file("a", "1")]);
        let (second, body) = encode(&mut [file("a", "1"), file("b", "2")]);
        assert_eq!(first, second);
        assert_eq!(
            body,
            r#"[{"name":"a","kind":"file","size":1,"sha256":"1","kept":true},{"name":"b","kind":"file","size":1,"sha256":"2","kept":true}]"#
        );
    }

    #[test]
    fn a_link_reads_as_the_hash_of_its_target() {
        let state = Node::Link {
            target: "abc".to_owned(),
        }
        .state()
        .unwrap();
        assert_eq!(state.kind, FileKind::Link);
        assert_eq!(state.size, 3);
        assert_eq!(
            state.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_kept_content_is_not_a_change() {
        let kept = file("a", "1").node;
        let Node::File { size, sha256, .. } = kept.clone() else {
            unreachable!()
        };
        let hash_alone = Node::File {
            size,
            sha256,
            executable: false,
            kept: false,
        };
        assert!(kept.same(&hash_alone));
    }
}
