//! Stop the process on the other end of a daemon connection: a `finderd` from an older version
//! that doesn't understand `Request::Quit`. The socket / pipe is private to the current user, so
//! the peer is always one of the user's own `finderd` processes.

use crate::proto::Stream;

#[cfg(target_os = "linux")]
pub fn stop_peer(c: &Stream) {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    let ok = unsafe {
        libc::getsockopt(c.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len)
    } == 0;
    if ok && cred.pid > 0 && cred.uid == unsafe { libc::getuid() } {
        unsafe { libc::kill(cred.pid, libc::SIGTERM) };
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn stop_peer(_: &Stream) {}

#[cfg(windows)]
pub fn stop_peer(c: &Stream) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(c.as_raw_handle(), &mut pid) } == 0 || pid == 0 {
        return;
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 0);
            CloseHandle(h);
        }
    }
}
