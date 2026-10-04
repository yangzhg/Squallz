//! Bounded diagnostic text with classification evidence from the complete stream.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

use aho_corasick::{AhoCorasick, BuildError, MatchKind, StartKind};
use squallz_format_api::FormatError;

const PREFIX_BYTES: usize = 64 * 1024;
const MISSING_VOLUME_PREFIX: &[u8] = b"ERROR = Missing volume : ";
const PASSWORD_MARKERS: &[&str] = &[
    "wrong password",
    "incorrect password",
    "password is incorrect",
    "enter password",
    "password required",
    "password is required",
    "requires a password",
    "no password",
    "password was not supplied",
    "password was not provided",
    "password is not defined",
];
const UNSUPPORTED_MARKERS: &[&str] = &["unsupported", "not implemented"];

pub(super) struct DiagnosticCapture {
    matchers: &'static DiagnosticMatchers,
    capture_missing_volume: bool,
    prefix: Vec<u8>,
    marker_tail: Vec<u8>,
    password: bool,
    unsupported: bool,
    utf8_tail: [u8; 3],
    utf8_tail_len: usize,
    invalid_utf8: bool,
    line: MissingVolumeLine,
    missing_volume: Option<String>,
}

struct DiagnosticMatchers {
    password: AhoCorasick,
    unsupported: AhoCorasick,
}

#[derive(Default)]
pub(super) struct Diagnostics {
    pub(super) prefix: Vec<u8>,
    password: bool,
    unsupported: bool,
    missing_volume: Option<String>,
}

#[derive(Default)]
enum MissingVolumeLine {
    #[default]
    Leading,
    Prefix(usize),
    // Only a recognized missing-volume line may retain a complete file name.
    Name(String),
    Ignored,
}

impl DiagnosticCapture {
    pub(super) fn for_stdout() -> Result<Self, FormatError> {
        Self::new(true)
    }

    pub(super) fn for_stderr() -> Result<Self, FormatError> {
        Self::new(false)
    }

    fn new(capture_missing_volume: bool) -> Result<Self, FormatError> {
        static MATCHERS: OnceLock<Result<DiagnosticMatchers, BuildError>> = OnceLock::new();
        let matchers = MATCHERS
            .get_or_init(|| {
                let mut builder = AhoCorasick::builder();
                builder
                    .match_kind(MatchKind::Standard)
                    .start_kind(StartKind::Unanchored)
                    .ascii_case_insensitive(true);
                Ok(DiagnosticMatchers {
                    password: builder.build(PASSWORD_MARKERS)?,
                    unsupported: builder.build(UNSUPPORTED_MARKERS)?,
                })
            })
            .as_ref()
            .map_err(|error| {
                FormatError::Other(format!("Could not read 7-Zip diagnostics: {error}"))
            })?;
        Ok(Self {
            matchers,
            capture_missing_volume,
            prefix: Vec::new(),
            marker_tail: Vec::new(),
            password: false,
            unsupported: false,
            utf8_tail: [0; 3],
            utf8_tail_len: 0,
            invalid_utf8: false,
            line: MissingVolumeLine::Leading,
            missing_volume: None,
        })
    }

    pub(super) fn observe(&mut self, bytes: &[u8]) {
        let remaining = PREFIX_BYTES.saturating_sub(self.prefix.len());
        self.prefix
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        self.observe_markers(bytes);
        if self.capture_missing_volume {
            self.observe_utf8(bytes);
        }
    }

    pub(super) fn finish(mut self) -> Diagnostics {
        if self.utf8_tail_len != 0 {
            self.invalidate_utf8();
        }
        if !self.invalid_utf8 {
            self.finish_line();
        }
        Diagnostics {
            prefix: self.prefix,
            password: self.password,
            unsupported: self.unsupported,
            missing_volume: self.missing_volume,
        }
    }

    fn observe_markers(&mut self, bytes: &[u8]) {
        if self.password && self.unsupported {
            return;
        }
        let tail_len = self
            .matchers
            .password
            .max_pattern_len()
            .max(self.matchers.unsupported.max_pattern_len())
            .saturating_sub(1);
        // Keep the search window bounded even if a caller hands us a large slice.
        for chunk in bytes.chunks(4096) {
            self.marker_tail.extend_from_slice(chunk);
            if !self.password {
                self.password = self.matchers.password.is_match(&self.marker_tail);
            }
            if !self.unsupported {
                self.unsupported = self.matchers.unsupported.is_match(&self.marker_tail);
            }
            let discard = self.marker_tail.len().saturating_sub(tail_len);
            self.marker_tail.drain(..discard);
            if self.password && self.unsupported {
                return;
            }
        }
    }

    fn observe_utf8(&mut self, mut bytes: &[u8]) {
        if self.invalid_utf8 {
            return;
        }
        if self.utf8_tail_len != 0 {
            // Complete a split character before decoding borrowed stream slices.
            let mut character = [0u8; 4];
            character[..self.utf8_tail_len].copy_from_slice(&self.utf8_tail[..self.utf8_tail_len]);
            let width = match character[0] {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => {
                    self.invalidate_utf8();
                    return;
                }
            };
            let copied = bytes.len().min(width - self.utf8_tail_len);
            let length = self.utf8_tail_len + copied;
            character[self.utf8_tail_len..length].copy_from_slice(&bytes[..copied]);
            bytes = &bytes[copied..];
            match std::str::from_utf8(&character[..length]) {
                Ok(text) => {
                    self.utf8_tail_len = 0;
                    self.observe_text(text);
                }
                Err(error) if error.error_len().is_none() => {
                    self.utf8_tail[..length].copy_from_slice(&character[..length]);
                    self.utf8_tail_len = length;
                    return;
                }
                Err(_) => {
                    self.invalidate_utf8();
                    return;
                }
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => self.observe_text(text),
            Err(error) => {
                if let Ok(text) = std::str::from_utf8(&bytes[..error.valid_up_to()]) {
                    self.observe_text(text);
                }
                if error.error_len().is_some() {
                    self.invalidate_utf8();
                } else {
                    let tail = &bytes[error.valid_up_to()..];
                    self.utf8_tail[..tail.len()].copy_from_slice(tail);
                    self.utf8_tail_len = tail.len();
                }
            }
        }
    }

    fn invalidate_utf8(&mut self) {
        // A malformed byte anywhere invalidates an earlier missing-volume name.
        self.invalid_utf8 = true;
        self.utf8_tail_len = 0;
        self.line = MissingVolumeLine::Ignored;
        self.missing_volume = None;
    }

    fn observe_text(&mut self, text: &str) {
        for segment in text.split_inclusive('\n') {
            if self.missing_volume.is_some() {
                return;
            }
            if !matches!(self.line, MissingVolumeLine::Ignored) {
                for character in segment.trim_end_matches('\n').chars() {
                    match &mut self.line {
                        MissingVolumeLine::Leading if character.is_whitespace() => {}
                        MissingVolumeLine::Leading if character == 'E' => {
                            self.line = MissingVolumeLine::Prefix(1);
                        }
                        MissingVolumeLine::Prefix(matched) => {
                            if MISSING_VOLUME_PREFIX
                                .get(*matched)
                                .is_some_and(|byte| character == char::from(*byte))
                            {
                                *matched += 1;
                                if *matched == MISSING_VOLUME_PREFIX.len() {
                                    self.line = MissingVolumeLine::Name(String::new());
                                }
                            } else {
                                self.line = MissingVolumeLine::Ignored;
                            }
                        }
                        MissingVolumeLine::Name(name) => {
                            if matches!(character, '/' | '\\' | '\0') {
                                self.line = MissingVolumeLine::Ignored;
                            } else if !name.is_empty() || !character.is_whitespace() {
                                name.push(character);
                            }
                        }
                        MissingVolumeLine::Leading => self.line = MissingVolumeLine::Ignored,
                        MissingVolumeLine::Ignored => {}
                    }
                    if matches!(self.line, MissingVolumeLine::Ignored) {
                        break;
                    }
                }
            }
            if segment.ends_with('\n') {
                self.finish_line();
            }
        }
    }

    fn finish_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        if let MissingVolumeLine::Name(mut name) = line {
            name.truncate(name.trim_end().len());
            if safe_file_name(&name) {
                self.missing_volume = Some(name);
            }
        }
    }
}

impl Diagnostics {
    pub(super) fn utc_switch_unsupported(&self) -> bool {
        if self.prefix.len() >= PREFIX_BYTES {
            return false;
        }
        let Ok(message) = std::str::from_utf8(&self.prefix) else {
            return false;
        };
        let mut lines = message
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        lines.next() == Some("Command Line Error:")
            && lines.next() == Some("Unknown switch:")
            && lines.next() == Some("-slmu")
            && lines.next().is_none()
    }

    pub(super) fn has_missing_volume(&self) -> bool {
        self.missing_volume.is_some()
    }

    pub(super) fn password_failure(&self, password_supplied: bool) -> Option<FormatError> {
        self.password.then_some(if password_supplied {
            FormatError::WrongPassword
        } else {
            FormatError::PasswordRequired
        })
    }
}

pub(super) fn map_output_error(
    stderr: &Diagnostics,
    stdout: &Diagnostics,
    password_supplied: bool,
) -> FormatError {
    if let Some(name) = &stdout.missing_volume {
        return FormatError::missing_volume(name);
    }
    if stderr.unsupported {
        return FormatError::DependencyMissing("7zz/7z external format bridge".into());
    }
    if let Some(error) = stderr
        .password_failure(password_supplied)
        .or_else(|| stdout.password_failure(password_supplied))
    {
        return error;
    }
    let detail = String::from_utf8_lossy(&stderr.prefix).trim().to_owned();
    FormatError::CorruptArchive(if detail.is_empty() {
        "7-Zip could not read archive".into()
    } else {
        detail
    })
}

fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_switch_rejection_requires_complete_exact_diagnostics() {
        let capture = |bytes: &[u8]| {
            let mut diagnostic = DiagnosticCapture::for_stderr().unwrap();
            for byte in bytes.chunks(1) {
                diagnostic.observe(byte);
            }
            diagnostic.finish()
        };
        for message in [
            b"Command Line Error:\nUnknown switch:\n-slmu".as_slice(),
            b"\r\n Command Line Error:\r\n\r\n Unknown switch:\r\n -slmu\r\n".as_slice(),
        ] {
            assert!(capture(message).utc_switch_unsupported());
        }
        for message in [
            b"".as_slice(),
            b"Unknown switch:\n-slmu".as_slice(),
            b"Command Line Error:\nUnsupported switch:\n-slmu".as_slice(),
            b"Command Line Error:\nUnknown switch:\n-slm".as_slice(),
            b"Command Line Error:\nUnknown switch:\n-slmux".as_slice(),
            b"Command Line Error:\nUnknown switch:\n-slmu private.zip".as_slice(),
            b"Command Line Error:\nUnknown switch:\n-slmu\nData Error".as_slice(),
            b"Wrong password\nCommand Line Error:\nUnknown switch:\n-slmu".as_slice(),
            b"Command Line Error:\nUnknown switch:\n-slmu\n\xff".as_slice(),
        ] {
            assert!(!capture(message).utc_switch_unsupported(), "{message:?}");
        }
        let mut truncated = b"Command Line Error:\nUnknown switch:\n-slmu\n".to_vec();
        truncated.resize(PREFIX_BYTES, b' ');
        truncated.extend_from_slice(b"Data Error");
        assert!(!capture(&truncated).utc_switch_unsupported());
    }

    #[test]
    fn sevenzip_missing_volume_diagnostic_accepts_only_one_file_name() {
        fn capture(bytes: &[u8]) -> Diagnostics {
            let mut capture = DiagnosticCapture::for_stdout().unwrap();
            for chunk in bytes.chunks(1) {
                capture.observe(chunk);
            }
            capture.finish()
        }

        let stdout = capture(
            b"\nPath = /private/stage/archive.part1.rar\nERROR = Missing volume : archive.part3.rar\n",
        );
        assert_eq!(stdout.missing_volume.as_deref(), Some("archive.part3.rar"));
        assert!(matches!(
            map_output_error(&capture(b""), &stdout, false),
            FormatError::CorruptArchive(detail)
                if detail == "missing volume: archive.part3.rar"
        ));
        for name in [
            "../secret.rar",
            "child/secret.rar",
            "child\\secret.rar",
            "\0secret.rar",
            "",
            ".",
            "..",
        ] {
            let diagnostic = format!("ERROR = Missing volume : {name}\n");
            assert_eq!(capture(diagnostic.as_bytes()).missing_volume, None);
        }
        for name in ["-part3.rar", "part 3.rar", "分卷😀.rar"] {
            let diagnostic = format!("\u{2003}ERROR = Missing volume : \u{2002}{name}\u{a0}\r\n");
            assert_eq!(
                capture(diagnostic.as_bytes()).missing_volume.as_deref(),
                Some(name),
            );
        }
        assert_eq!(
            capture(b"ERROR = Missing volume : first.rar\nERROR = Missing volume : second.rar")
                .missing_volume
                .as_deref(),
            Some("first.rar"),
        );

        let padding = vec![b'x'; PREFIX_BYTES + 11];
        let mut stderr = DiagnosticCapture::for_stderr().unwrap();
        stderr.observe(b"ERROR = Missing volume : ");
        stderr.observe(&padding);
        assert!(matches!(stderr.line, MissingVolumeLine::Leading));
        assert!(stderr.finish().missing_volume.is_none());

        let mut late = DiagnosticCapture::for_stdout().unwrap();
        late.observe(&padding);
        assert!(matches!(late.line, MissingVolumeLine::Ignored));
        assert!(late.marker_tail.len() < b"password was not provided".len());
        late.observe(b"\n");
        for chunk in "ERROR = Missing volume : 分卷😀.rar".as_bytes().chunks(1) {
            late.observe(chunk);
        }
        let late = late.finish();
        assert_eq!(late.prefix, padding[..PREFIX_BYTES]);
        assert_eq!(late.missing_volume.as_deref(), Some("分卷😀.rar"));
        for invalid_tail in [
            b"\xff".as_slice(),
            b"\xf0\x9f".as_slice(),
            b"\xe0\x80\x80".as_slice(),
        ] {
            let mut invalid = DiagnosticCapture::for_stdout().unwrap();
            invalid.observe(b"ERROR = Missing volume : first.rar\n");
            invalid.observe(invalid_tail);
            assert_eq!(invalid.finish().missing_volume, None);
        }

        assert!(matches!(
            map_output_error(
                &capture(b"Cannot read password-notes.txt"),
                &capture(b""),
                false,
            ),
            FormatError::CorruptArchive(_),
        ));
        for marker in PASSWORD_MARKERS {
            let mut diagnostic = DiagnosticCapture::for_stdout().unwrap();
            diagnostic.observe(&padding);
            for byte in marker.bytes().map(|byte| byte.to_ascii_uppercase()) {
                diagnostic.observe(&[byte]);
            }
            let diagnostic = diagnostic.finish();
            assert_eq!(diagnostic.prefix.len(), PREFIX_BYTES);
            assert!(matches!(
                diagnostic.password_failure(true),
                Some(FormatError::WrongPassword),
            ));
            assert!(matches!(
                map_output_error(&capture(b""), &diagnostic, false),
                FormatError::PasswordRequired,
            ));
        }
        for marker in UNSUPPORTED_MARKERS {
            let mut diagnostic = DiagnosticCapture::for_stdout().unwrap();
            diagnostic.observe(&padding);
            for byte in marker.bytes().map(|byte| byte.to_ascii_uppercase()) {
                diagnostic.observe(&[byte]);
            }
            assert!(matches!(
                map_output_error(&diagnostic.finish(), &capture(b"Wrong password"), true),
                FormatError::DependencyMissing(_),
            ));
        }
        assert!(matches!(
            map_output_error(&capture(b"Wrong password"), &capture(b"unsupported"), true),
            FormatError::WrongPassword,
        ));
        assert!(matches!(
            map_output_error(&capture(b"unsupported; Wrong password"), &stdout, true),
            FormatError::CorruptArchive(detail) if detail == "missing volume: archive.part3.rar",
        ));
        assert!(matches!(
            map_output_error(&capture(b""), &capture(b"unsupported"), false),
            FormatError::CorruptArchive(detail) if detail == "7-Zip could not read archive",
        ));
    }
}
