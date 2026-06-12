//! Roundtrip, guardrail and progress tests for every single-stream
//! compressor (gzip/bzip2/xz/zstd/lz4/brotli). All fixtures are generated
//! in code.

mod common;

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use squallz_core::api::{
    CompressionLevel, Compressor, ControlToken, Detected, EntryPath, FormatError, NoProgress,
    ProgressSink, ResourceOptions, SafetyLimits,
};

const ALL_EXTENSIONS: [&str; 6] = ["gz", "bz2", "xz", "zst", "lz4", "br"];

/// Looks a compressor up through the registry (the same path the engine
/// takes), so registration is covered too.
fn compressor(ext: &str) -> Arc<dyn Compressor> {
    match squallz_formats::registry().detect_by_name(&format!("file.{ext}")) {
        Some(Detected::Compressed {
            compressor,
            inner_archive: None,
        }) => compressor,
        _ => panic!("extension {ext} did not resolve to a plain compressor"),
    }
}

/// Compressible test payload larger than several 64 KiB chunks.
fn payload() -> Vec<u8> {
    let mut data = Vec::with_capacity(300 * 1024);
    for i in 0..30_000u32 {
        data.extend_from_slice(format!("line {i} of the squallz fixture\n").as_bytes());
    }
    data
}

/// Counts progress callbacks (verifies chunk-level reporting).
#[derive(Default)]
struct CountingSink(AtomicU64);

impl ProgressSink for CountingSink {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn compress(c: &dyn Compressor, data: &[u8], level: CompressionLevel) -> Vec<u8> {
    let ctl = ControlToken::new();
    let mut src = Cursor::new(data.to_vec());
    let mut dst = Cursor::new(Vec::new());
    c.compress(
        &mut src,
        &mut dst,
        level,
        &ResourceOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    dst.into_inner()
}

fn compress_with_resources(
    c: &dyn Compressor,
    data: &[u8],
    level: CompressionLevel,
    resources: &ResourceOptions,
) -> Vec<u8> {
    let ctl = ControlToken::new();
    let mut src = Cursor::new(data.to_vec());
    let mut dst = Cursor::new(Vec::new());
    c.compress(&mut src, &mut dst, level, resources, &NoProgress, &ctl)
        .unwrap();
    dst.into_inner()
}

fn decompress(
    c: &dyn Compressor,
    data: &[u8],
    limits: &SafetyLimits,
) -> Result<Vec<u8>, FormatError> {
    let ctl = ControlToken::new();
    let mut src = Cursor::new(data.to_vec());
    let mut dst = Cursor::new(Vec::new());
    c.decompress(&mut src, &mut dst, limits, &NoProgress, &ctl)?;
    Ok(dst.into_inner())
}

#[test]
fn roundtrip_all_compressors_all_levels() {
    let data = payload();
    for ext in ALL_EXTENSIONS {
        let c = compressor(ext);
        for level in [
            CompressionLevel::Store,
            CompressionLevel::Fastest,
            CompressionLevel::Normal,
            CompressionLevel::Ultra,
        ] {
            let compressed = compress(&*c, &data, level);
            assert_ne!(compressed, data, "{ext}/{level:?} produced identity output");
            let restored = decompress(&*c, &compressed, &SafetyLimits::default()).unwrap();
            assert_eq!(restored, data, "{ext}/{level:?} roundtrip mismatch");
        }
    }
}

#[test]
fn zstd_honours_worker_resource_option() {
    let data = payload();
    let c = compressor("zst");
    let resources = ResourceOptions {
        threads: Some(2),
        memory_limit: None,
    };
    let compressed = compress_with_resources(&*c, &data, CompressionLevel::Normal, &resources);
    let restored = decompress(&*c, &compressed, &SafetyLimits::default()).unwrap();
    assert_eq!(restored, data);
}

#[test]
fn decompress_output_guardrail_trips() {
    let data = vec![0u8; 4 * 1024 * 1024]; // highly compressible 4 MiB
    let limits = SafetyLimits {
        max_output_bytes: 64 * 1024,
        ..SafetyLimits::default()
    };
    for ext in ALL_EXTENSIONS {
        let c = compressor(ext);
        let compressed = compress(&*c, &data, CompressionLevel::Normal);
        let err = decompress(&*c, &compressed, &limits).unwrap_err();
        assert!(
            matches!(err, FormatError::ResourceLimitExceeded(_)),
            "{ext}: expected ResourceLimitExceeded, got {err:?}"
        );
    }
}

#[test]
fn compress_reports_chunked_progress() {
    let data = payload(); // several 64 KiB chunks
    for ext in ALL_EXTENSIONS {
        let c = compressor(ext);
        let sink = CountingSink::default();
        let ctl = ControlToken::new();
        let mut src = Cursor::new(data.clone());
        let mut dst = Cursor::new(Vec::new());
        c.compress(
            &mut src,
            &mut dst,
            CompressionLevel::Fastest,
            &ResourceOptions::default(),
            &sink,
            &ctl,
        )
        .unwrap();
        let calls = sink.0.load(Ordering::Relaxed);
        assert!(
            calls >= data.len() as u64 / (64 * 1024),
            "{ext}: expected chunk-granular progress, got {calls} calls"
        );
    }
}

#[test]
fn compress_honours_cancellation() {
    let c = compressor("gz");
    let ctl = ControlToken::new();
    ctl.cancel();
    let mut src = Cursor::new(payload());
    let mut dst = Cursor::new(Vec::new());
    let err = c
        .compress(
            &mut src,
            &mut dst,
            CompressionLevel::Normal,
            &ResourceOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Cancelled));
}

#[test]
fn corrupt_input_is_an_error_not_a_panic() {
    for ext in ALL_EXTENSIONS {
        let c = compressor(ext);
        let garbage = b"this is definitely not a compressed stream".to_vec();
        let result = decompress(&*c, &garbage, &SafetyLimits::default());
        assert!(result.is_err(), "{ext}: garbage input must fail");
    }
}
