//! The signals that stop the daemon. On Unix, SIGTERM (a service stop, a
//! logout), SIGINT (Ctrl+C), and SIGHUP (the end of a terminal) each stop it
//! cleanly; a second one stops it at once. Windows has none of them: the
//! daemon ends with its job there.

use std::fmt;

/// A signal that stops the daemon.
#[derive(Clone, Copy)]
pub enum StopSignal {
    Terminate,
    Interrupt,
    HangUp,
}

impl StopSignal {
    /// The number of the signal, for the exit status `128 + n` of a daemon
    /// that a second signal stopped.
    pub fn number(self) -> i32 {
        #[cfg(unix)]
        {
            match self {
                Self::Terminate => libc::SIGTERM,
                Self::Interrupt => libc::SIGINT,
                Self::HangUp => libc::SIGHUP,
            }
        }
        #[cfg(not(unix))]
        {
            match self {
                Self::Terminate => 15,
                Self::Interrupt => 2,
                Self::HangUp => 1,
            }
        }
    }
}

impl fmt::Display for StopSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
            Self::HangUp => "SIGHUP",
        })
    }
}

#[cfg(unix)]
pub struct Signals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
    hang_up: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    /// From now on these signals no longer end the process on their own.
    pub fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
            hang_up: signal(SignalKind::hangup())?,
        })
    }

    pub async fn next(&mut self) -> StopSignal {
        tokio::select! {
            _ = self.terminate.recv() => StopSignal::Terminate,
            _ = self.interrupt.recv() => StopSignal::Interrupt,
            _ = self.hang_up.recv() => StopSignal::HangUp,
        }
    }
}

#[cfg(not(unix))]
pub struct Signals;

#[cfg(not(unix))]
impl Signals {
    #[allow(clippy::unnecessary_wraps)]
    pub fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    pub async fn next(&mut self) -> StopSignal {
        std::future::pending().await
    }
}
