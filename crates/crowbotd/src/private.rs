//! The daemon's files are its user's alone: the database holds every
//! conversation, and the provider homes hold sign-ins. On Unix each folder the
//! daemon creates is 0700 and each file 0600, whatever the umask. A folder the
//! user made already keeps its mode: only the files inside are made private.
//!
//! The umask itself is left alone, because the agents inherit it and write the
//! user's own files in the bots' folders.

use std::fs::OpenOptions;
use std::path::Path;

/// Creates a folder and the missing folders above it. On Unix each folder it
/// creates is 0700.
pub fn create_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(folder)
}

/// Async form of `create_folder`.
pub async fn create_folder_async(folder: &Path) -> std::io::Result<()> {
    let folder = folder.to_owned();
    tokio::task::spawn_blocking(move || create_folder(&folder))
        .await
        .map_err(std::io::Error::other)?
}

/// Options that create a file only its user may read, on Unix.
pub fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// Writes a file that only its user may read, on Unix: a new file is created
/// 0600, and a file that exists is made so.
pub fn write(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    restrict(path)?;
    file.write_all(content.as_ref())
}

/// Makes a file that exists, and that belongs to this user, readable and
/// writable by its user only. A missing file is no error.
pub fn restrict(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if metadata.is_file() && metadata.uid() == user() && metadata.mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Prepares the folder that holds the database. A missing folder is created
/// 0700, and with `own_mode` (a folder of Crow Bot's own, such as the default
/// data folder) a folder that exists is made 0700 too. A folder of another
/// account is refused: that account could read the conversations.
pub fn database_folder(folder: &Path, own_mode: bool) -> Result<(), String> {
    create_folder(folder)
        .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = std::fs::metadata(folder)
            .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
        if metadata.uid() != user() {
            return Err(format!(
                "the folder {} belongs to {}, another account: Crow Bot keeps its files to the account that runs it",
                folder.display(),
                account_name(metadata.uid())
            ));
        }
        if own_mode && metadata.mode() & 0o077 != 0 {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("cannot make {} private: {error}", folder.display()))?;
            eprintln!("[crowbotd] made {} private to its user", folder.display());
        }
    }
    #[cfg(not(unix))]
    let _ = own_mode;
    Ok(())
}

#[cfg(unix)]
fn user() -> u32 {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() }
}

/// The name of an account, and its number.
#[cfg(unix)]
pub fn account_name(uid: u32) -> String {
    match account(uid) {
        Some(name) => format!("{name} (uid {uid})"),
        None => format!("uid {uid}"),
    }
}

/// The name of an account, when the system knows it.
#[cfg(unix)]
pub fn account(uid: u32) -> Option<String> {
    let mut buffer = vec![0_u8; 4096];
    // SAFETY: `passwd` is plain data, for which all zeroes is valid.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and the buffer outlives
    // the use of the strings it receives.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &raw mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_name.is_null() {
        return None;
    }
    // SAFETY: on success `pw_name` points to a NUL-terminated string inside
    // `buffer`, which is still alive.
    let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };
    Some(name.to_string_lossy().into_owned())
}
