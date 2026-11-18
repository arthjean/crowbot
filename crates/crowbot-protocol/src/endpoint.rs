use std::fmt;

/// The longest path of a Unix socket, in bytes: the kernel's `sun_path` holds
/// it and a closing NUL, in 108 bytes on Linux and 104 on the BSDs and macOS
/// (unix(7)).
pub const SOCKET_PATH_LIMIT: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
    107
} else {
    103
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(String);

impl Endpoint {
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    #[cfg(windows)]
    pub fn local() -> Option<Self> {
        let user = std::env::var("USERNAME").ok()?;
        let user: String = user
            .chars()
            .map(|character| {
                if character.is_alphanumeric() || character == '-' || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        Some(Self(format!(r"\\.\pipe\crowbotd-{user}")))
    }

    /// The socket of this user's daemon, in the runtime folder (`folders.rs`).
    #[cfg(not(windows))]
    pub fn local() -> Option<Self> {
        Some(Self(
            crate::Folders::local()?
                .socket()
                .to_string_lossy()
                .into_owned(),
        ))
    }

    /// The length in bytes of a socket path that is too long to listen on,
    /// over `SOCKET_PATH_LIMIT`. A named pipe has no such limit.
    pub fn too_long(&self) -> Option<usize> {
        let length = self.0.len();
        (cfg!(unix) && length > SOCKET_PATH_LIMIT).then_some(length)
    }

    pub fn address(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
