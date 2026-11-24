//! What a record left out because a `.gitignore` of the folder matched it
//! when the record was taken, read back from the `.gitignore` files that
//! record holds. A file the folder holds now and a record lacks is one the
//! turn added only when that record did not leave it out: otherwise it may
//! have been there all along, unrecorded, and a restore must not remove it.
//!
//! The rules follow the walk of `snapshot.rs`: each folder's `.gitignore`
//! applies to what lies under it, the deepest match decides, and a folder
//! that is ignored hides all it holds. A `.gitignore` whose content the
//! record did not keep cannot be read back, so everything under its folder
//! counts as left out.

use std::collections::BTreeMap;
use std::path::Path;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::objects::Objects;
use super::tree::{Listing, Node};

pub struct Ignored {
    /// The rules of each folder that holds a `.gitignore`, by its path
    /// relative to the bot's folder (`""` for the folder itself); `None`
    /// when they cannot be read back.
    rules: BTreeMap<String, Option<Gitignore>>,
}

impl Ignored {
    /// The rules of the record that `listing` holds, for the folder `folder`.
    pub fn of(folder: &Path, listing: &Listing, objects: &Objects) -> Self {
        let mut rules = BTreeMap::new();
        for (path, node) in &listing.files {
            let parent = match path.rsplit_once('/') {
                Some((parent, ".gitignore")) => parent,
                None if path == ".gitignore" => "",
                _ => continue,
            };
            let content = match node {
                Node::File {
                    sha256, kept: true, ..
                } => objects.get(sha256),
                _ => None,
            };
            let matcher = content.and_then(|content| {
                let root = parent
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .fold(folder.to_owned(), |root, part| root.join(part));
                let mut builder = GitignoreBuilder::new(root);
                for line in String::from_utf8_lossy(&content).lines() {
                    // A line that is not a valid pattern is skipped, as the
                    // walk skips it.
                    builder.add_line(None, line).ok();
                }
                builder.build().ok()
            });
            rules.insert(parent.to_owned(), matcher);
        }
        Self { rules }
    }

    /// Whether the record left `path` out: a file, or a folder when `is_dir`.
    pub fn covers(&self, folder: &Path, path: &str, is_dir: bool) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let parts: Vec<&str> = path.split('/').collect();
        // Each folder above the file, then the file: an ignored folder hides
        // what it holds.
        (1..=parts.len()).any(|end| {
            let place = parts[..end]
                .iter()
                .fold(folder.to_owned(), |place, part| place.join(part));
            let is_dir = end < parts.len() || is_dir;
            // The rules of the folders above this entry, the deepest first.
            for depth in (0..end).rev() {
                let owner = parts[..depth].join("/");
                match self.rules.get(&owner) {
                    None => {}
                    Some(None) => return true,
                    Some(Some(rules)) => match rules.matched(&place, is_dir) {
                        Match::Ignore(_) => return true,
                        Match::Whitelist(_) => return false,
                        Match::None => {}
                    },
                }
            }
            false
        })
    }
}
