//! Case folding that preserves byte length, so folded names share offsets with the originals.

/// Append the lowercased form of `src` to `out`; exactly `src.len()` bytes are written.
/// Characters whose lowercase form has a different UTF-8 length are kept as-is.
pub fn fold_into(src: &[u8], out: &mut Vec<u8>) {
    if src.is_ascii() {
        out.extend(src.iter().map(u8::to_ascii_lowercase));
        return;
    }
    let Ok(s) = std::str::from_utf8(src) else {
        out.extend(src.iter().map(u8::to_ascii_lowercase));
        return;
    };
    let mut buf = [0u8; 4];
    for c in s.chars() {
        let mut lower = c.to_lowercase();
        let l = match (lower.next(), lower.next()) {
            (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
            _ => c,
        };
        out.extend_from_slice(l.encode_utf8(&mut buf).as_bytes());
    }
}

pub fn fold(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    fold_into(src, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn folds_and_keeps_length() {
        assert_eq!(fold(b"ReadMe.TXT"), b"readme.txt");
        assert_eq!(fold("ÄÖÜ Straße".as_bytes()), "äöü straße".as_bytes());
        // 'İ' lowercases to two chars; kept as-is.
        assert_eq!(fold("İx".as_bytes()), "İx".as_bytes());
        // Invalid UTF-8 falls back to ASCII folding.
        assert_eq!(fold(b"A\xffB"), b"a\xffb");
        for s in ["ẞ", "Ω", "Ⱥ", "ſ"] {
            assert_eq!(fold(s.as_bytes()).len(), s.len());
        }
    }
}
