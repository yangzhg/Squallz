#![forbid(unsafe_code)]
//! squallz-formats: built-in format implementations and registration.
//!
//! Adding a format = add one module + register it in [`registry`];
//! core/cli/gui stay untouched. `unsafe` is forbidden in this crate; FFI
//! backends (libarchive/unrar) must live in dedicated `*-sys` wrapper
//! crates.

use std::sync::Arc;

use squallz_format_api::FormatRegistry;

#[cfg(all(test, feature = "process-backend"))]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "process-backend")]
mod external_process;
#[cfg(feature = "process-backend")]
mod rar;
mod sevenz;
#[cfg(feature = "process-backend")]
mod sevenzip_bridge;
mod sqz;
#[cfg_attr(not(feature = "process-backend"), allow(dead_code))]
mod stable_source;
mod stream;
mod tar;
mod zip;

#[cfg(feature = "process-backend")]
pub use rar::{unrar_backend_status, UnrarBackendSource, UnrarBackendStatus};
#[cfg(feature = "process-backend")]
pub use sevenzip_bridge::{
    sevenzip_backend_status, wimlib_backend_status, SevenZipBackendSource, SevenZipBackendStatus,
    WimlibBackendSource, WimlibBackendStatus,
};

/// Builds the registry containing every built-in format.
pub fn registry() -> FormatRegistry {
    let mut reg = FormatRegistry::new();
    reg.register_archive(Arc::new(zip::ZipFormat));
    reg.register_archive(Arc::new(tar::TarFormat));
    reg.register_archive(Arc::new(sevenz::SevenZFormat));
    #[cfg(feature = "process-backend")]
    reg.register_archive(Arc::new(rar::RarFormat));
    #[cfg(feature = "process-backend")]
    for format in sevenzip_bridge::formats() {
        reg.register_archive(Arc::new(format));
    }
    reg.register_archive(Arc::new(sqz::SqzFormat));
    reg.register_compressor(Arc::new(stream::Gzip));
    reg.register_compressor(Arc::new(stream::Bzip2));
    reg.register_compressor(Arc::new(stream::Xz));
    reg.register_compressor(Arc::new(stream::Zstd));
    reg.register_compressor(Arc::new(stream::Lz4));
    reg.register_compressor(Arc::new(stream::Brotli));
    // Compound-format shorthand extensions.
    reg.register_alias("tgz", "tar.gz");
    reg.register_alias("tbz2", "tar.bz2");
    reg.register_alias("txz", "tar.xz");
    reg.register_alias("tzst", "tar.zst");
    reg
}

/// Builds the self-contained registry used inside constrained preview hosts.
///
/// Finder Quick Look extensions cannot rely on launching a sibling process.
/// This registry therefore includes only implementations that decode in the
/// current process. RAR and the long-tail 7-Zip bridge remain available from
/// [`registry`] in the main application and CLI.
pub fn embedded_preview_registry() -> FormatRegistry {
    let mut reg = FormatRegistry::new();
    reg.register_archive(Arc::new(zip::ZipFormat));
    reg.register_archive(Arc::new(tar::TarFormat));
    reg.register_archive(Arc::new(sevenz::SevenZFormat));
    reg.register_archive(Arc::new(sqz::SqzFormat));
    reg.register_compressor(Arc::new(stream::Gzip));
    reg.register_compressor(Arc::new(stream::Bzip2));
    reg.register_compressor(Arc::new(stream::Xz));
    reg.register_compressor(Arc::new(stream::Zstd));
    reg.register_compressor(Arc::new(stream::Lz4));
    reg.register_compressor(Arc::new(stream::Brotli));
    reg.register_alias("tgz", "tar.gz");
    reg.register_alias("tbz2", "tar.bz2");
    reg.register_alias("txz", "tar.xz");
    reg.register_alias("tzst", "tar.zst");
    reg
}

#[cfg(test)]
mod tests {
    use squallz_format_api::{
        ControlToken, Detected, ExtractOptions, FormatError, FormatInfo, FormatKind, NoProgress,
        OpenOptions,
    };

    fn format_info<'a>(formats: &'a [FormatInfo], id: &str) -> &'a FormatInfo {
        formats
            .iter()
            .find(|format| format.id == id)
            .unwrap_or_else(|| panic!("{id} registered"))
    }

    fn assert_archive(
        format: &FormatInfo,
        extensions: &[&str],
        can_create: bool,
        can_encrypt_data: bool,
        can_encrypt_names: bool,
        can_split: bool,
        can_update: bool,
    ) {
        assert_eq!(format.kind, FormatKind::Archive);
        assert_eq!(format.extensions.as_slice(), extensions);
        assert_eq!(format.capabilities.can_create, can_create);
        assert!(format.capabilities.can_extract);
        assert_eq!(format.capabilities.can_encrypt_data, can_encrypt_data);
        assert_eq!(format.capabilities.can_encrypt_names, can_encrypt_names);
        assert_eq!(format.capabilities.can_split, can_split);
        assert_eq!(format.capabilities.can_update, can_update);
        assert!(format.capabilities.can_test);
    }

    fn assert_detected_archive(detected: Option<Detected>, id: &str) {
        assert!(
            matches!(detected, Some(Detected::Archive(format)) if format.id() == id),
            "expected archive detection for {id}"
        );
    }

    fn assert_detected_compressed(
        detected: Option<Detected>,
        compressor_id: &str,
        inner_archive_id: Option<&str>,
    ) {
        match (detected, inner_archive_id) {
            (
                Some(Detected::Compressed {
                    compressor,
                    inner_archive: Some(inner_archive),
                }),
                Some(expected_inner),
            ) => {
                assert_eq!(compressor.id(), compressor_id);
                assert_eq!(inner_archive.id(), expected_inner);
            }
            (
                Some(Detected::Compressed {
                    compressor,
                    inner_archive: None,
                }),
                None,
            ) => {
                assert_eq!(compressor.id(), compressor_id);
            }
            _ => panic!("expected compressed detection for {compressor_id}"),
        }
    }

    #[test]
    fn embedded_preview_registry_never_advertises_process_backed_formats() {
        let formats = super::embedded_preview_registry().formats();
        let ids: Vec<&str> = formats.iter().map(|format| format.id).collect();

        assert!(ids.contains(&"zip"));
        assert!(ids.contains(&"tar"));
        assert!(ids.contains(&"7z"));
        assert!(ids.contains(&"sqz"));
        assert!(ids.contains(&"gzip"));
        assert!(!ids.contains(&"rar"));
        assert!(!ids.contains(&"wim"));
        assert!(!ids.contains(&"iso"));
    }

    fn missing_volume<T>(result: Result<T, FormatError>) -> std::path::PathBuf {
        match result {
            Err(error) => error
                .missing_volume_path()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| panic!("expected missing-volume error, got {error:?}")),
            Ok(_) => panic!("expected missing-volume error"),
        }
    }

    fn split_wim_header() -> Vec<u8> {
        let mut header = vec![0u8; 208];
        header[..8].copy_from_slice(b"MSWIM\0\0\0");
        header[8..12].copy_from_slice(&208u32.to_le_bytes());
        header[12..16].copy_from_slice(&0x0001_0d00u32.to_le_bytes());
        header[16..20].copy_from_slice(&0x0000_0008u32.to_le_bytes());
        header[20..24].copy_from_slice(&(32 * 1024u32).to_le_bytes());
        header[24..40].copy_from_slice(&[0x5a; 16]);
        header[40..42].copy_from_slice(&1u16.to_le_bytes());
        header[42..44].copy_from_slice(&2u16.to_le_bytes());
        header
    }

    /// A matching name and signature preserve compound streams before other
    /// magic matches; unverified names remain the final fallback.
    #[test]
    fn compressor_sniff_detects_extensionless_streams() {
        let reg = super::registry();
        let cases: [(&str, &[u8]); 5] = [
            ("gzip", &[0x1F, 0x8B, 0x08, 0x00]),
            ("xz", &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]),
            ("zstd", &[0x28, 0xB5, 0x2F, 0xFD]),
            ("bzip2", b"BZh9\x31\x41\x59\x26"),
            ("lz4", &[0x04, 0x22, 0x4D, 0x18]),
        ];
        for (id, head) in cases {
            match reg.detect(Some("blob.bin"), head, &[]) {
                Some(Detected::Compressed {
                    compressor,
                    inner_archive: None,
                }) => assert_eq!(compressor.id(), id),
                _ => panic!("{id}: magic bytes not detected"),
            }
        }
        // brotli has no reliable magic: stays undetected without extension.
        assert!(reg
            .detect(Some("blob.bin"), b"\x0B\x02\x80brotli?", &[])
            .is_none());
    }

    #[test]
    fn verified_compound_name_wins_over_an_unrelated_tail_signature() {
        let reg = super::registry();
        assert_detected_compressed(
            reg.detect(
                Some("backup.tar.gz"),
                &[0x1F, 0x8B, 0x08, 0x00],
                b"compressed payload containing PK\x05\x06 bytes",
            ),
            "gzip",
            Some("tar"),
        );
    }

    /// Generic `.001` and native ZIP `.z01` volume names detect under their
    /// logical archive format.
    #[test]
    fn volume_suffix_detection_by_name() {
        let reg = super::registry();
        assert!(matches!(
            reg.detect_by_name("backup.zip.001"),
            Some(Detected::Archive(f)) if f.id() == "zip"
        ));
        assert!(matches!(
            reg.detect_by_name("backup.tar.gz.017"),
            Some(Detected::Compressed { inner_archive: Some(a), .. }) if a.id() == "tar"
        ));
        assert_detected_archive(reg.detect_by_name("backup.z01"), "zip");
        assert_detected_archive(
            reg.detect(Some("backup.z02"), b"middle volume bytes", b""),
            "zip",
        );
        assert_eq!(reg.display_stem("backup.tar.gz.017"), "backup");
        assert_eq!(reg.display_stem("notes.tgz"), "notes");
        assert_eq!(reg.display_stem("x.zip.001"), "x");
        assert_eq!(reg.display_stem("x.z01"), "x");
    }

    #[test]
    fn registry_declares_core_archive_and_longtail_boundaries() {
        let reg = super::registry();
        let formats = reg.formats();

        assert_archive(
            format_info(&formats, "zip"),
            &["zip", "jar", "apk", "cbz", "ipa"],
            true,
            true,
            false,
            true,
            true,
        );
        assert_archive(
            format_info(&formats, "tar"),
            &["tar"],
            true,
            false,
            false,
            true,
            false,
        );
        assert_archive(
            format_info(&formats, "7z"),
            &["7z"],
            true,
            true,
            true,
            true,
            false,
        );
        assert_archive(
            format_info(&formats, "sqz"),
            &["sqz"],
            true,
            false,
            false,
            true,
            false,
        );
        assert_archive(
            format_info(&formats, "rar"),
            &["rar", "cbr"],
            false,
            false,
            false,
            false,
            false,
        );
        assert_archive(
            format_info(&formats, "wim"),
            &["wim", "swm", "esd"],
            true,
            false,
            false,
            true,
            false,
        );

        for (id, extensions) in [
            ("apfs", &["apfs"][..]),
            ("cab", &["cab"][..]),
            ("iso", &["iso"][..]),
            ("vhdx", &["vhdx"][..]),
            ("z", &["z", "taz"][..]),
        ] {
            assert_archive(
                format_info(&formats, id),
                extensions,
                false,
                false,
                false,
                false,
                false,
            );
        }
    }

    #[test]
    fn split_wim_reports_the_same_missing_member_from_every_reader_entry_point() {
        let root = std::env::temp_dir().join(format!(
            "squallz-split-wim-missing-member-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("install.swm");
        let destination = root.join("output");
        std::fs::write(&archive, split_wim_header()).unwrap();

        let engine = squallz_core::Engine::new(super::registry());
        let open_options = OpenOptions::default();
        let control = ControlToken::new();
        let missing = root.join("install2.swm");
        let paths = [
            missing_volume(engine.open(&archive, &open_options)),
            missing_volume(engine.list(&archive, &open_options)),
            missing_volume(engine.test(&archive, &open_options, &NoProgress, &control)),
            missing_volume(engine.extract(
                &archive,
                &destination,
                None,
                &open_options,
                &ExtractOptions::default(),
                &NoProgress,
                &control,
            )),
        ];

        assert!(paths.iter().all(|path| path == &missing));
        assert!(!destination.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_declares_single_stream_compressors() {
        let reg = super::registry();
        let formats = reg.formats();

        for (id, extensions) in [
            ("gzip", &["gz"][..]),
            ("bzip2", &["bz2"][..]),
            ("xz", &["xz"][..]),
            ("zstd", &["zst"][..]),
            ("lz4", &["lz4"][..]),
            ("brotli", &["br"][..]),
        ] {
            let format = format_info(&formats, id);
            assert_eq!(format.kind, FormatKind::Compressor);
            assert_eq!(format.extensions.as_slice(), extensions);
            assert!(format.capabilities.can_create);
            assert!(format.capabilities.can_extract);
            assert!(!format.capabilities.can_encrypt_data);
            assert!(!format.capabilities.can_encrypt_names);
            assert!(format.capabilities.can_split);
            assert!(!format.capabilities.can_update);
            assert!(format.capabilities.can_test);
        }
    }

    #[test]
    fn registry_expands_compound_aliases_and_compressor_suffixes() {
        let reg = super::registry();

        for (name, compressor_id) in [
            ("backup.tgz", "gzip"),
            ("backup.tbz2", "bzip2"),
            ("backup.txz", "xz"),
            ("backup.tzst", "zstd"),
            ("backup.tar.br", "brotli"),
            ("backup.tar.lz4", "lz4"),
        ] {
            assert_detected_compressed(reg.detect_by_name(name), compressor_id, Some("tar"));
            assert_eq!(reg.display_stem(name), "backup");
        }

        assert_detected_compressed(reg.detect_by_name("payload.gz"), "gzip", None);
        assert_eq!(reg.display_stem("payload.gz"), "payload");
        assert_detected_archive(reg.detect_by_name("comic.cbr"), "rar");
        assert_detected_archive(reg.detect_by_name("package.deb"), "ar");
    }

    #[test]
    fn registry_detects_core_archive_magic_without_extension() {
        let reg = super::registry();

        assert_detected_archive(reg.detect(None, b"PK\x03\x04rest", b""), "zip");
        assert_detected_archive(
            reg.detect(None, &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00], b""),
            "7z",
        );
        assert_detected_archive(reg.detect(None, b"Rar!\x1A\x07\x00rest", b""), "rar");
        assert_detected_archive(reg.detect(None, b"MSWIM\0\0\0rest", b""), "wim");
        assert_detected_archive(reg.detect(None, b"MSCFrest", b""), "cab");
        assert_detected_compressed(
            reg.detect(None, &[0x04, 0x22, 0x4D, 0x18, 0x40], b""),
            "lz4",
            None,
        );
    }

    #[test]
    fn registry_contains_zip() {
        let reg = super::registry();
        let formats = reg.formats();
        assert!(!formats.is_empty());
        let zip = formats
            .iter()
            .find(|f| f.id == "zip")
            .expect("zip registered");
        assert_eq!(zip.kind, FormatKind::Archive);
        assert!(zip.capabilities.can_create);
        assert!(zip.capabilities.can_extract);
        assert!(zip.capabilities.can_encrypt_data);
        assert!(!zip.capabilities.can_encrypt_names);
        let sqz = formats
            .iter()
            .find(|f| f.id == "sqz")
            .expect("sqz registered");
        assert_eq!(sqz.kind, FormatKind::Archive);
        assert_eq!(sqz.extensions, vec!["sqz"]);
        assert!(sqz.capabilities.can_create);
        assert!(sqz.capabilities.can_extract);
        assert!(sqz.capabilities.can_test);
        assert!(sqz.capabilities.can_split);
        let rar = formats
            .iter()
            .find(|f| f.id == "rar")
            .expect("rar registered");
        assert_eq!(rar.kind, FormatKind::Archive);
        assert_eq!(rar.extensions, vec!["rar", "cbr"]);
        assert!(!rar.capabilities.can_create);
        assert!(rar.capabilities.can_extract);
        let cab = formats
            .iter()
            .find(|f| f.id == "cab")
            .expect("cab registered");
        assert_eq!(cab.kind, FormatKind::Archive);
        assert_eq!(cab.extensions, vec!["cab"]);
        assert!(!cab.capabilities.can_create);
        assert!(cab.capabilities.can_extract);
        assert!(cab.capabilities.can_test);
        let wim = formats
            .iter()
            .find(|f| f.id == "wim")
            .expect("wim registered");
        assert_eq!(wim.kind, FormatKind::Archive);
        assert!(wim.capabilities.can_create);
        assert!(wim.capabilities.can_extract);
        for id in ["apfs", "iso", "vhdx", "z"] {
            let format = formats
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{id} registered"));
            assert_eq!(format.kind, FormatKind::Archive);
            assert!(!format.capabilities.can_create);
            assert!(format.capabilities.can_extract);
        }
    }
}
