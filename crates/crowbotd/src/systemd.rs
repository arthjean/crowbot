//! What the daemon tells systemd, and what it reads from it, on Linux. The
//! readiness message is the protocol of sd_notify(3), written here: one
//! datagram to the socket that `NOTIFY_SOCKET` names. No library is linked.
//!
//! Under the user unit (`Type=notify`) the daemon sends `READY=1` once it
//! holds its database, ended the turns a previous daemon left running, and
//! claimed its endpoint, and `STOPPING=1` when a signal stops it.

use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::OnceLock;

/// The socket of the service manager, read once at the start and removed
/// from the environment, so that no agent the daemon starts inherits it.
static NOTIFY_SOCKET: OnceLock<Option<OsString>> = OnceLock::new();

/// Takes `NOTIFY_SOCKET` out of the environment. Call it before any thread
/// starts: the environment of a process with several threads cannot be
/// changed safely.
pub fn take_notify_socket() {
    let socket = std::env::var_os("NOTIFY_SOCKET");
    if socket.is_some() {
        // SAFETY: called at the top of `main`, before the runtime or any
        // other thread starts, so nothing reads the environment meanwhile.
        unsafe { std::env::remove_var("NOTIFY_SOCKET") };
    }
    NOTIFY_SOCKET.set(socket).ok();
}

/// Tells the service manager that the daemon is ready to serve.
pub fn ready() {
    notify("READY=1");
}

/// Tells the service manager that the daemon is stopping.
pub fn stopping() {
    notify("STOPPING=1");
}

fn notify(state: &str) {
    let Some(Some(socket)) = NOTIFY_SOCKET.get() else {
        return;
    };
    if let Err(error) = send(socket, state) {
        eprintln!("[crowbotd] cannot tell the service manager {state}: {error}");
    }
}

#[cfg(target_os = "linux")]
fn send(socket: &std::ffi::OsStr, state: &str) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let bytes = socket.as_bytes();
    // A name that starts with `@` is in the abstract namespace.
    let address = match bytes.split_first() {
        Some((b'@', name)) => SocketAddr::from_abstract_name(name)?,
        Some((b'/', _)) => SocketAddr::from_pathname(Path::new(socket))?,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NOTIFY_SOCKET is neither a path nor an abstract name",
            ));
        }
    };
    let sent = UnixDatagram::unbound()?.send_to_addr(state.as_bytes(), &address)?;
    if sent == state.len() {
        Ok(())
    } else {
        Err(std::io::ErrorKind::WriteZero.into())
    }
}

#[cfg(not(target_os = "linux"))]
fn send(_socket: &std::ffi::OsStr, _state: &str) -> std::io::Result<()> {
    Ok(())
}

/// Whether systemd keeps this user's services running with no session
/// (`loginctl enable-linger`). `None` on a computer that systemd did not
/// start, or off Linux.
#[cfg(target_os = "linux")]
pub fn linger() -> Option<bool> {
    // sd_booted(3): systemd started the computer when this folder exists.
    if !Path::new("/run/systemd/system").is_dir() {
        return None;
    }
    // SAFETY: `geteuid` has no preconditions.
    let name = crate::private::account(unsafe { libc::geteuid() })?;
    // logind keeps one file per user who lingers (loginctl(1)).
    Some(Path::new("/var/lib/systemd/linger").join(name).exists())
}

#[cfg(not(target_os = "linux"))]
pub fn linger() -> Option<bool> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::net::UnixDatagram;

    use super::*;

    #[test]
    fn the_readiness_message_reaches_a_socket_path_and_an_abstract_name() {
        let dir = crowbot_protocol::scratch::Scratch::new("crowbotd-notify");
        let path = dir.join("notify");
        let listener = UnixDatagram::bind(&path).unwrap();
        send(path.as_os_str(), "READY=1").unwrap();
        let mut buffer = [0_u8; 64];
        let length = listener.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"READY=1");

        use std::os::linux::net::SocketAddrExt;
        let name = format!("crowbotd-notify-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let listener = UnixDatagram::bind_addr(&address).unwrap();
        send(std::ffi::OsStr::new(&format!("@{name}")), "STOPPING=1").unwrap();
        let length = listener.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"STOPPING=1");
    }
}
