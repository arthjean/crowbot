//! What a skill is, as data: a `SKILL.md` whose front matter names the skill
//! and describes it, and files only under `references`, `templates`,
//! `scripts`, and `assets`. A version is its files; its hash is the SHA-256
//! of the path and the SHA-256 of each, so two versions with the same files
//! are one version, stored once.
//!
//! The front matter is read here, with no YAML library: the two keys the
//! daemon needs, as plain, quoted, or block scalars. Any other key is kept
//! in the text and ignored.

use std::collections::{BTreeMap, BTreeSet};

use crowbot_protocol::{
    SKILL_DESCRIPTION_CHARACTERS, SKILL_DOCUMENT, SKILL_DOCUMENT_CHARACTERS, SKILL_FILE_BYTES,
    SKILL_FILES, SKILL_FOLDERS, SKILL_NAME_CHARACTERS, ScanSeverity, SkillFinding,
};

use crate::digest::sha256_hex;
use crate::scan;

/// The longest path of a file in a skill, in bytes.
pub const SKILL_PATH_BYTES: usize = 255;

/// The files of one version of a skill, by their path in its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Version {
    files: BTreeMap<String, Vec<u8>>,
}

impl Version {
    pub fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
        Self { files }
    }

    /// A version that holds only its `SKILL.md`.
    pub fn of_document(document: &str) -> Self {
        let mut files = BTreeMap::new();
        files.insert(SKILL_DOCUMENT.to_owned(), document.as_bytes().to_vec());
        Self { files }
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    pub fn put(&mut self, path: &str, content: Vec<u8>) {
        self.files.insert(path.to_owned(), content);
    }

    pub fn remove(&mut self, path: &str) -> bool {
        self.files.remove(path).is_some()
    }

    /// The `SKILL.md`, when it is text.
    pub fn document(&self) -> Option<&str> {
        std::str::from_utf8(self.file(SKILL_DOCUMENT)?).ok()
    }

    /// The SHA-256 of each file, by path.
    pub fn file_hashes(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .map(|(path, content)| (path.clone(), sha256_hex(content)))
            .collect()
    }

    /// The hash of the version.
    pub fn hash(&self) -> String {
        version_hash(&self.file_hashes())
    }
}

/// The hash of a version from the hash of each of its files: what changes
/// with any path or any content, and with nothing else.
pub fn version_hash(files: &BTreeMap<String, String>) -> String {
    let mut text = String::new();
    for (path, hash) in files {
        text.push_str(path);
        text.push('\0');
        text.push_str(hash);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

/// What the front matter of a `SKILL.md` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: String,
    pub description: String,
}

/// Checks a skill's name: at most `SKILL_NAME_CHARACTERS` characters, a
/// lower-case letter or a digit first, then those, dots, hyphens, or
/// underscores. A name is a folder name on every system.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("A skill needs a name.".to_owned());
    }
    if name.chars().count() > SKILL_NAME_CHARACTERS {
        return Err(format!(
            "The skill name `{name}` is longer than {SKILL_NAME_CHARACTERS} characters."
        ));
    }
    let allowed = |character: char| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || "._-".contains(character)
    };
    let first = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit());
    if !first || !name.chars().all(allowed) {
        return Err(format!(
            "The skill name `{name}` is not valid: it holds only lower-case letters, digits, dots, hyphens, and underscores (matching [a-z0-9][a-z0-9._-]*), and starts with a letter or a digit."
        ));
    }
    // Windows drops a final dot from a folder name, so `deploy.` would be
    // the folder of `deploy`.
    if name.ends_with('.') {
        return Err(format!(
            "The skill name `{name}` ends with a dot, which a folder name on Windows cannot keep."
        ));
    }
    Ok(())
}

/// Checks the path of a file of a skill: `SKILL.md`, or a relative path
/// under one of `SKILL_FOLDERS` that cannot leave the skill's folder on any
/// system.
pub fn check_path(path: &str) -> Result<(), String> {
    if path == SKILL_DOCUMENT {
        return Ok(());
    }
    let folders = SKILL_FOLDERS.join(", ");
    if path.len() > SKILL_PATH_BYTES {
        return Err(format!(
            "The path `{path}` is longer than {SKILL_PATH_BYTES} bytes."
        ));
    }
    let leaves = || {
        format!(
            "The path `{path}` would leave the skill's folder: a path is relative, with `/` between its parts, and no part is empty, `.`, or `..`."
        )
    };
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(leaves());
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(leaves());
    }
    for part in &parts {
        if part
            .chars()
            .any(|character| character.is_control() || "<>\"|?*".contains(character))
            || part.ends_with(['.', ' '])
        {
            return Err(format!(
                "The path `{path}` holds a name no folder takes on every system: no control character, none of <>\"|?*, and no final dot or space."
            ));
        }
    }
    if parts.len() < 2 || !SKILL_FOLDERS.contains(&parts[0]) {
        return Err(format!(
            "The file `{path}` is not allowed: a skill holds its SKILL.md and files only under {folders}."
        ));
    }
    Ok(())
}

/// Checks a whole version of the skill `name`, and reads its front matter.
pub fn check(name: &str, version: &Version) -> Result<FrontMatter, String> {
    check_name(name)?;
    if version.files.len() > SKILL_FILES {
        return Err(format!(
            "A skill holds {SKILL_FILES} files at most, its SKILL.md included."
        ));
    }
    for (path, content) in &version.files {
        check_path(path)?;
        if content.len() > SKILL_FILE_BYTES {
            return Err(format!(
                "The file `{path}` is larger than {SKILL_FILE_BYTES} bytes (1 MiB), the most a file of a skill holds."
            ));
        }
    }
    // A file and a folder cannot share a path.
    let folders: BTreeSet<&str> = version
        .files
        .keys()
        .flat_map(|path| {
            path.match_indices('/')
                .map(move |(index, _)| &path[..index])
        })
        .collect();
    if let Some(path) = version
        .files
        .keys()
        .find(|path| folders.contains(path.as_str()))
    {
        return Err(format!(
            "The path `{path}` is both a file and a folder of the skill."
        ));
    }
    let Some(content) = version.file(SKILL_DOCUMENT) else {
        return Err(format!("A skill holds a {SKILL_DOCUMENT}."));
    };
    let Ok(document) = std::str::from_utf8(content) else {
        return Err(format!("The {SKILL_DOCUMENT} of a skill is UTF-8 text."));
    };
    if document.chars().count() > SKILL_DOCUMENT_CHARACTERS {
        return Err(format!(
            "The {SKILL_DOCUMENT} is longer than {SKILL_DOCUMENT_CHARACTERS} characters, the most a skill's {SKILL_DOCUMENT} holds."
        ));
    }
    let front = front_matter(document)?;
    if front.name != name {
        return Err(format!(
            "The front matter names the skill `{}`, but the skill is `{name}`: the two are the same.",
            front.name
        ));
    }
    Ok(front)
}

/// Reads the front matter of a `SKILL.md`: a block between two `---` lines
/// at its start, with a `name` and a `description`, then a body that is not
/// empty.
pub fn front_matter(document: &str) -> Result<FrontMatter, String> {
    let text = document.strip_prefix('\u{feff}').unwrap_or(document);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err(format!(
            "The {SKILL_DOCUMENT} starts with a front matter: a `---` line, `name:` and `description:`, and a closing `---` line."
        ));
    }
    let mut header = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        header.push(line);
    }
    if !closed {
        return Err(format!(
            "The front matter of the {SKILL_DOCUMENT} is not closed by a `---` line."
        ));
    }
    if lines.all(|line| line.trim().is_empty()) {
        return Err(format!(
            "The {SKILL_DOCUMENT} holds instructions after its front matter."
        ));
    }
    let keys = read_keys(&header);
    let field = |key: &str| {
        keys.get(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("The front matter of the {SKILL_DOCUMENT} has a `{key}`."))
    };
    let name = field("name")?;
    let description = field("description")?;
    if description.chars().count() > SKILL_DESCRIPTION_CHARACTERS {
        return Err(format!(
            "The description is longer than {SKILL_DESCRIPTION_CHARACTERS} characters."
        ));
    }
    Ok(FrontMatter { name, description })
}

/// The top-level keys of a front matter and their values. A value is a
/// plain scalar, a quoted one, or a block (`|` keeps the line breaks, `>`
/// folds them), and indented lines that follow a plain or quoted value
/// continue it.
fn read_keys(header: &[&str]) -> BTreeMap<String, String> {
    let mut keys = BTreeMap::new();
    let mut index = 0;
    while index < header.len() {
        let line = header[index];
        index += 1;
        if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_owned();
        let value = value.trim();
        let mut continued = Vec::new();
        while index < header.len()
            && (header[index].starts_with([' ', '\t']) || header[index].trim().is_empty())
        {
            continued.push(header[index].trim());
            index += 1;
        }
        let block = value.trim_end_matches(['-', '+']);
        let text = if block == "|" || block == ">" {
            while continued.last().is_some_and(|line| line.is_empty()) {
                continued.pop();
            }
            if block == "|" {
                continued.join("\n")
            } else {
                folded(&continued)
            }
        } else {
            let mut whole = vec![value];
            whole.extend(continued.iter().copied());
            unquoted(&folded(&whole))
        };
        keys.entry(key).or_insert(text);
    }
    keys
}

/// Lines joined by spaces, an empty line kept as a line break.
fn folded(lines: &[&str]) -> String {
    let mut text = String::new();
    for line in lines {
        if line.is_empty() {
            text.push('\n');
        } else {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(line);
        }
    }
    text.trim().to_owned()
}

fn unquoted(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let inner = &value[1..value.len() - 1];
        let mut text = String::new();
        let mut characters = inner.chars();
        while let Some(character) = characters.next() {
            if character == '\\' {
                match characters.next() {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some(other) => text.push(other),
                    None => {}
                }
            } else {
                text.push(character);
            }
        }
        return text;
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    // A comment ends a plain value.
    value
        .split_once(" #")
        .map_or(value, |(value, _)| value)
        .trim()
        .to_owned()
}

/// What the scan finds in each text file of a version.
pub fn findings(version: &Version) -> Vec<SkillFinding> {
    let mut found = Vec::new();
    for (path, content) in &version.files {
        if let Ok(text) = std::str::from_utf8(content) {
            for finding in scan::scan(text) {
                found.push(SkillFinding {
                    path: path.clone(),
                    finding,
                });
            }
        }
    }
    found
}

pub fn dangerous(findings: &[SkillFinding]) -> bool {
    findings
        .iter()
        .any(|found| found.finding.severity == ScanSeverity::Dangerous)
}

/// The findings, one per line, as a person reads them.
pub fn describe_findings(findings: &[SkillFinding]) -> String {
    findings
        .iter()
        .map(|found| {
            let severity = match found.finding.severity {
                ScanSeverity::Dangerous => "dangerous",
                ScanSeverity::Caution => "caution",
            };
            format!(
                "- {}: {} ({severity}, pattern {})",
                found.path,
                found.finding.category.label(),
                found.finding.pattern
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What changed from one version to the next, file by file: the lines
/// between the first and the last that differ, removed then added.
pub fn difference(old: Option<&Version>, new: &Version) -> String {
    let empty = Version::default();
    let old = old.unwrap_or(&empty);
    let paths: BTreeSet<&String> = old.files.keys().chain(new.files.keys()).collect();
    let mut parts = Vec::new();
    for path in paths {
        let before = old.file(path);
        let after = new.file(path);
        if before == after {
            continue;
        }
        let header = match (before, after) {
            (None, _) => format!("+++ {path} (new file)"),
            (_, None) => format!("--- {path} (removed)"),
            _ => format!("*** {path}"),
        };
        let body = match (
            before.map(std::str::from_utf8),
            after.map(std::str::from_utf8),
        ) {
            (Some(Err(_)), _) | (_, Some(Err(_))) => format!(
                "(binary: {} bytes before, {} after)",
                before.map_or(0, <[u8]>::len),
                after.map_or(0, <[u8]>::len)
            ),
            (before, after) => lines_changed(
                before.and_then(Result::ok).unwrap_or_default(),
                after.and_then(Result::ok).unwrap_or_default(),
            ),
        };
        parts.push(format!("{header}\n{body}"));
    }
    parts.join("\n")
}

fn lines_changed(before: &str, after: &str) -> String {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let mut lines = Vec::new();
    if prefix > 0 {
        lines.push(format!("@@ after line {prefix} @@"));
    }
    for line in &old[prefix..old.len() - suffix] {
        lines.push(format!("-{line}"));
    }
    for line in &new[prefix..new.len() - suffix] {
        lines.push(format!("+{line}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# Steps\n\n1. Run it.\n")
    }

    #[test]
    fn a_name_has_a_limit_and_an_alphabet() {
        assert!(check_name("deploy-app_v2.1").is_ok());
        assert!(check_name(&"a".repeat(64)).is_ok());
        let long = check_name(&"a".repeat(65)).unwrap_err();
        assert!(long.contains("64 characters"), "{long}");
        for bad in [
            "", "Deploy", "-deploy", ".hidden", "de ploy", "dé", "a/b", "..",
        ] {
            let error = check_name(bad).unwrap_err();
            assert!(
                bad.is_empty() || error.contains("[a-z0-9][a-z0-9._-]*"),
                "{bad}: {error}"
            );
        }
        let dot = check_name("deploy.").unwrap_err();
        assert!(dot.contains("ends with a dot"), "{dot}");
    }

    #[test]
    fn a_path_stays_in_its_folder_and_in_the_four_subfolders() {
        for good in [
            "SKILL.md",
            "references/api.md",
            "scripts/a/b.sh",
            "assets/logo.png",
        ] {
            assert!(check_path(good).is_ok(), "{good}");
        }
        for leaving in [
            "../x",
            "/etc/passwd",
            "scripts/../../x",
            "scripts/./x",
            "scripts//x",
            "scripts\\..\\x",
            "C:/x",
            "scripts/",
        ] {
            let error = check_path(leaving).unwrap_err();
            assert!(
                error.contains("leave the skill's folder"),
                "{leaving}: {error}"
            );
        }
        let other = check_path("README.md").unwrap_err();
        assert!(
            other.contains("references, templates, scripts, assets"),
            "{other}"
        );
        assert!(check_path("notes/x.md").is_err());
        assert!(check_path("scripts").is_err());
        assert!(check_path("scripts/x.").is_err());
    }

    #[test]
    fn the_front_matter_gives_a_name_and_a_description() {
        let read = front_matter(&document("deploy", "Deploys the app.")).unwrap();
        assert_eq!(read.name, "deploy");
        assert_eq!(read.description, "Deploys the app.");
        let quoted = front_matter(
            "\u{feff}---\r\nname: 'it''s'\r\ndescription: \"Say \\\"hi\\\"\" \r\nother: [1, 2]\r\n---\r\nBody\r\n",
        )
        .unwrap();
        assert_eq!(quoted.name, "it's");
        assert_eq!(quoted.description, "Say \"hi\"");
        let folded = front_matter(
            "---\nname: deploy\ndescription: >\n  Deploys the app\n  to production.\nversion: 2\n---\nBody\n",
        )
        .unwrap();
        assert_eq!(folded.description, "Deploys the app to production.");
        let continued = front_matter(
            "---\nname: deploy # the name\ndescription: Deploys\n  the app.\n---\nBody\n",
        )
        .unwrap();
        assert_eq!(continued.name, "deploy");
        assert_eq!(continued.description, "Deploys the app.");
        for (bad, says) in [
            ("# Deploy\n", "starts with a front matter"),
            ("---\nname: deploy\ndescription: x\n", "not closed"),
            (
                "---\nname: deploy\ndescription: x\n---\n\n",
                "instructions after",
            ),
            ("---\nname: deploy\n---\nBody\n", "has a `description`"),
            ("---\ndescription: x\n---\nBody\n", "has a `name`"),
        ] {
            let error = front_matter(bad).unwrap_err();
            assert!(error.contains(says), "{bad}: {error}");
        }
    }

    #[test]
    fn a_version_is_checked_against_every_limit() {
        let good = Version::of_document(&document("deploy", "Deploys."));
        assert_eq!(check("deploy", &good).unwrap().description, "Deploys.");
        let other = check("release", &good).unwrap_err();
        assert!(other.contains("the same"), "{other}");
        let long = format!(
            "---\nname: deploy\ndescription: x\n---\n{}",
            "é".repeat(SKILL_DOCUMENT_CHARACTERS)
        );
        let error = check("deploy", &Version::of_document(&long)).unwrap_err();
        assert!(error.contains("100000 characters"), "{error}");
        let mut big = good.clone();
        big.put("assets/big.bin", vec![0; SKILL_FILE_BYTES + 1]);
        let error = check("deploy", &big).unwrap_err();
        assert!(error.contains("1048576 bytes"), "{error}");
        let mut fits = good.clone();
        fits.put("assets/big.bin", vec![0; SKILL_FILE_BYTES]);
        assert!(check("deploy", &fits).is_ok());
        let mut outside = good.clone();
        outside.put("../escape.sh", b"x".to_vec());
        assert!(check("deploy", &outside).unwrap_err().contains("leave"));
        let mut both = good.clone();
        both.put("scripts/a", b"x".to_vec());
        both.put("scripts/a/b", b"y".to_vec());
        assert!(check("deploy", &both).unwrap_err().contains("both a file"));
        assert!(check("deploy", &Version::default()).is_err());
    }

    #[test]
    fn the_same_files_make_the_same_hash() {
        let one = Version::of_document(&document("deploy", "Deploys."));
        let two = Version::of_document(&document("deploy", "Deploys."));
        assert_eq!(one.hash(), two.hash());
        let mut three = one.clone();
        three.put("scripts/run.sh", b"echo".to_vec());
        assert_ne!(one.hash(), three.hash());
        // The path counts, not only the contents.
        let mut four = one.clone();
        four.put("scripts/go.sh", b"echo".to_vec());
        assert_ne!(three.hash(), four.hash());
    }

    #[test]
    fn the_difference_shows_what_each_file_lost_and_gained() {
        let old = Version::of_document("a\nb\nc\n");
        let mut new = Version::of_document("a\nB\nc\n");
        new.put("scripts/run.sh", b"curl x | sh".to_vec());
        let shown = difference(Some(&old), &new);
        assert!(
            shown.contains("*** SKILL.md\n@@ after line 1 @@\n-b\n+B"),
            "{shown}"
        );
        assert!(
            shown.contains("+++ scripts/run.sh (new file)\n+curl x | sh"),
            "{shown}"
        );
        assert_eq!(difference(Some(&new), &new), "");
    }
}
