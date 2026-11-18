//! Windows: the icon Explorer shows, from the shell (`SHGetFileInfoW`). Files whose icon depends
//! on the file itself (programs, shortcuts, `.ico`, folders with a custom icon) are asked for by
//! path, falling back to the icon embedded in the file and then to the type's icon; all others
//! by extension only, which needs no disk access. The system image list index identifies a
//! picture, so each one is converted once.

use std::collections::{HashMap, HashSet};
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDIBits, GetObjectW, HBITMAP,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use super::{Found, Pixels};

/// Not needed: shell icons have their own colors.
pub struct Colors;

/// Extensions whose icon comes from the file itself.
const OWN_ICON: &[&str] = &["exe", "lnk", "ico", "url", "cpl", "msc", "scr", "appref-ms", "dll"];

/// Extensions whose embedded icon `ExtractIconExW` can read without the shell.
const EMBEDDED: &[&str] = &["exe", "dll", "ico", "cpl", "scr"];

pub struct Loader {
    /// Extension (`""` for none, `/` for folders) → system image list index.
    generic: HashMap<String, Option<i32>>,
    sent: HashSet<String>,
}

impl Loader {
    pub fn new(_: Colors, _px: u32) -> Loader {
        // The shell needs COM on this thread.
        unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32) };
        Loader { generic: HashMap::new(), sent: HashSet::new() }
    }

    /// `id` with its pixels, unless already sent.
    fn found(&mut self, id: String, pixels: impl FnOnce() -> Option<Pixels>) -> Found {
        if self.sent.contains(&id) {
            return Some((id, None));
        }
        let p = pixels()?;
        self.sent.insert(id.clone());
        Some((id, Some(p)))
    }

    pub fn load(&mut self, path: &[u8], is_dir: bool) -> Found {
        let path = String::from_utf8_lossy(path);
        let name = path.rsplit('\\').next().unwrap_or(&path);
        let ext = match name.rsplit_once('.') {
            _ if is_dir => "/".to_string(),
            Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
            _ => String::new(),
        };
        if is_dir || OWN_ICON.contains(&ext.as_str()) {
            // The icon Explorer shows for this very file.
            if let Some(i) = icon_index(&path, is_dir, false) {
                return self.found(format!("#{i}"), || icon_pixels(&path, is_dir, false));
            }
            if EMBEDDED.contains(&ext.as_str())
                && let Some(f) = self.found(format!("file:{path}"), || embedded_icon(&path))
            {
                return Some(f);
            }
        }
        // The icon for the type (by name only, no disk access).
        let shown = if is_dir { "x".to_string() } else { format!("x.{ext}") };
        let index = *self.generic.entry(ext).or_insert_with(|| icon_index(&shown, is_dir, true));
        self.found(format!("#{}", index?), || icon_pixels(&shown, is_dir, true))
    }
}

/// The first icon embedded in a program, library or `.ico` file.
fn embedded_icon(path: &str) -> Option<Pixels> {
    let mut large: HICON = null_mut();
    let n = unsafe { ExtractIconExW(wide(path).as_ptr(), 0, &mut large, null_mut(), 1) };
    if n == 0 || n == u32::MAX || large.is_null() {
        return None;
    }
    let pixels = unsafe { to_rgba(large) };
    unsafe { DestroyIcon(large) };
    pixels
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn shell_info(path: &str, is_dir: bool, by_name_only: bool, flags: u32) -> Option<SHFILEINFOW> {
    let mut info: SHFILEINFOW = unsafe { zeroed() };
    let attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let flags = flags | if by_name_only { SHGFI_USEFILEATTRIBUTES } else { 0 };
    let r = unsafe { SHGetFileInfoW(wide(path).as_ptr(), attrs, &mut info, size_of::<SHFILEINFOW>() as u32, flags) };
    (r != 0).then_some(info)
}

fn icon_index(path: &str, is_dir: bool, by_name_only: bool) -> Option<i32> {
    shell_info(path, is_dir, by_name_only, SHGFI_SYSICONINDEX).map(|i| i.iIcon)
}

/// The 32×32 icon as RGBA.
fn icon_pixels(path: &str, is_dir: bool, by_name_only: bool) -> Option<Pixels> {
    let info = shell_info(path, is_dir, by_name_only, SHGFI_ICON | SHGFI_LARGEICON)?;
    let icon: HICON = info.hIcon;
    if icon.is_null() {
        return None;
    }
    let pixels = unsafe { to_rgba(icon) };
    unsafe { DestroyIcon(icon) };
    pixels
}

/// 32-bit top-down BGRA rows of a bitmap.
unsafe fn bits(bitmap: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bmi: BITMAPINFO = unsafe { zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { zeroed() }
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let dc = unsafe { CreateCompatibleDC(null_mut()) };
    let lines = unsafe { GetDIBits(dc, bitmap, 0, h as u32, buf.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS) };
    unsafe { DeleteDC(dc) };
    (lines == h).then_some(buf)
}

unsafe fn to_rgba(icon: HICON) -> Option<Pixels> {
    let mut ii: ICONINFO = unsafe { zeroed() };
    if unsafe { GetIconInfo(icon, &mut ii) } == 0 {
        return None;
    }
    let result = (|| {
        if ii.hbmColor.is_null() {
            return None;
        }
        let mut bm: BITMAP = unsafe { zeroed() };
        if unsafe { GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&raw mut bm).cast()) } == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut px = unsafe { bits(ii.hbmColor, w, h) }?;
        // Old icons have no alpha channel: take transparency from the mask (set = transparent).
        if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            let mask = unsafe { bits(ii.hbmMask, w, h) }?;
            for (p, m) in px.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
                p[3] = if m[0] == 0 { 255 } else { 0 };
            }
        }
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        Some(Pixels { w: w as u32, h: h as u32, rgba: px })
    })();
    unsafe {
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Programs get their own icon; other files the one for their extension, shared.
    #[test]
    fn shell_icons() {
        let mut l = Loader::new(Colors, 32);
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let (id, px) = l.load(format!(r"{windir}\notepad.exe").as_bytes(), false).expect("notepad icon");
        let px = px.expect("pixels on first load");
        assert_eq!((px.w, px.h, px.rgba.len()), (32, 32, 32 * 32 * 4));
        assert!(px.rgba.chunks_exact(4).any(|p| p[3] > 0), "not fully transparent");
        let (a, _) = l.load(br"C:\x\a.txt", false).expect("txt icon");
        let (b, again) = l.load(br"C:\y\b.TXT", false).expect("txt icon");
        assert_eq!((a.as_str(), again.is_none()), (b.as_str(), true));
        assert_ne!(a, id);
        assert!(l.load(windir.as_bytes(), true).is_some());
    }
}
