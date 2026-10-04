//! Integrity tests use the same resource limits across native formats.

mod common;

use std::fs;

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, FormatError, NoProgress, OpenOptions, SafetyLimits,
    SqzCreateOptions, SqzInnerFormat,
};
use squallz_core::CreateCommitPolicy;

#[test]
fn integrity_test_limits_stop_each_format_and_allow_an_explicit_retry() {
    let dir = TempDir::new("integrity-test-limits");
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("first.bin"), vec![17; 96 * 1024]).unwrap();
    fs::write(tree.join("second.bin"), vec![23; 96 * 1024]).unwrap();
    let engine = engine();
    let control = ControlToken::default();
    for (name, inner_format) in [
        ("payload.zip", SqzInnerFormat::Sqz),
        ("payload.7z", SqzInnerFormat::Sqz),
        ("payload.tar", SqzInnerFormat::Sqz),
        ("payload.tar.gz", SqzInnerFormat::Sqz),
        ("payload.sqz", SqzInnerFormat::Sqz),
        ("zip.sqz", SqzInnerFormat::Zip),
        ("sevenzip.sqz", SqzInnerFormat::SevenZip),
        ("tar.sqz", SqzInnerFormat::Tar),
        ("zstd.sqz", SqzInnerFormat::Zstd),
    ] {
        let archive = dir.path().join(name);
        engine
            .create(
                &archive,
                std::slice::from_ref(&tree),
                &CreateOptions {
                    sqz: SqzCreateOptions {
                        inner_format,
                        ..SqzCreateOptions::default()
                    },
                    ..CreateOptions::default()
                },
                CreateCommitPolicy::ReplaceExisting,
                &NoProgress,
                &control,
            )
            .unwrap();
        let original = fs::read(&archive).unwrap();
        for limits in [
            SafetyLimits {
                max_output_bytes: 120 * 1024,
                ..SafetyLimits::default()
            },
            SafetyLimits {
                max_entries: 1,
                ..SafetyLimits::default()
            },
        ] {
            let result = engine.test_summary(
                &archive,
                &OpenOptions::default(),
                &limits,
                &NoProgress,
                &control,
            );
            assert!(
                matches!(result, Err(FormatError::ResourceLimitExceeded(_))),
                "{name}: {result:?}"
            );
            assert_eq!(fs::read(&archive).unwrap(), original, "{name}");
        }
        let report = engine
            .test_summary(
                &archive,
                &OpenOptions::default(),
                &SafetyLimits {
                    max_output_bytes: 192 * 1024,
                    max_entries: 3,
                    ..SafetyLimits::default()
                },
                &NoProgress,
                &control,
            )
            .unwrap();
        assert!(report.is_ok(), "{name}: {report:?}");
        assert_eq!(fs::read(&archive).unwrap(), original, "{name}");
    }
}

#[test]
fn single_compressed_stream_tests_charge_decoded_bytes() {
    let dir = TempDir::new("single-stream-test-limits");
    let file = dir.path().join("payload.bin");
    fs::write(&file, vec![42; 128 * 1024]).unwrap();
    let engine = engine();
    let control = ControlToken::default();
    for suffix in ["gz", "xz", "zst"] {
        let archive = dir.path().join(format!("payload.bin.{suffix}"));
        engine
            .create(
                &archive,
                std::slice::from_ref(&file),
                &CreateOptions::default(),
                CreateCommitPolicy::ReplaceExisting,
                &NoProgress,
                &control,
            )
            .unwrap();
        let result = engine.test_summary(
            &archive,
            &OpenOptions::default(),
            &SafetyLimits {
                max_output_bytes: 64 * 1024,
                ..SafetyLimits::default()
            },
            &NoProgress,
            &control,
        );
        assert!(
            matches!(result, Err(FormatError::ResourceLimitExceeded(_))),
            "{suffix}: {result:?}"
        );
        let report = engine
            .test_summary(
                &archive,
                &OpenOptions::default(),
                &SafetyLimits {
                    max_output_bytes: 128 * 1024,
                    ..SafetyLimits::default()
                },
                &NoProgress,
                &control,
            )
            .unwrap();
        assert!(report.is_ok(), "{suffix}: {report:?}");
        assert_eq!(report.entries_tested, 1);
    }
}
