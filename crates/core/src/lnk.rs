//! Windows shortcuts (`.lnk`, the MS-SHLLINK format): just enough to tell whether one starts a
//! program. Read directly (no COM), so it is fast and platform-neutral (tested everywhere).

/// Link flags.
const HAS_ID_LIST: u32 = 0x1;
const HAS_LINK_INFO: u32 = 0x2;
const HAS_NAME: u32 = 0x4;
const HAS_RELATIVE_PATH: u32 = 0x8;
const HAS_WORKING_DIR: u32 = 0x10;
const HAS_ARGUMENTS: u32 = 0x20;
const HAS_ICON_LOCATION: u32 = 0x40;
const IS_UNICODE: u32 = 0x80;
/// Extra data blocks.
const ENVIRONMENT_BLOCK: u32 = 0xA000_0001;
/// Installer ("advertised") shortcuts, such as Office's: they start the installed program.
const DARWIN_BLOCK: u32 = 0xA000_0006;

const HEADER_LEN: usize = 0x4C;
const MAGIC: [u8; 20] = [0x4C, 0, 0, 0, 0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

fn u16_at(d: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn u32_at(d: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// NUL-terminated 8-bit string (code page; non-ASCII is irrelevant for an extension check).
fn ansi_at(d: &[u8], at: usize) -> Option<String> {
    let s = d.get(at..)?;
    let end = s.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&s[..end]).into_owned())
}

/// NUL-terminated UTF-16 string, at most `max` units.
fn wide_at(d: &[u8], at: usize, max: usize) -> Option<String> {
    let units: Vec<u16> = d
        .get(at..)?
        .as_chunks::<2>()
        .0
        .iter()
        .take(max)
        .map(|&c| u16::from_le_bytes(c))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// What a shortcut points to, as far as the file says.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// Target path (local path, relative path or the environment-variable form).
    pub target: Option<String>,
    /// An installer ("advertised") shortcut.
    pub advertised: bool,
}

impl Link {
    pub fn parse(d: &[u8]) -> Option<Link> {
        if d.get(..MAGIC.len())? != MAGIC {
            return None;
        }
        let flags = u32_at(d, 0x14)? as u32;
        let mut at = HEADER_LEN;
        if flags & HAS_ID_LIST != 0 {
            at += 2 + u16_at(d, at)?;
        }
        let mut link = Link::default();
        if flags & HAS_LINK_INFO != 0 {
            let info = at;
            let size = u32_at(d, info)?;
            let header = u32_at(d, info + 4)?;
            let has_local = u32_at(d, info + 8)? & 1 != 0;
            if has_local {
                let base = if header >= 0x24 {
                    wide_at(d, info + u32_at(d, info + 0x1C)?, 32_768)
                } else {
                    ansi_at(d, info + u32_at(d, info + 0x10)?)
                };
                let suffix = if header >= 0x24 {
                    wide_at(d, info + u32_at(d, info + 0x20)?, 32_768)
                } else {
                    ansi_at(d, info + u32_at(d, info + 0x18)?)
                };
                link.target = base.map(|b| b + &suffix.unwrap_or_default());
            }
            at = info + size;
        }
        // String data: name, relative path, working dir, arguments, icon location.
        let unicode = flags & IS_UNICODE != 0;
        for f in [HAS_NAME, HAS_RELATIVE_PATH, HAS_WORKING_DIR, HAS_ARGUMENTS, HAS_ICON_LOCATION] {
            if flags & f == 0 {
                continue;
            }
            let n = u16_at(d, at)?;
            let bytes = if unicode { n * 2 } else { n };
            if f == HAS_RELATIVE_PATH && link.target.is_none() {
                let s = d.get(at + 2..at + 2 + bytes)?;
                link.target = Some(if unicode {
                    let units: Vec<u16> = s.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                    String::from_utf16_lossy(&units)
                } else {
                    String::from_utf8_lossy(s).into_owned()
                });
            }
            at += 2 + bytes;
        }
        // Extra data blocks, until the terminal block (size < 4).
        while let Some(size) = u32_at(d, at).filter(|&s| s >= 8) {
            match u32_at(d, at + 4)? as u32 {
                // TargetAnsi (260 bytes), then TargetUnicode.
                ENVIRONMENT_BLOCK if link.target.is_none() => {
                    link.target =
                        wide_at(d, at + 8 + 260, 260).filter(|s| !s.is_empty()).or_else(|| ansi_at(d, at + 8));
                }
                DARWIN_BLOCK => link.advertised = true,
                _ => {}
            }
            at += size;
        }
        Some(link)
    }

    /// The shortcut starts a program (or a batch file or console snap-in).
    pub fn runs_program(&self) -> bool {
        let program = |t: &String| {
            let ext = t.rsplit_once('.').map_or("", |(_, e)| e);
            !ext.contains('\\') && ["exe", "com", "bat", "cmd", "msc"].iter().any(|p| ext.eq_ignore_ascii_case(p))
        };
        self.advertised || self.target.as_ref().is_some_and(program)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A minimal shortcut: optional LinkInfo with a local path (Unicode offsets when `wide`),
    /// optional relative path string, optional extra block.
    pub fn build(local: Option<&str>, wide: bool, relative: Option<&str>, extra: Option<(u32, Vec<u8>)>) -> Vec<u8> {
        let mut flags = IS_UNICODE | HAS_ID_LIST;
        let mut d = MAGIC.to_vec();
        d.resize(HEADER_LEN, 0);
        // ID list: two empty bytes of content.
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&[0, 0]);
        if let Some(path) = local {
            flags |= HAS_LINK_INFO;
            let header: u32 = if wide { 0x24 } else { 0x1C };
            let mut strings = Vec::new();
            let (base_off, suffix_off);
            if wide {
                base_off = header as usize;
                strings.extend(path.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
                suffix_off = base_off + strings.len();
                strings.extend_from_slice(&[0, 0]);
            } else {
                base_off = header as usize;
                strings.extend(path.bytes().chain([0]));
                suffix_off = base_off + strings.len();
                strings.push(0);
            }
            let size = header as usize + strings.len();
            let mut info = Vec::new();
            for v in
                [size, header as usize, 1, 0, if wide { 0 } else { base_off }, 0, if wide { 0 } else { suffix_off }]
            {
                info.extend_from_slice(&(v as u32).to_le_bytes());
            }
            if wide {
                info.extend_from_slice(&(base_off as u32).to_le_bytes());
                info.extend_from_slice(&(suffix_off as u32).to_le_bytes());
            }
            info.extend(strings);
            d.extend(info);
        }
        if let Some(rel) = relative {
            flags |= HAS_RELATIVE_PATH;
            let units: Vec<u16> = rel.encode_utf16().collect();
            d.extend_from_slice(&(units.len() as u16).to_le_bytes());
            d.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        }
        if let Some((sig, body)) = extra {
            d.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
            d.extend_from_slice(&sig.to_le_bytes());
            d.extend(body);
        }
        d.extend_from_slice(&0u32.to_le_bytes());
        d[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
        d
    }

    #[test]
    fn parses_targets() {
        let p = |d: Vec<u8>| Link::parse(&d).unwrap();
        let l = p(build(Some(r"C:\Program Files\App\App.EXE"), false, None, None));
        assert_eq!(l.target.as_deref(), Some(r"C:\Program Files\App\App.EXE"));
        assert!(l.runs_program());
        let l = p(build(Some(r"C:\Users\Ü\Docs\notes.txt"), true, None, None));
        assert_eq!(l.target.as_deref(), Some(r"C:\Users\Ü\Docs\notes.txt"));
        assert!(!l.runs_program());
        // No link info: relative path, then the environment block.
        assert!(p(build(None, false, Some(r"..\App\app.exe"), None)).runs_program());
        let mut env = vec![0u8; 260];
        env.extend(r"%LOCALAPPDATA%\Discord\Update.exe".encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        env.resize(260 + 520, 0);
        let l = p(build(None, false, None, Some((ENVIRONMENT_BLOCK, env))));
        assert_eq!(l.target.as_deref(), Some(r"%LOCALAPPDATA%\Discord\Update.exe"));
        assert!(l.runs_program());
        // Installer shortcut without a path; a shortcut to a folder.
        assert!(p(build(None, false, None, Some((DARWIN_BLOCK, vec![0; 16])))).runs_program());
        assert!(!p(build(Some(r"C:\Games"), false, None, None)).runs_program());
        assert!(!p(build(Some(r"C:\Games.exe\Saves"), false, None, None)).runs_program());
        assert!(p(build(Some(r"C:\tools\start.cmd"), false, None, None)).runs_program());
        // Not a shortcut, or cut short.
        assert_eq!(Link::parse(b"hello"), None);
        let d = build(Some(r"C:\a.exe"), false, None, None);
        assert_eq!(Link::parse(&d[..HEADER_LEN + 6]), None);
    }
}
