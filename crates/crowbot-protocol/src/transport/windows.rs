use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::time::Duration;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::BoxedStream;
use crate::Endpoint;

const ERROR_PIPE_BUSY: i32 = 231;

/// The account a process runs as, read from its access token.
struct UserSid {
    /// A `TOKEN_USER` followed by the SID it points to, as `GetTokenInformation` wrote them.
    /// `u64` elements keep the buffer aligned for `TOKEN_USER`.
    token_user: Vec<u64>,
}

impl UserSid {
    fn of_current_process() -> io::Result<Self> {
        // SAFETY: `GetCurrentProcess` returns a pseudo handle that is always valid
        // and needs no close.
        Self::of_process(unsafe { GetCurrentProcess() })
    }

    fn of_process(process: HANDLE) -> io::Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `process` is a live process handle and `token` is a valid out pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `OpenProcessToken` succeeded, so `token` is a handle this code owns.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };

        let mut needed = 0_u32;
        // SAFETY: a null buffer of length zero only asks for the size, written to `needed`.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &raw mut needed,
            );
        }
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut token_user = vec![0_u64; (needed as usize).div_ceil(size_of::<u64>())];
        // SAFETY: the buffer holds at least `needed` writable bytes.
        let filled = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                token_user.as_mut_ptr().cast(),
                needed,
                &raw mut needed,
            )
        };
        if filled == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { token_user })
    }

    fn as_psid(&self) -> PSID {
        // SAFETY: `GetTokenInformation` filled the buffer with a `TOKEN_USER`, and the
        // buffer is aligned for it. The SID pointer targets the same buffer, which
        // lives as long as `self`.
        unsafe { (*self.token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn same_account(&self, other: &Self) -> bool {
        // SAFETY: both pointers are valid SIDs owned by their buffers.
        unsafe { EqualSid(self.as_psid(), other.as_psid()) != 0 }
    }

    fn to_sddl(&self) -> io::Result<String> {
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: the SID is valid and `text` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(self.as_psid(), &raw mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success `text` is a NUL-terminated string allocated with
        // `LocalAlloc`. It is read up to its terminator, then freed exactly once.
        let sid = unsafe {
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            sid
        };
        Ok(sid)
    }
}

/// A descriptor that makes the account of this process the owner of the pipe, with
/// an access list that grants the pipe to that account and to nobody else.
struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: the descriptor is immutable memory from `LocalAlloc`, owned by this value
// and freed once on drop. Nothing about it is tied to a thread.
unsafe impl Send for PipeSecurity {}

impl PipeSecurity {
    fn current_user_only() -> io::Result<Self> {
        let sid = UserSid::of_current_process()?.to_sddl()?;
        // This account as the owner ("O:"), which the client checks, and a protected
        // access list ("D:P") with one entry: allow ("A") all access ("GA") to this
        // account. Every other account is refused by omission.
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and `descriptor` is a valid out pointer.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor })
    }

    fn create(&self, options: &ServerOptions, address: &str) -> io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` is a valid `SECURITY_ATTRIBUTES` whose descriptor
        // outlives the call.
        unsafe {
            options.create_with_security_attributes_raw(address, (&raw mut attributes).cast())
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from `LocalAlloc` and is freed only here.
        unsafe {
            LocalFree(self.descriptor.cast());
        }
    }
}

pub struct Listener {
    address: String,
    security: PipeSecurity,
    next: NamedPipeServer,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let security = PipeSecurity::current_user_only()?;
        let next = security.create(
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true),
            endpoint.address(),
        )?;
        Ok(Self {
            address: endpoint.address().to_owned(),
            security,
            next,
        })
    }

    pub async fn accept(&mut self) -> io::Result<BoxedStream> {
        let connected = self.next.connect().await;
        let fresh = self.security.create(
            ServerOptions::new().reject_remote_clients(true),
            &self.address,
        )?;
        let stream = std::mem::replace(&mut self.next, fresh);
        connected?;
        Ok(Box::new(stream))
    }
}

pub async fn connect(endpoint: &Endpoint) -> io::Result<BoxedStream> {
    loop {
        // Identification level lets the server learn who the client is, and
        // never act with the client's rights.
        let opened = ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(endpoint.address());
        match opened {
            Ok(pipe) => {
                if !served_by_current_user(&pipe) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "the endpoint is served by a process of another user",
                    ));
                }
                return Ok(Box::new(pipe));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Two checks, and both must pass. The owner is read from the pipe this client
/// just opened: an account can only give its own pipes this owner, whatever
/// process id the pipe reports. The server process is then checked as well.
fn served_by_current_user(pipe: &NamedPipeClient) -> bool {
    if !owned_by_current_user(pipe.as_raw_handle()).unwrap_or(false) {
        return false;
    }
    let mut server = 0_u32;
    // SAFETY: the pipe handle is open and `server` is a valid out pointer.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &raw mut server) } == 0 {
        return false;
    }
    runs_as_current_user(server)
}

/// Whether the object behind `handle` has the account of this process as its owner.
fn owned_by_current_user(handle: HANDLE) -> io::Result<bool> {
    let ours = UserSid::of_current_process()?;
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle is open, only the owner is requested, and `owner` and
    // `descriptor` are valid out pointers.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    // SAFETY: on success `owner` is null or points to a SID inside `descriptor`,
    // which came from `LocalAlloc` and is freed once, after the comparison.
    let same = unsafe {
        let same = !owner.is_null() && EqualSid(owner, ours.as_psid()) != 0;
        LocalFree(descriptor.cast());
        same
    };
    Ok(same)
}

/// A process that cannot be inspected counts as another user's.
fn runs_as_current_user(process_id: u32) -> bool {
    // SAFETY: `OpenProcess` takes plain values and returns null on failure.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return false;
    }
    // SAFETY: `OpenProcess` succeeded, so `process` is a handle this code owns.
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    match (
        UserSid::of_process(process.as_raw_handle()),
        UserSid::of_current_process(),
    ) {
        (Ok(theirs), Ok(ours)) => theirs.same_account(&ours),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    use super::*;

    const SYSTEM_PROCESS: u32 = 4;

    fn access_list(pipe: &NamedPipeServer) -> String {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the pipe handle is open, only the descriptor is requested, and
        // `descriptor` is a valid out pointer.
        let status = unsafe {
            GetSecurityInfo(
                pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        assert_eq!(status, 0, "cannot read the access list of the pipe");
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: `descriptor` is valid on success and `text` is a valid out pointer.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &raw mut text,
                ptr::null_mut(),
            )
        };
        assert_ne!(converted, 0, "cannot print the access list of the pipe");
        // SAFETY: `text` is NUL-terminated on success. Both allocations come from
        // `LocalAlloc` and are freed once.
        unsafe {
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            LocalFree(descriptor.cast());
            sddl
        }
    }

    fn test_endpoint(name: &str) -> Endpoint {
        Endpoint::new(format!(
            r"\\.\pipe\crowbot-test-{name}-{}",
            std::process::id()
        ))
    }

    /// Whether a trustee of an SDDL text names the account of this process.
    /// SDDL writes some accounts with an alias, as `LA` for the built-in
    /// administrator that GitHub's Windows runners use, so the SIDs themselves
    /// are compared.
    fn names_this_account(trustee: &str) -> bool {
        let text: Vec<u16> = trustee.encode_utf16().chain([0]).collect();
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `text` is NUL-terminated and `sid` is a valid out pointer.
        let converted = unsafe { ConvertStringSidToSidW(text.as_ptr(), &raw mut sid) };
        assert_ne!(converted, 0, "{trustee} is not a SID");
        let ours = UserSid::of_current_process().unwrap();
        // SAFETY: both SIDs are valid; `sid` comes from `LocalAlloc` and is
        // freed once.
        unsafe {
            let same = EqualSid(sid, ours.as_psid()) != 0;
            LocalFree(sid.cast());
            same
        }
    }

    #[tokio::test]
    async fn the_pipe_grants_access_to_the_current_user_only() {
        let listener = Listener::bind(&test_endpoint("acl")).unwrap();
        let sddl = access_list(&listener.next);
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert_eq!(sddl.matches('(').count(), 1, "{sddl}");
        assert!(sddl.contains("(A;;"), "{sddl}");
        let trustee = sddl
            .strip_suffix(')')
            .and_then(|entry| entry.rsplit(";;;").next())
            .unwrap();
        assert!(names_this_account(trustee), "{sddl}");
        // The comparison refuses another account.
        assert!(!names_this_account("SY"));
    }

    #[tokio::test]
    async fn later_pipe_instances_keep_the_same_access_list() {
        let endpoint = test_endpoint("acl-next");
        let mut listener = Listener::bind(&endpoint).unwrap();
        let first = access_list(&listener.next);
        let accepting = tokio::spawn(async move {
            listener.accept().await.unwrap();
            listener
        });
        let _client = connect(&endpoint).await.unwrap();
        let listener = accepting.await.unwrap();
        assert_eq!(access_list(&listener.next), first);
    }

    #[tokio::test]
    async fn the_pipe_belongs_to_the_current_user() {
        let endpoint = test_endpoint("owner");
        let mut listener = Listener::bind(&endpoint).unwrap();
        assert!(owned_by_current_user(listener.next.as_raw_handle()).unwrap());
        let accepting = tokio::spawn(async move {
            listener.accept().await.unwrap();
            listener
        });
        let client = ClientOptions::new().open(endpoint.address()).unwrap();
        assert!(owned_by_current_user(client.as_raw_handle()).unwrap());
        assert!(served_by_current_user(&client));
        let listener = accepting.await.unwrap();
        assert!(owned_by_current_user(listener.next.as_raw_handle()).unwrap());
    }

    #[test]
    fn an_object_of_another_account_is_refused() {
        // Windows system files belong to the system installer, not to a user.
        let root = std::env::var_os("SystemRoot").unwrap();
        let system_file = std::path::Path::new(&root)
            .join("System32")
            .join("ntdll.dll");
        let file = std::fs::File::open(system_file).unwrap();
        assert!(!owned_by_current_user(file.as_raw_handle()).unwrap());
    }

    #[test]
    fn this_process_runs_as_the_current_user() {
        assert!(runs_as_current_user(std::process::id()));
    }

    #[test]
    fn a_process_of_another_account_is_refused() {
        assert!(!runs_as_current_user(SYSTEM_PROCESS));
    }

    #[test]
    fn a_process_that_does_not_exist_is_refused() {
        assert!(!runs_as_current_user(u32::MAX - 3));
    }
}
