//! The page of a sign-in, on Linux. An agent that signs in opens its page with
//! `xdg-open` or the program `BROWSER` names. That program would start the
//! browser inside the process group of the sign-in, which the daemon kills
//! when the sign-in is cancelled or takes too long, and a daemon with no
//! display variable could not open it at all. So during a sign-in both names
//! lead to the daemon's own program, which opens nothing: it hands the address
//! to the daemon over an abstract socket of that sign-in (a socket with a
//! name and no file, so no path length limits it), the daemon passes it to the
//! clients, and the window that started the sign-in opens it in its own
//! session (`docs/provider-sign-in.md`).

use std::collections::BTreeMap;
use std::hash::{BuildHasher, RandomState};
use std::io::Write;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

/// The name the daemon's program answers to as the opener of a sign-in page.
pub const OPENER: &str = "xdg-open";
/// The variable that names the abstract socket of the running sign-in.
const SOCKET_VARIABLE: &str = "CROWBOT_SIGN_IN_PAGE";
/// How long an opener gets to send its page once connected.
const SEND_PATIENCE: Duration = Duration::from_secs(5);
/// The longest address the daemon takes from an opener.
const PAGE_BYTES: usize = 8 << 10;

/// Whether the daemon's program was started as the opener of a page.
pub fn started_as_opener() -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == OPENER)
}

/// Runs as `xdg-open <address>`: hands the address to the daemon. The exit
/// status follows `xdg-open`: 1 for a wrong argument, 4 when the daemon could
/// not be told, so that the agent says it could not open the page.
pub fn opener() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(page), None) = (args.next(), args.next()) else {
        eprintln!("usage: {OPENER} <address>");
        return ExitCode::from(1);
    };
    let Some(page) = web_page(&page) else {
        eprintln!("{OPENER}: Crow Bot opens only an http or https address during a sign-in");
        return ExitCode::from(1);
    };
    let Some(socket) = std::env::var_os(SOCKET_VARIABLE) else {
        eprintln!("{OPENER}: no sign-in of Crow Bot is running");
        return ExitCode::from(4);
    };
    let told = SocketAddr::from_abstract_name(socket.as_encoded_bytes())
        .and_then(|address| UnixStream::connect_addr(&address))
        .and_then(|mut stream| stream.write_all(format!("{page}\n").as_bytes()));
    match told {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{OPENER}: cannot pass the page to Crow Bot: {error}");
            ExitCode::from(4)
        }
    }
}

/// The address when it is one web page of a reasonable length on one line.
fn web_page(text: &str) -> Option<&str> {
    let text = text.trim();
    let web = text.starts_with("https://") || text.starts_with("http://");
    let plain = !text.chars().any(char::is_control);
    (web && plain && text.len() <= PAGE_BYTES).then_some(text)
}

/// What a running sign-in takes its page through: an abstract socket, and the
/// opener in a folder of that sign-in alone, so that a sign-in that ends
/// after the next one started (a cancel, then a new start) leaves the next
/// one's opener in place. Dropping it closes the socket and removes the
/// folder.
pub struct PageCatch {
    folder: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PageCatch {
    /// Puts the opener in a folder of its own under `sign_ins`, both made
    /// 0700, listens for the page of a sign-in, and calls `on_page` with each
    /// address an opener of this account passes. Returns the variables that
    /// lead the sign-in's `xdg-open` and `BROWSER` to the opener, for a
    /// sign-in whose own variables are `environment`.
    pub fn start(
        sign_ins: &Path,
        environment: &BTreeMap<String, String>,
        on_page: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<(Self, BTreeMap<String, String>)> {
        let name = socket_name();
        // The name is new to every sign-in, of this daemon or of another that
        // shares the data folder.
        let folder = sign_ins.join(&name);
        crate::private::create_folder(&folder)?;
        // Removes the folder again when a step below fails.
        let mut catch = Self {
            folder: folder.clone(),
            task: None,
        };
        let opener = folder.join(OPENER);
        // The program of the running daemon, even once a build replaced the
        // file it started from, which `current_exe` would then name as deleted.
        let daemon = PathBuf::from(format!("/proc/{}/exe", std::process::id()));
        std::os::unix::fs::symlink(daemon, &opener)?;
        let listener = UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes())?)?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(listener)?;
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                // Any account can reach an abstract socket: only this one is heard.
                if !same_user(&stream) {
                    continue;
                }
                let mut line = String::new();
                // At most one address: an opener cannot fill the memory.
                let limited = tokio::io::AsyncReadExt::take(stream, PAGE_BYTES as u64 + 1);
                let mut reader = tokio::io::BufReader::new(limited);
                let read = tokio::time::timeout(SEND_PATIENCE, reader.read_line(&mut line)).await;
                if matches!(read, Ok(Ok(_)))
                    && let Some(page) = web_page(&line)
                {
                    on_page(page.to_owned());
                }
            }
        });
        catch.task = Some(task);
        let path = environment
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
            .map(|(_, value)| std::ffi::OsString::from(value))
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(folder.clone()).chain(std::env::split_paths(&path)),
        )
        .map_err(std::io::Error::other)?;
        let variables = BTreeMap::from([
            ("PATH".to_owned(), path.to_string_lossy().into_owned()),
            ("BROWSER".to_owned(), opener.to_string_lossy().into_owned()),
            (SOCKET_VARIABLE.to_owned(), name),
        ]);
        Ok((catch, variables))
    }
}

impl Drop for PageCatch {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        std::fs::remove_dir_all(&self.folder).ok();
    }
}

/// A name for the socket of one sign-in that no other sign-in, of this daemon
/// or another, takes: the process, a count, and a random part.
fn socket_name() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let random = RandomState::new().hash_one((std::process::id(), count));
    format!(
        "crowbot-sign-in-{}-{count}-{random:016x}",
        std::process::id()
    )
}

/// Whether the peer runs as the daemon's account.
fn same_user(stream: &tokio::net::UnixStream) -> bool {
    // SAFETY: `geteuid` has no preconditions.
    let own = unsafe { libc::geteuid() };
    stream.peer_cred().is_ok_and(|peer| peer.uid() == own)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_web_page_on_one_line_is_taken() {
        assert_eq!(
            web_page(" https://claude.ai/oauth?x=1\n"),
            Some("https://claude.ai/oauth?x=1")
        );
        assert_eq!(
            web_page("http://localhost:1455/"),
            Some("http://localhost:1455/")
        );
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://a\nhttps://b",
            "",
        ] {
            assert_eq!(web_page(refused), None, "{refused}");
        }
        assert_eq!(
            web_page(&format!("https://{}", "a".repeat(PAGE_BYTES))),
            None
        );
    }

    #[tokio::test]
    async fn an_opener_hands_its_page_to_the_catch_and_the_catch_leaves_nothing() {
        let root = crowbot_protocol::scratch::Scratch::new("crowbotd-page");
        let folder = root.join("sign-in");
        let (pages, mut caught) = tokio::sync::mpsc::unbounded_channel();
        let environment = BTreeMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]);
        let (catch, variables) = PageCatch::start(&folder, &environment, move |page| {
            pages.send(page).ok();
        })
        .unwrap();
        let own = std::env::split_paths(&variables["PATH"]).collect::<Vec<_>>();
        assert_eq!(own.len(), 2);
        assert_eq!(own[0].parent(), Some(folder.as_path()));
        assert_eq!(own[1], Path::new("/usr/bin"));
        assert_eq!(Path::new(&variables["BROWSER"]), own[0].join(OPENER));
        let address =
            SocketAddr::from_abstract_name(variables[SOCKET_VARIABLE].as_bytes()).unwrap();
        let send = |page: &'static [u8]| {
            let address = address.clone();
            tokio::task::spawn_blocking(move || {
                UnixStream::connect_addr(&address)
                    .and_then(|mut stream| stream.write_all(page))
                    .unwrap();
            })
        };
        send(b"file:///etc/passwd\n").await.unwrap();
        send(b"https://example.org/sign-in\n").await.unwrap();
        assert_eq!(caught.recv().await.unwrap(), "https://example.org/sign-in");
        assert!(own[0].join(OPENER).is_symlink());
        drop(catch);
        assert!(!Path::new(&variables["BROWSER"]).exists());
        assert!(std::fs::read_dir(&folder).unwrap().next().is_none());
        // The socket closed with the catch.
        tokio::task::yield_now().await;
        assert!(UnixStream::connect_addr(&address).is_err());
    }

    #[tokio::test]
    async fn a_catch_that_ends_leaves_the_opener_of_the_next_sign_in_alone() {
        let root = crowbot_protocol::scratch::Scratch::new("crowbotd-page");
        let folder = root.join("sign-in");
        let environment = BTreeMap::new();
        // A cancelled sign-in ends after the next one started: its catch is
        // dropped last.
        let (first, _) = PageCatch::start(&folder, &environment, |_| {}).unwrap();
        let (second, variables) = PageCatch::start(&folder, &environment, |_| {}).unwrap();
        drop(first);
        let opener = Path::new(&variables["BROWSER"]);
        assert!(opener.is_symlink());
        let path = std::env::split_paths(&variables["PATH"]).next().unwrap();
        assert!(path.join(OPENER).is_symlink());
        drop(second);
        assert!(std::fs::read_dir(&folder).unwrap().next().is_none());
    }
}
