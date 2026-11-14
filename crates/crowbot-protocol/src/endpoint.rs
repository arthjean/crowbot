use std::fmt;

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

    #[cfg(not(windows))]
    pub fn local() -> Option<Self> {
        let home = std::env::home_dir()?;
        Some(Self(
            home.join(".crowbot")
                .join("crowbotd.sock")
                .to_string_lossy()
                .into_owned(),
        ))
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
