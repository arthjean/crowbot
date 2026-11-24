use std::fs::File;
use std::io;
use std::path::Path;

pub use sys::Dir;

pub struct Root {
    dir: Dir,
}

pub fn split(path: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    (parts, name)
}

pub fn is_moved(error: &io::Error) -> bool {
    let moved = matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    );
    #[cfg(unix)]
    let moved = moved || error.raw_os_error() == Some(libc::ELOOP);
    moved
}

impl Root {
    pub fn open(folder: &Path) -> io::Result<Self> {
        Ok(Self {
            dir: Dir::open_root(folder)?,
        })
    }

    pub fn dir(&self, parents: &[&str], create: bool) -> io::Result<Dir> {
        if parents.is_empty() {
            return self.dir.try_clone();
        }
        #[cfg(target_os = "linux")]
        match self.dir.open_beneath_dir(&parents.join("/")) {
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound && create => {}
            Some(opened) => return opened,
            None => {}
        }
        self.walk(parents, create)
    }

    fn walk(&self, parents: &[&str], create: bool) -> io::Result<Dir> {
        let mut dir = self.dir.try_clone()?;
        for part in parents {
            dir = dir.child(part, create)?;
        }
        Ok(dir)
    }

    pub fn open_file(&self, path: &str) -> io::Result<File> {
        #[cfg(target_os = "linux")]
        if let Some(opened) = self.dir.open_beneath_file(path) {
            return opened;
        }
        self.walk_file(path)
    }

    fn walk_file(&self, path: &str) -> io::Result<File> {
        let (parents, name) = split(path);
        self.walk(&parents, false)?.open_file(name)
    }

    pub fn holds(&self, path: &str) -> bool {
        let (parents, name) = split(path);
        self.dir(&parents, false)
            .ok()
            .and_then(|dir| dir.kind(name))
            .is_some_and(|kind| kind != Kind::Folder)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Link,
    Folder,
    Other,
}

#[cfg(unix)]
mod sys {
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::Kind;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub struct Dir {
        fd: OwnedFd,
    }

    fn name(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn checked(result: libc::c_int) -> io::Result<()> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn opened(fd: libc::c_int) -> io::Result<OwnedFd> {
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a descriptor the call just opened, owned by nobody else.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn openat(
        dir: RawFd,
        path: &CStr,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> io::Result<OwnedFd> {
        loop {
            // SAFETY: `path` is a valid C string that outlives the call.
            let fd = unsafe {
                libc::openat(
                    dir,
                    path.as_ptr(),
                    flags | libc::O_CLOEXEC,
                    libc::c_uint::from(mode),
                )
            };
            match opened(fd) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }

    #[cfg(target_os = "linux")]
    static BENEATH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

    #[cfg(target_os = "linux")]
    pub fn beneath_unavailable() {
        BENEATH.store(false, Ordering::Relaxed);
    }

    impl Dir {
        pub fn open_root(path: &Path) -> io::Result<Self> {
            let path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            let fd = openat(
                libc::AT_FDCWD,
                &path,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                0,
            )?;
            Ok(Self { fd })
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Self {
                fd: self.fd.try_clone()?,
            })
        }

        #[cfg(target_os = "linux")]
        fn openat2(&self, path: &str, flags: libc::c_int) -> Option<io::Result<OwnedFd>> {
            if !BENEATH.load(Ordering::Relaxed) {
                return None;
            }
            let path = match name(path) {
                Ok(path) => path,
                Err(error) => return Some(Err(error)),
            };
            // SAFETY: `open_how` is plain data, for which all zeroes is valid.
            let mut how: libc::open_how = unsafe { std::mem::zeroed() };
            how.flags = u64::try_from(flags | libc::O_CLOEXEC).unwrap_or_default();
            how.resolve =
                libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
            loop {
                // SAFETY: `path` and `how` are valid for the call, and the size
                // given is the size of `how`.
                let fd = unsafe {
                    libc::syscall(
                        libc::SYS_openat2,
                        self.fd.as_raw_fd(),
                        path.as_ptr(),
                        &raw const how,
                        std::mem::size_of::<libc::open_how>(),
                    )
                };
                let fd = libc::c_int::try_from(fd).unwrap_or(-1);
                match opened(fd) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error)
                        if matches!(
                            error.raw_os_error(),
                            Some(libc::ENOSYS | libc::EPERM | libc::E2BIG | libc::EINVAL)
                        ) =>
                    {
                        beneath_unavailable();
                        return None;
                    }
                    result => return Some(result),
                }
            }
        }

        #[cfg(target_os = "linux")]
        pub fn open_beneath_dir(&self, path: &str) -> Option<io::Result<Self>> {
            self.openat2(path, libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .map(|result| result.map(|fd| Self { fd }))
        }

        #[cfg(target_os = "linux")]
        pub fn open_beneath_file(&self, path: &str) -> Option<io::Result<File>> {
            self.openat2(path, libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .map(|result| result.map(File::from))
        }

        pub fn child(&self, part: &str, create: bool) -> io::Result<Self> {
            let part = name(part)?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
            match openat(self.fd.as_raw_fd(), &part, flags, 0) {
                Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                    // SAFETY: `part` is a valid C string that outlives the call.
                    let made = unsafe { libc::mkdirat(self.fd.as_raw_fd(), part.as_ptr(), 0o777) };
                    match checked(made) {
                        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                            return Err(error);
                        }
                        _ => {}
                    }
                    openat(self.fd.as_raw_fd(), &part, flags, 0).map(|fd| Self { fd })
                }
                result => result.map(|fd| Self { fd }),
            }
        }

        pub fn open_file(&self, part: &str) -> io::Result<File> {
            let part = name(part)?;
            openat(
                self.fd.as_raw_fd(),
                &part,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0,
            )
            .map(File::from)
        }

        fn stat(&self, part: &CStr) -> io::Result<libc::stat> {
            // SAFETY: `stat` is plain data, for which all zeroes is valid.
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: `part` is a valid C string and `stat` a valid place to write.
            let result = unsafe {
                libc::fstatat(
                    self.fd.as_raw_fd(),
                    part.as_ptr(),
                    &raw mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            checked(result).map(|()| stat)
        }

        pub fn kind(&self, part: &str) -> Option<Kind> {
            let stat = self.stat(&name(part).ok()?).ok()?;
            Some(match stat.st_mode & libc::S_IFMT {
                libc::S_IFREG => Kind::File,
                libc::S_IFLNK => Kind::Link,
                libc::S_IFDIR => Kind::Folder,
                _ => Kind::Other,
            })
        }

        #[allow(
            clippy::useless_conversion,
            reason = "mode_t is narrower than u32 on macOS"
        )]
        pub fn mode(&self, part: &str) -> Option<u32> {
            let stat = self.stat(&name(part).ok()?).ok()?;
            (stat.st_mode & libc::S_IFMT == libc::S_IFREG)
                .then_some(u32::from(stat.st_mode) & 0o777)
        }

        pub fn write_file(&self, part: &str, content: &[u8], mode: u32) -> io::Result<()> {
            let target = name(part)?;
            let partial = name(&format!(
                ".crowbot-restore-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))?;
            let dir = self.fd.as_raw_fd();
            let written = (|| {
                let mut file = File::from(openat(
                    dir,
                    &partial,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
                    0o600,
                )?);
                file.write_all(content)?;
                let mode = libc::mode_t::try_from(mode & 0o777).unwrap_or(0o600);
                // SAFETY: the descriptor is open for the life of `file`.
                checked(unsafe { libc::fchmod(file.as_raw_fd(), mode) })?;
                file.sync_all()?;
                drop(file);
                // SAFETY: both names are valid C strings that outlive the call.
                checked(unsafe { libc::renameat(dir, partial.as_ptr(), dir, target.as_ptr()) })
            })();
            if written.is_err() {
                // SAFETY: `partial` is a valid C string that outlives the call.
                unsafe { libc::unlinkat(dir, partial.as_ptr(), 0) };
            }
            written
        }

        pub fn set_mode(&self, part: &str, mode: u32) -> io::Result<()> {
            let file = self.open_file(part)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
            let mode = libc::mode_t::try_from(mode & 0o777).unwrap_or(0o600);
            // SAFETY: the descriptor is open for the life of `file`.
            checked(unsafe { libc::fchmod(file.as_raw_fd(), mode) })
        }

        fn unlink(&self, part: &str, flags: libc::c_int) -> io::Result<()> {
            let part = name(part)?;
            // SAFETY: `part` is a valid C string that outlives the call.
            checked(unsafe { libc::unlinkat(self.fd.as_raw_fd(), part.as_ptr(), flags) })
        }

        pub fn remove_file(&self, part: &str) -> io::Result<()> {
            match self.unlink(part, 0) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }

        pub fn remove_dir(&self, part: &str) -> io::Result<()> {
            self.unlink(part, libc::AT_REMOVEDIR)
        }

        pub fn symlink(&self, target: &str, part: &str) -> io::Result<()> {
            let (target, part) = (name(target)?, name(part)?);
            // SAFETY: both are valid C strings that outlive the call.
            checked(unsafe { libc::symlinkat(target.as_ptr(), self.fd.as_raw_fd(), part.as_ptr()) })
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::Kind;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub struct Dir {
        path: PathBuf,
        pins: Arc<Vec<File>>,
    }

    fn pin(path: &Path) -> io::Result<File> {
        let folder = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let attributes = folder.metadata()?.file_attributes();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a folder", path.display()),
            ));
        }
        Ok(folder)
    }

    impl Dir {
        pub fn open_root(path: &Path) -> io::Result<Self> {
            Ok(Self {
                path: path.to_owned(),
                pins: Arc::new(vec![pin(path)?]),
            })
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Self {
                path: self.path.clone(),
                pins: self.pins.clone(),
            })
        }

        pub fn child(&self, part: &str, create: bool) -> io::Result<Self> {
            let path = self.path.join(part);
            let pinned = match pin(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                    match std::fs::create_dir(&path) {
                        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                            return Err(error);
                        }
                        _ => {}
                    }
                    pin(&path)?
                }
                result => result?,
            };
            let mut pins: Vec<File> = Vec::with_capacity(self.pins.len() + 1);
            for held in self.pins.iter() {
                pins.push(held.try_clone()?);
            }
            pins.push(pinned);
            Ok(Self {
                path,
                pins: Arc::new(pins),
            })
        }

        pub fn open_file(&self, part: &str) -> io::Result<File> {
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(self.path.join(part))
        }

        pub fn kind(&self, part: &str) -> Option<Kind> {
            let metadata = self.path.join(part).symlink_metadata().ok()?;
            let kind = metadata.file_type();
            Some(if kind.is_symlink() {
                Kind::Link
            } else if kind.is_dir() {
                Kind::Folder
            } else if kind.is_file() {
                Kind::File
            } else {
                Kind::Other
            })
        }

        pub fn mode(&self, part: &str) -> Option<u32> {
            let metadata = self.path.join(part).symlink_metadata().ok()?;
            metadata.is_file().then(|| {
                if metadata.permissions().readonly() {
                    0o444
                } else {
                    0o666
                }
            })
        }

        pub fn write_file(&self, part: &str, content: &[u8], mode: u32) -> io::Result<()> {
            let target = self.path.join(part);
            let partial = self.path.join(format!(
                ".crowbot-restore-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let written = (|| {
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&partial)?;
                file.write_all(content)?;
                file.sync_all()?;
                let mut permissions = file.metadata()?.permissions();
                permissions.set_readonly(mode & 0o222 == 0);
                file.set_permissions(permissions)?;
                drop(file);
                if let Ok(existing) = target.symlink_metadata()
                    && existing.is_file()
                    && existing.permissions().readonly()
                {
                    let mut permissions = existing.permissions();
                    #[allow(
                        clippy::permissions_set_readonly_false,
                        reason = "Windows clears the read-only attribute alone"
                    )]
                    permissions.set_readonly(false);
                    std::fs::set_permissions(&target, permissions)?;
                }
                std::fs::rename(&partial, &target)
            })();
            if written.is_err() {
                std::fs::remove_file(&partial).ok();
            }
            written
        }

        pub fn set_mode(&self, part: &str, mode: u32) -> io::Result<()> {
            let path = self.path.join(part);
            let metadata = path.symlink_metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
            let mut permissions = metadata.permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "Windows clears the read-only attribute alone"
            )]
            permissions.set_readonly(mode & 0o222 == 0);
            std::fs::set_permissions(path, permissions)
        }

        pub fn remove_file(&self, part: &str) -> io::Result<()> {
            match std::fs::remove_file(self.path.join(part)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }

        pub fn remove_dir(&self, part: &str) -> io::Result<()> {
            std::fs::remove_dir(self.path.join(part))
        }

        pub fn symlink(&self, target: &str, part: &str) -> io::Result<()> {
            let place = self.path.join(part);
            if self.path.join(target).is_dir() {
                std::os::windows::fs::symlink_dir(target, place)
            } else {
                std::os::windows::fs::symlink_file(target, place)
            }
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crowbot_protocol::scratch::Scratch;

    struct Bench {
        _scratch: Scratch,
        folder: std::path::PathBuf,
        outside: std::path::PathBuf,
        root: Root,
    }

    fn bench() -> Bench {
        let scratch = Scratch::new("beneath");
        let folder = scratch.path().join("bot");
        let outside = scratch.path().join("outside");
        std::fs::create_dir_all(folder.join("real")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"outside").unwrap();
        std::fs::write(folder.join("real/inside"), b"inside").unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("away")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), folder.join("real/link")).unwrap();
        let root = Root::open(&folder).unwrap();
        Bench {
            _scratch: scratch,
            folder,
            outside,
            root,
        }
    }

    type Open = fn(&Root, &str) -> io::Result<File>;

    fn content(mut file: File) -> Vec<u8> {
        use std::io::Read;
        let mut read = Vec::new();
        file.read_to_end(&mut read).unwrap();
        read
    }

    #[test]
    fn both_ways_of_opening_stay_inside_the_folder() {
        let bench = bench();
        let opens: [(&str, Open); 2] = [
            ("open_file", Root::open_file),
            ("walk_file", Root::walk_file),
        ];
        for (how, open) in opens {
            assert_eq!(
                content(open(&bench.root, "real/inside").unwrap()),
                b"inside",
                "{how}"
            );
            for path in ["away/secret", "real/link"] {
                let error = open(&bench.root, path).unwrap_err();
                assert!(is_moved(&error), "{how} {path}: {error}");
            }
        }
        let error = bench.root.walk(&["away"], true).err().unwrap();
        assert!(is_moved(&error), "{error}");
        let error = bench.root.dir(&["away"], true).err().unwrap();
        assert!(is_moved(&error), "{error}");
    }

    #[test]
    fn a_folder_is_created_and_written_one_step_at_a_time() {
        let bench = bench();
        for (how, deep) in [("dir", "made"), ("walk", "walked")] {
            let parents = ["real", deep, "deeper"];
            let dir = if how == "dir" {
                bench.root.dir(&parents, true)
            } else {
                bench.root.walk(&parents, true)
            }
            .unwrap();
            dir.write_file("file", b"written", 0o640).unwrap();
            let written = bench.folder.join("real").join(deep).join("deeper/file");
            assert_eq!(std::fs::read(&written).unwrap(), b"written");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                written.metadata().unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        let real = bench.root.dir(&["real"], false).unwrap();
        real.write_file("link", b"over the link", 0o600).unwrap();
        assert_eq!(
            std::fs::read(bench.outside.join("secret")).unwrap(),
            b"outside"
        );
        assert_eq!(real.kind("link"), Some(Kind::File));
        assert!(bench.root.holds("real/inside"));
        assert!(!bench.root.holds("real"));
        assert!(!bench.root.holds("away/secret"));
    }

    #[test]
    fn a_folder_swapped_for_a_link_while_a_restore_writes_never_leads_outside() {
        let bench = bench();
        let swapped = bench.folder.join("swapped");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let swapper = {
            let (swapped, outside, stop) = (swapped.clone(), bench.outside.clone(), stop.clone());
            std::thread::spawn(move || {
                let parked = swapped.with_file_name("parked");
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::fs::create_dir(&swapped).ok();
                    std::fs::rename(&swapped, &parked).ok();
                    std::os::unix::fs::symlink(&outside, &swapped).ok();
                    std::fs::remove_file(&swapped).ok();
                    std::fs::remove_dir_all(&parked).ok();
                }
            })
        };
        for round in 0..2_000 {
            let dir = if round % 2 == 0 {
                bench.root.dir(&["swapped"], true)
            } else {
                bench.root.walk(&["swapped"], true)
            };
            if let Ok(dir) = dir {
                dir.write_file(&format!("file-{round}"), b"x", 0o600).ok();
            }
            if let Ok(dir) = bench.root.dir(&["swapped"], false) {
                dir.remove_file("secret").ok();
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        swapper.join().unwrap();
        let outside: Vec<_> = std::fs::read_dir(&bench.outside)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(outside, [std::ffi::OsString::from("secret")]);
    }
}
