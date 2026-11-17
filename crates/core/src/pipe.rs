//! Windows IPC: a named pipe `\\.\pipe\finder-<user SID>`, accessible to the current user only.
//!
//! The daemon creates the pipe with an owner-only DACL and `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a
//! second daemon (or another user) cannot listen on it. Clients connect at identification level
//! (the server cannot impersonate them) and check that the server process runs as the same user.

use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
    GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

const BUF_SIZE: u32 = 64 * 1024;
/// How long a client keeps retrying while all pipe instances are busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Closes a handle on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// String SID (`S-1-5-21-…`) of the user a process runs as.
fn process_user_sid(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error());
        }
        let token = Owned(token);
        let mut len = 0u32;
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len);
        // u64 buffer: TOKEN_USER needs pointer alignment.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
            return Err(last_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return Err(last_error());
        }
        let n = (0..).take_while(|&i| *s.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
        LocalFree(s.cast());
        Ok(sid)
    }
}

fn current_user_sid() -> io::Result<String> {
    process_user_sid(unsafe { GetCurrentProcess() })
}

fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\finder-{sid}")
}

/// Server side: always keeps one instance waiting for the next client.
pub struct Listener {
    name: Vec<u16>,
    sa: SECURITY_ATTRIBUTES,
    pending: HANDLE,
}

// The raw pointers are an owned pipe handle and a security descriptor that lives as long as the listener.
unsafe impl Send for Listener {}

impl Listener {
    /// Create the pipe. Fails with `AddrInUse` if another daemon already owns it.
    pub fn bind() -> io::Result<Listener> {
        let sid = current_user_sid()?;
        Listener::bind_at(&pipe_name(&sid), &sid)
    }

    fn bind_at(name: &str, sid: &str) -> io::Result<Listener> {
        let in_use = || io::Error::new(io::ErrorKind::AddrInUse, "another finderd is already running");
        // Probe without connecting (a connect would use up the other daemon's waiting instance).
        // FILE_FLAG_FIRST_PIPE_INSTANCE below closes the race; this check also covers Wine, which ignores the flag.
        let wname = wide(name);
        if unsafe { WaitNamedPipeW(wname.as_ptr(), 1) } != 0 || unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND {
            return Err(in_use());
        }
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut())
        };
        if ok == 0 {
            return Err(last_error());
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let mut l = Listener { name: wname, sa, pending: INVALID_HANDLE_VALUE };
        l.pending = l.create(true).map_err(|e| match e.raw_os_error() {
            Some(c) if c == ERROR_ACCESS_DENIED as i32 => in_use(),
            _ => e,
        })?;
        Ok(l)
    }

    fn create(&self, first: bool) -> io::Result<HANDLE> {
        let open_mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let h = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                pipe_mode,
                PIPE_UNLIMITED_INSTANCES,
                BUF_SIZE,
                BUF_SIZE,
                0,
                &self.sa,
            )
        };
        if h == INVALID_HANDLE_VALUE { Err(last_error()) } else { Ok(h) }
    }

    /// Wait for the next client and return its connection.
    pub fn accept(&mut self) -> io::Result<File> {
        if self.pending == INVALID_HANDLE_VALUE {
            self.pending = self.create(false)?;
        }
        let h = self.pending;
        if unsafe { ConnectNamedPipe(h, null_mut()) } == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            let e = last_error();
            unsafe { CloseHandle(h) };
            self.pending = INVALID_HANDLE_VALUE;
            return Err(e);
        }
        // Have the next instance listening before handing this one off.
        self.pending = self.create(false).unwrap_or(INVALID_HANDLE_VALUE);
        Ok(unsafe { File::from_raw_handle(h) })
    }
}

/// Connect to the daemon's pipe and verify it is served by the current user.
pub fn connect() -> io::Result<File> {
    let sid = current_user_sid()?;
    connect_at(&pipe_name(&sid), &sid)
}

fn connect_at(name: &str, sid: &str) -> io::Result<File> {
    let name = wide(name);
    let start = Instant::now();
    let h = loop {
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            break h;
        }
        let e = last_error();
        if e.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) || start.elapsed() > BUSY_TIMEOUT {
            return Err(e);
        }
        unsafe { WaitNamedPipeW(name.as_ptr(), 200) };
    };
    let file = unsafe { File::from_raw_handle(h) };
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(h, &mut pid) } == 0 {
        return Err(last_error());
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(last_error());
    }
    let process = Owned(process);
    if process_user_sid(process.0)? != *sid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "finder pipe is served by another user"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn round_trip_and_single_owner() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        let name = format!(r"\\.\pipe\finder-test-{}", std::process::id());
        let mut l = Listener::bind_at(&name, &sid).unwrap();
        assert_eq!(Listener::bind_at(&name, &sid).err().map(|e| e.kind()), Some(io::ErrorKind::AddrInUse));
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let mut c = l.accept().unwrap();
                let mut buf = [0u8; 5];
                c.read_exact(&mut buf).unwrap();
                buf.reverse();
                c.write_all(&buf).unwrap();
            }
        });
        // Several clients in a row: each gets a fresh instance.
        for _ in 0..3 {
            let mut c = connect_at(&name, &sid).unwrap();
            c.write_all(b"hello").unwrap();
            let mut buf = [0u8; 5];
            c.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"olleh");
        }
        server.join().unwrap();
        assert!(connect_at(r"\\.\pipe\finder-test-missing", &sid).is_err());
    }
}
