//! Legacy entry-name encoding handling.
//!
//! ZIP entry names are raw bytes; modern tools mark them UTF-8 via the
//! language-encoding flag, but archives produced by legacy Windows/Japanese
//! tools carry CP936/Shift-JIS/Big5 bytes with no marker. Policy: a name
//! that is valid UTF-8 is taken as UTF-8; everything else is decoded with
//! the user override, or with an encoding guessed across *all* non-UTF-8
//! names of the archive (chardetng).

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use squallz_format_api::EntryPath;

/// Resolves the archive-wide fallback encoding for non-UTF-8 entry names.
///
/// `override_label` wins when it names a known encoding (e.g. `"gbk"`,
/// `"shift_jis"`); otherwise the encoding is guessed from the concatenated
/// non-UTF-8 name bytes. Returns `None` when every name is valid UTF-8 and
/// no detection is needed.
pub(super) fn resolve_fallback_encoding(
    raw_names: &[Vec<u8>],
    override_label: Option<&str>,
) -> Option<&'static Encoding> {
    if let Some(encoding) = override_encoding(override_label) {
        return Some(encoding);
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    let mut fed_any = false;
    for name in raw_names {
        if std::str::from_utf8(name).is_err() {
            detector.feed(name, false);
            fed_any = true;
        }
    }
    if !fed_any {
        return None;
    }
    detector.feed(b"", true);
    Some(detector.guess(None, Utf8Detection::Deny))
}

fn override_encoding(override_label: Option<&str>) -> Option<&'static Encoding> {
    match override_label {
        Some(label) => Encoding::for_label(label.as_bytes()),
        None => None,
    }
}

/// Decodes one raw entry name into an [`EntryPath`]: valid UTF-8 stays
/// UTF-8, anything else goes through the fallback encoding (lossy as a last
/// resort so listing never fails on hostile bytes).
pub(super) fn decode_entry_name(raw: &[u8], fallback: Option<&'static Encoding>) -> EntryPath {
    if let Ok(s) = std::str::from_utf8(raw) {
        return EntryPath::from_raw(raw.to_vec(), s.to_string(), "utf-8");
    }
    match fallback {
        Some(enc) => {
            let (decoded, _, _) = enc.decode(raw);
            EntryPath::from_raw(raw.to_vec(), decoded.into_owned(), enc.name())
        }
        None => EntryPath::from_raw(
            raw.to_vec(),
            String::from_utf8_lossy(raw).into_owned(),
            "utf-8",
        ),
    }
}

#[cfg(test)]
mod tests {
    use encoding_rs::GBK;

    use super::*;

    const CHINESE_NAME: &str = "压缩文件中文名称测试.txt";

    fn gbk_name() -> Vec<u8> {
        let (encoded, _, had_errors) = GBK.encode(CHINESE_NAME);
        assert!(!had_errors);
        encoded.into_owned()
    }

    #[test]
    fn override_encoding_accepts_known_labels_only() {
        assert_eq!(
            override_encoding(Some("gbk")).map(Encoding::name),
            Some("GBK")
        );
        assert_eq!(
            override_encoding(Some("shift_jis")).map(Encoding::name),
            Some("Shift_JIS")
        );
        assert_eq!(override_encoding(Some("not-an-encoding")), None);
        assert_eq!(override_encoding(None), None);
    }

    #[test]
    fn resolve_fallback_uses_manual_override_before_detection() {
        let raw_names = vec![gbk_name()];
        assert_eq!(
            resolve_fallback_encoding(&raw_names, Some("shift_jis")).map(Encoding::name),
            Some("Shift_JIS")
        );
    }

    #[test]
    fn resolve_fallback_ignores_invalid_override_when_names_are_utf8() {
        let raw_names = vec![b"plain.txt".to_vec(), "目录/文件.txt".as_bytes().to_vec()];
        assert_eq!(
            resolve_fallback_encoding(&raw_names, Some("not-an-encoding")),
            None
        );
    }

    #[test]
    fn decode_prefers_valid_utf8_even_when_fallback_is_present() {
        let path = decode_entry_name("目录/文件.txt".as_bytes(), Some(GBK));
        assert_eq!(path.display, "目录/文件.txt");
        assert_eq!(path.encoding, "utf-8");
        assert_eq!(path.raw, "目录/文件.txt".as_bytes());
    }

    #[test]
    fn decode_non_utf8_uses_fallback_or_lossy_utf8() {
        let raw = gbk_name();
        let decoded = decode_entry_name(&raw, Some(GBK));
        assert_eq!(decoded.display, CHINESE_NAME);
        assert_eq!(decoded.encoding, GBK.name());
        assert_eq!(decoded.raw, raw);

        let hostile = vec![0xff, b'.', b't', b'x', b't'];
        let decoded = decode_entry_name(&hostile, None);
        assert!(decoded.display.contains('\u{fffd}'));
        assert_eq!(decoded.encoding, "utf-8");
        assert_eq!(decoded.raw, hostile);
    }
}
