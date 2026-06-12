//! Legacy entry-name encoding tests (CP936/GBK): manual override and
//! automatic detection. The fixture archive is handcrafted so its names are
//! raw GBK bytes without the UTF-8 flag, exactly like archives produced by
//! legacy Windows tools.

mod common;

use std::fs;

use common::{build_stored_zip, engine, RawZipEntry, TempDir};
use squallz_format_api::{ControlToken, ExtractOptions, NoProgress, OpenOptions};

const NAME_UTF8: &str = "压缩文件中文名称测试.txt";

fn gbk_fixture(tmp: &TempDir) -> std::path::PathBuf {
    let (gbk_name, _, had_errors) = encoding_rs::GBK.encode(NAME_UTF8);
    assert!(!had_errors);
    assert!(
        std::str::from_utf8(&gbk_name).is_err(),
        "fixture name must not be valid UTF-8"
    );
    let archive = tmp.path().join("gbk.zip");
    fs::write(
        &archive,
        build_stored_zip(&[RawZipEntry {
            name: gbk_name.into_owned(),
            data: "GBK 命名条目".as_bytes().to_vec(),
        }]),
    )
    .unwrap();
    archive
}

#[test]
fn gbk_names_decode_with_manual_override() {
    let tmp = TempDir::new("gbk-override");
    let archive = gbk_fixture(&tmp);
    let opts = OpenOptions {
        encoding_override: Some("gbk".into()),
        ..OpenOptions::default()
    };
    let entries = engine().list(&archive, &opts).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, NAME_UTF8);
    assert_eq!(entries[0].path.encoding, "GBK");
    // The raw bytes stay untouched as the lookup key.
    assert_eq!(
        entries[0].path.raw,
        encoding_rs::GBK.encode(NAME_UTF8).0.into_owned()
    );
}

#[test]
fn gbk_names_decode_with_auto_detection() {
    let tmp = TempDir::new("gbk-auto");
    let archive = gbk_fixture(&tmp);
    let entries = engine().list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].path.display, NAME_UTF8,
        "chardetng should detect GBK; got encoding {}",
        entries[0].path.encoding
    );
}

#[test]
fn gbk_named_entry_extracts_with_decoded_name() {
    let tmp = TempDir::new("gbk-extract");
    let archive = gbk_fixture(&tmp);
    let dest = tmp.path().join("dest");
    let opts = OpenOptions {
        encoding_override: Some("gbk".into()),
        ..OpenOptions::default()
    };
    engine()
        .extract(
            &archive,
            &dest,
            None,
            &opts,
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        fs::read(dest.join(NAME_UTF8)).unwrap(),
        "GBK 命名条目".as_bytes()
    );
}
