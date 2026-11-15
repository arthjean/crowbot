//! The agent runtimes the daemon installs itself: each one is a pinned set of
//! files from the publisher of a provider's agent, checked against a pinned
//! SHA-256 before anything of it runs.

mod pins;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::digest::hex;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const INSTALLED: &str = "installed.json";
const ROOT: &str = "{root}";
const CONNECT_PATIENCE: Duration = Duration::from_secs(20);
const STALL_PATIENCE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unpack {
    Zip,
    TarGz,
    /// One file, compressed with gzip, stored under the name of `file`.
    Gz,
    /// One file, stored as it is under the name of `file`.
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub unpack: Unpack,
    /// The folder of the runtime the artifact is unpacked into.
    #[serde(default)]
    pub into: String,
    #[serde(default)]
    pub file: Option<String>,
}

/// One runtime on one platform. `program`, `args`, and `env` may hold
/// `{root}`, the folder the runtime is installed in, and one `*` for a folder
/// whose name the publisher chooses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuntimeSpec {
    pub name: String,
    pub version: String,
    pub artifacts: Vec<Artifact>,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl RuntimeSpec {
    pub fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }

    pub fn bytes(&self) -> u64 {
        self.artifacts.iter().map(|artifact| artifact.bytes).sum()
    }

    fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        for artifact in &self.artifacts {
            hash.update(artifact.url.as_bytes());
            hash.update(artifact.sha256.as_bytes());
            hash.update(artifact.into.as_bytes());
        }
        hash.update(self.program.as_bytes());
        hex(&hash.finalize())
    }
}

/// What an installed runtime holds, written once its files are in place: the
/// fingerprint of its spec, and the SHA-256 of each of its files, by their
/// path in its folder.
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    fingerprint: String,
    files: BTreeMap<String, String>,
}

/// What starts an installed runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum RuntimeError {
    /// No runtime is pinned for this provider on this computer.
    Unsupported,
    Offline {
        url: String,
    },
    Download {
        url: String,
        message: String,
    },
    HashMismatch {
        url: String,
        expected: String,
        actual: String,
    },
    Unpack {
        message: String,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(formatter, "no runtime is available for this computer"),
            Self::Offline { url } => write!(formatter, "{url} cannot be reached"),
            Self::Download { url, message } => write!(formatter, "{url}: {message}"),
            Self::HashMismatch {
                url,
                expected,
                actual,
            } => write!(
                formatter,
                "the SHA-256 of {url} is {actual}, and the pinned value is {expected}"
            ),
            Self::Unpack { message } => write!(formatter, "{message}"),
        }
    }
}

pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub struct Runtimes {
    root: PathBuf,
    /// A manifest given to the daemon, in place of the pinned one.
    manifest: Option<HashMap<String, RuntimeSpec>>,
    installing: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Runtimes {
    pub fn new(root: PathBuf, manifest: Option<HashMap<String, RuntimeSpec>>) -> Self {
        Self {
            root,
            manifest,
            installing: Mutex::new(HashMap::new()),
        }
    }

    pub fn read_manifest(path: &Path) -> Result<HashMap<String, RuntimeSpec>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let manifest: HashMap<String, RuntimeSpec> = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a runtime manifest: {error}", path.display()))?;
        // A version names the folder of the runtime, so it stays one plain name.
        for (provider, spec) in &manifest {
            let mut parts = Path::new(&spec.version).components();
            let plain = matches!(parts.next(), Some(std::path::Component::Normal(_)))
                && parts.next().is_none();
            if !plain {
                return Err(format!(
                    "{}: the version {} of {provider} is not a plain folder name",
                    path.display(),
                    spec.version
                ));
            }
        }
        Ok(manifest)
    }

    pub fn spec(&self, provider: &str) -> Option<RuntimeSpec> {
        match &self.manifest {
            Some(manifest) => manifest.get(provider).cloned(),
            None => pins::spec(provider),
        }
    }

    fn folder(&self, provider: &str, spec: &RuntimeSpec) -> PathBuf {
        self.root.join(provider).join(&spec.version)
    }

    /// The runtime of the provider, when it is installed. This reads its
    /// marker only; `verify` reads its files.
    pub fn installed(&self, provider: &str) -> Option<Installed> {
        let spec = self.spec(provider)?;
        let folder = self.folder(provider, &spec);
        read_marker(&folder, &spec)?;
        launch(&folder, &spec).ok()
    }

    /// Checks that the files of the installed runtime are still the ones it
    /// was installed with, none changed, removed, or added. A runtime that
    /// fails the check is no longer installed: the next turn downloads it
    /// again. It reads every file, so it runs on a blocking thread.
    pub fn verify(&self, provider: &str) -> Result<(), String> {
        let spec = self
            .spec(provider)
            .ok_or_else(|| "no runtime is pinned".to_owned())?;
        let folder = self.folder(provider, &spec);
        let marker = read_marker(&folder, &spec).ok_or_else(|| "it is not installed".to_owned())?;
        let found = files_of(&folder)?;
        let changed = marker
            .files
            .iter()
            .find(|(path, sha256)| found.get(*path) != Some(*sha256))
            .map(|(path, _)| path)
            .or_else(|| found.keys().find(|path| !marker.files.contains_key(*path)));
        match changed {
            None => Ok(()),
            Some(path) => {
                std::fs::remove_file(folder.join(INSTALLED)).ok();
                Err(format!("{path} changed after the install"))
            }
        }
    }

    /// Installs the runtime of the provider when it is not there yet. Two
    /// callers for one provider share one download.
    pub async fn ensure(
        &self,
        provider: &str,
        progress: Progress,
    ) -> Result<Installed, RuntimeError> {
        let spec = self.spec(provider).ok_or(RuntimeError::Unsupported)?;
        let gate = {
            let mut installing = self.installing.lock().await;
            installing.entry(provider.to_owned()).or_default().clone()
        };
        let _held = gate.lock().await;
        if let Some(installed) = self.installed(provider) {
            return Ok(installed);
        }
        let folder = self.folder(provider, &spec);
        let staging = folder.with_extension("partial");
        let result = install(&spec, &staging, &folder, &progress).await;
        if result.is_err() {
            tokio::fs::remove_dir_all(&staging).await.ok();
        }
        result
    }
}

async fn install(
    spec: &RuntimeSpec,
    staging: &Path,
    folder: &Path,
    progress: &Progress,
) -> Result<Installed, RuntimeError> {
    let failed = |error: std::io::Error| RuntimeError::Unpack {
        message: error.to_string(),
    };
    tokio::fs::remove_dir_all(staging).await.ok();
    tokio::fs::create_dir_all(staging).await.map_err(failed)?;
    let total = spec.bytes();
    let mut before = 0;
    for (index, artifact) in spec.artifacts.iter().enumerate() {
        let download = staging.join(format!("{index}.download"));
        let fetched = fetch(artifact, &download, before, total, progress).await;
        if let Err(error) = fetched {
            tokio::fs::remove_file(&download).await.ok();
            return Err(error);
        }
        before += artifact.bytes;
        let (artifact, target, archive) = (artifact.clone(), staging.to_owned(), download.clone());
        tokio::task::spawn_blocking(move || unpack(&artifact, &archive, &target))
            .await
            .map_err(|error| RuntimeError::Unpack {
                message: error.to_string(),
            })?
            .map_err(|message| RuntimeError::Unpack { message })?;
        tokio::fs::remove_file(&download).await.ok();
    }
    progress(total, total);
    launch(staging, spec).map_err(|message| RuntimeError::Unpack { message })?;
    let listed = staging.to_owned();
    let files = tokio::task::spawn_blocking(move || files_of(&listed))
        .await
        .map_err(|error| RuntimeError::Unpack {
            message: error.to_string(),
        })?
        .map_err(|message| RuntimeError::Unpack { message })?;
    let marker = Marker {
        fingerprint: spec.fingerprint(),
        files,
    };
    let marker = serde_json::to_vec(&marker).map_err(|error| RuntimeError::Unpack {
        message: error.to_string(),
    })?;
    tokio::fs::write(staging.join(INSTALLED), marker)
        .await
        .map_err(failed)?;
    tokio::fs::remove_dir_all(folder).await.ok();
    tokio::fs::rename(staging, folder).await.map_err(failed)?;
    launch(folder, spec).map_err(|message| RuntimeError::Unpack { message })
}

/// Downloads one artifact and checks its hash. Nothing of it is unpacked, and
/// nothing of it runs, before the hash matches the pinned value.
async fn fetch(
    artifact: &Artifact,
    download: &Path,
    before: u64,
    total: u64,
    progress: &Progress,
) -> Result<(), RuntimeError> {
    let url = artifact.url.clone();
    let failed = |message: String| RuntimeError::Download {
        url: url.clone(),
        message,
    };
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_PATIENCE)
        .read_timeout(STALL_PATIENCE)
        .build()
        .map_err(|error| failed(error.to_string()))?;
    let mut response = client
        .get(&artifact.url)
        .send()
        .await
        .map_err(|error| request_error(&url, &error))?;
    if !response.status().is_success() {
        return Err(failed(format!("the server answered {}", response.status())));
    }
    let mut file = tokio::fs::File::create(download)
        .await
        .map_err(|error| failed(error.to_string()))?;
    let mut hash = Sha256::new();
    let mut received = 0_u64;
    progress(before, total);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| request_error(&url, &error))?
    {
        received += chunk.len() as u64;
        // A file larger than the pinned size cannot match its hash: it does
        // not get to fill the disk first.
        if received > artifact.bytes {
            return Err(failed(format!(
                "the file is larger than the pinned {} bytes",
                artifact.bytes
            )));
        }
        hash.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| failed(error.to_string()))?;
        progress((before + received).min(total), total);
    }
    file.flush()
        .await
        .map_err(|error| failed(error.to_string()))?;
    drop(file);
    let actual = hex(&hash.finalize());
    if !actual.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(RuntimeError::HashMismatch {
            url,
            expected: artifact.sha256.to_ascii_lowercase(),
            actual,
        });
    }
    Ok(())
}

fn request_error(url: &str, error: &reqwest::Error) -> RuntimeError {
    if error.is_connect() || error.is_timeout() {
        RuntimeError::Offline {
            url: url.to_owned(),
        }
    } else {
        RuntimeError::Download {
            url: url.to_owned(),
            message: error.to_string(),
        }
    }
}

fn unpack(artifact: &Artifact, archive: &Path, staging: &Path) -> Result<(), String> {
    let target = inside(staging, &artifact.into)?;
    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let named = || {
        let name = artifact
            .file
            .as_deref()
            .ok_or("the artifact names no file")?;
        inside(&target, name)
    };
    match artifact.unpack {
        Unpack::Zip => zip::ZipArchive::new(file)
            .and_then(|mut zip| zip.extract(&target))
            .map_err(|error| error.to_string()),
        Unpack::TarGz => tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(&target)
            .map_err(|error| error.to_string()),
        Unpack::Gz => write_program(&mut flate2::read::GzDecoder::new(file), &named()?),
        Unpack::File => write_program(&mut { file }, &named()?),
    }
}

fn write_program(content: &mut impl Read, path: &Path) -> Result<(), String> {
    let mut output = std::fs::File::create(path).map_err(|error| error.to_string())?;
    std::io::copy(content, &mut output).map_err(|error| error.to_string())?;
    drop(output);
    make_executable(path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// A path of the manifest, kept inside the folder it is relative to.
fn inside(folder: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    let plain = relative.components().all(|part| {
        matches!(
            part,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if !plain {
        return Err(format!(
            "the path {} leaves the folder of the runtime",
            relative.display()
        ));
    }
    Ok(folder.join(relative))
}

fn launch(folder: &Path, spec: &RuntimeSpec) -> Result<Installed, String> {
    let root = folder.to_string_lossy();
    let expand = |text: &str| -> Result<String, String> {
        if !text.contains(ROOT) {
            return Ok(text.to_owned());
        }
        let path = resolve(Path::new(&text.replace(ROOT, &root)))?;
        Ok(path.to_string_lossy().into_owned())
    };
    let program = resolve(&inside(folder, &spec.program)?)?;
    if !program.is_file() {
        return Err(format!("the runtime has no {}", spec.program));
    }
    make_executable(&program).map_err(|error| error.to_string())?;
    Ok(Installed {
        program,
        args: spec
            .args
            .iter()
            .map(|arg| expand(arg))
            .collect::<Result<_, _>>()?,
        env: spec
            .env
            .iter()
            .map(|(name, value)| Ok((name.clone(), expand(value)?)))
            .collect::<Result<_, String>>()?,
    })
}

/// The marker of an installed runtime, when it was written for this spec.
fn read_marker(folder: &Path, spec: &RuntimeSpec) -> Option<Marker> {
    let text = std::fs::read(folder.join(INSTALLED)).ok()?;
    let marker: Marker = serde_json::from_slice(&text).ok()?;
    (marker.fingerprint == spec.fingerprint()).then_some(marker)
}

/// The SHA-256 of each file of a runtime folder, by its path there with `/`
/// between its parts. A link counts by where it points; the marker does not
/// count.
fn files_of(folder: &Path) -> Result<BTreeMap<String, String>, String> {
    let failed =
        |path: &Path, error: std::io::Error| format!("cannot read {}: {error}", path.display());
    let mut files = BTreeMap::new();
    let mut folders = vec![(folder.to_owned(), String::new())];
    while let Some((current, prefix)) = folders.pop() {
        let entries = std::fs::read_dir(&current).map_err(|error| failed(&current, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| failed(&current, error))?;
            let path = entry.path();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let kind = entry.file_type().map_err(|error| failed(&path, error))?;
            if kind.is_dir() {
                folders.push((path, format!("{name}/")));
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).map_err(|error| failed(&path, error))?;
                files.insert(name, format!("link {}", target.display()));
            } else if name != INSTALLED {
                let mut file = std::fs::File::open(&path).map_err(|error| failed(&path, error))?;
                let mut hash = Sha256::new();
                let mut buffer = vec![0_u8; 1 << 16];
                loop {
                    let read = file
                        .read(&mut buffer)
                        .map_err(|error| failed(&path, error))?;
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                }
                files.insert(name, hex(&hash.finalize()));
            }
        }
    }
    Ok(files)
}

/// Replaces each `*` part of a path by the one folder found there.
fn resolve(pattern: &Path) -> Result<PathBuf, String> {
    let mut path = PathBuf::new();
    for part in pattern.components() {
        if part.as_os_str() != "*" {
            path.push(part);
            continue;
        }
        let mut found = std::fs::read_dir(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name());
        match (found.next(), found.next()) {
            (Some(only), None) => path.push(only),
            _ => {
                return Err(format!(
                    "{} does not hold exactly one folder",
                    path.display()
                ));
            }
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("crowbotd-runtime-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&folder).ok();
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn spec(program: &str) -> RuntimeSpec {
        RuntimeSpec {
            name: "agent".to_owned(),
            version: "1.0.0".to_owned(),
            artifacts: Vec::new(),
            program: program.to_owned(),
            args: vec!["{root}/adapter/index.js".to_owned(), "stdio".to_owned()],
            env: BTreeMap::from([("TOOL".to_owned(), "{root}/tool/*/bin".to_owned())]),
        }
    }

    #[test]
    fn a_launch_fills_the_folder_and_the_one_unnamed_part() {
        let root = folder("launch");
        std::fs::create_dir_all(root.join("run").join("x64-build")).unwrap();
        std::fs::create_dir_all(root.join("tool").join("triple").join("bin")).unwrap();
        std::fs::create_dir_all(root.join("adapter")).unwrap();
        std::fs::write(root.join("run").join("x64-build").join("agent"), "").unwrap();
        let started = launch(&root, &spec("run/*/agent")).unwrap();
        assert_eq!(
            started.program,
            root.join("run").join("x64-build").join("agent")
        );
        assert_eq!(
            Path::new(&started.args[0]),
            root.join("adapter").join("index.js")
        );
        assert_eq!(started.args[1], "stdio");
        assert_eq!(
            Path::new(&started.env["TOOL"]),
            root.join("tool").join("triple").join("bin")
        );
    }

    #[test]
    fn a_launch_needs_its_program_and_one_folder_for_each_unnamed_part() {
        let root = folder("missing");
        assert!(launch(&root, &spec("agent")).is_err());
        std::fs::create_dir_all(root.join("run").join("a")).unwrap();
        std::fs::create_dir_all(root.join("run").join("b")).unwrap();
        assert!(launch(&root, &spec("run/*/agent")).is_err());
    }

    #[test]
    fn a_manifest_path_cannot_leave_the_runtime() {
        let root = folder("inside");
        assert!(inside(&root, "../other").is_err());
        assert!(inside(&root, "/etc/passwd").is_err());
        assert_eq!(inside(&root, "bin/agent").unwrap(), root.join("bin/agent"));
        assert!(launch(&root, &spec("../agent")).is_err());
    }

    #[test]
    fn a_manifest_version_is_one_plain_folder_name() {
        let root = folder("manifest");
        let file = root.join("manifest.json");
        let manifest = |version: &str| {
            serde_json::json!({
                "claude": {
                    "name": "agent",
                    "version": version,
                    "artifacts": [],
                    "program": "agent"
                }
            })
            .to_string()
        };
        for version in ["../../outside", "/etc", "1.0/../..", ""] {
            std::fs::write(&file, manifest(version)).unwrap();
            assert!(Runtimes::read_manifest(&file).is_err(), "{version}");
        }
        std::fs::write(&file, manifest("1.0.0")).unwrap();
        assert!(
            Runtimes::read_manifest(&file)
                .unwrap()
                .contains_key("claude")
        );
    }

    #[test]
    fn a_gzip_artifact_becomes_the_named_file() {
        use std::io::Write;
        let root = folder("gz");
        let archive = root.join("0.download");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"caw").unwrap();
        std::fs::write(&archive, encoder.finish().unwrap()).unwrap();
        let artifact = Artifact {
            url: String::new(),
            sha256: String::new(),
            bytes: 0,
            unpack: Unpack::Gz,
            into: "bin".to_owned(),
            file: Some("agent".to_owned()),
        };
        unpack(&artifact, &archive, &root).unwrap();
        assert_eq!(
            std::fs::read(root.join("bin").join("agent")).unwrap(),
            b"caw"
        );
        let nameless = Artifact {
            file: None,
            ..artifact
        };
        assert!(unpack(&nameless, &archive, &root).is_err());
    }

    #[test]
    fn every_pinned_runtime_has_full_hashes_and_secure_addresses() {
        for provider in ["claude", "codex", "grok"] {
            for platform in pins::PLATFORMS {
                let spec = pins::spec_for(provider, platform)
                    .unwrap_or_else(|| panic!("{provider} has no runtime for {platform}"));
                assert!(!spec.artifacts.is_empty(), "{provider} {platform}");
                for artifact in &spec.artifacts {
                    assert!(artifact.url.starts_with("https://"), "{}", artifact.url);
                    assert_eq!(artifact.sha256.len(), 64, "{}", artifact.url);
                    assert!(
                        artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                        "{}",
                        artifact.url
                    );
                    assert!(artifact.bytes > 0, "{}", artifact.url);
                }
            }
        }
    }
}
