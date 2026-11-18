//! Windows: the `hotkey` setting starts `finder` directly.
//!
//! A shortcut key on a `.lnk` file is handled by Explorer, which first asks every top-level window
//! whether it owns that key and waits ~3 s for each one that is hung. `RegisterHotKey` has no
//! such delay. The key is re-registered whenever `settings.json` changes.

use std::os::windows::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::ptr::null_mut;
use std::thread;

use finder_core::hotkey::{self, Hotkey};
use finder_core::paths;
use finder_core::settings::Settings;
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
    FindNextChangeNotification,
};
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_HOTKEY, WM_HOTKEY,
};

const ID: i32 = 1;

pub fn spawn() {
    if let Err(e) = thread::Builder::new().name("hotkey".into()).spawn(run) {
        eprintln!("finderd: cannot start hotkey thread: {e}");
    }
}

fn wanted() -> Option<Hotkey> {
    hotkey::parse(&Settings::load()?.0.hotkey)
}

fn run() {
    // `main` creates the data dir, so it can be watched before `settings.json` exists.
    let mut want = wanted();
    let dir: Vec<u16> = paths::data_dir().as_os_str().encode_wide().chain([0]).collect();
    let change = unsafe {
        FindFirstChangeNotificationW(dir.as_ptr(), 0, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME)
    };
    let handles = if change == INVALID_HANDLE_VALUE {
        eprintln!(
            "finderd: cannot watch settings ({}); hotkey changes need a restart",
            std::io::Error::last_os_error()
        );
        Vec::new()
    } else {
        vec![change]
    };
    let mut current: Option<Hotkey> = None;
    loop {
        if want != current {
            if current.take().is_some() {
                unsafe { UnregisterHotKey(null_mut(), ID) };
            }
            if let Some(hk) = want {
                if unsafe { RegisterHotKey(null_mut(), ID, hk.mods | MOD_NOREPEAT, hk.vk) } != 0 {
                    current = Some(hk);
                } else {
                    eprintln!("finderd: cannot register hotkey: {}", std::io::Error::last_os_error());
                }
            }
        }
        let n = handles.len() as u32;
        let r = unsafe { MsgWaitForMultipleObjects(n, handles.as_ptr(), 0, INFINITE, QS_HOTKEY) };
        if n == 1 && r == WAIT_OBJECT_0 {
            unsafe { FindNextChangeNotification(change) };
            // Also retries a registration that failed (key taken by another app).
            want = wanted();
            continue;
        }
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_HOTKEY {
                launch();
            }
        }
    }
}

/// Start `finder.exe` from next to this executable. The hotkey made us the process that received
/// the last input, so we may pass the right to take the foreground on to it.
fn launch() {
    let Some(ui) = paths::sibling_exe("finder") else {
        return eprintln!("finderd: finder.exe not found next to finderd.exe");
    };
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    if let Err(e) = Command::new(&ui).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        eprintln!("finderd: cannot start {}: {e}", ui.display());
    }
}
