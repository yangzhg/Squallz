//! Encryption tests: AES-256 write/read and legacy ZipCrypto read-only
//! interop.

mod common;

use std::fs;
use std::process::Command;

use common::{command_exists, engine, TempDir};
use squallz_format_api::{
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions, Password,
};

fn open_with(password: Option<&str>) -> OpenOptions {
    OpenOptions {
        password: password.map(Password::new),
        encoding_override: None,
    }
}

#[test]
fn aes256_roundtrip_and_password_errors() {
    let tmp = TempDir::new("aes");
    let src = tmp.path().join("secret.txt");
    fs::write(&src, b"top secret content").unwrap();
    let archive = tmp.path().join("secret.zip");
    let eng = engine();
    let ctl = ControlToken::new();

    eng.create(
        &archive,
        &[src],
        &CreateOptions {
            password: Some(Password::new("correct horse")),
            ..CreateOptions::default()
        },
        &NoProgress,
        &ctl,
    )
    .unwrap();

    // Listing works without a password; metadata marks entries encrypted.
    let entries = eng.list(&archive, &open_with(None)).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].encrypted);

    // Extracting without a password reports PasswordRequired.
    let err = eng
        .extract(
            &archive,
            &tmp.path().join("no-pw"),
            None,
            &open_with(None),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");

    // A wrong password reports WrongPassword (AES verifier).
    let err = eng
        .extract(
            &archive,
            &tmp.path().join("bad-pw"),
            None,
            &open_with(Some("wrong password")),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::WrongPassword), "{err:?}");

    // The correct password decrypts the content.
    let dest = tmp.path().join("good-pw");
    eng.extract(
        &archive,
        &dest,
        None,
        &open_with(Some("correct horse")),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(
        fs::read(dest.join("secret.txt")).unwrap(),
        b"top secret content"
    );

    // test() also distinguishes the password cases.
    let err = eng
        .test(&archive, &open_with(None), &NoProgress, &ctl)
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");
    let report = eng
        .test(
            &archive,
            &open_with(Some("correct horse")),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
}

#[test]
fn zipcrypto_legacy_archive_is_readable() {
    if !command_exists("zip") {
        eprintln!("skipped: system zip not found");
        return;
    }
    let tmp = TempDir::new("zipcrypto");
    fs::write(tmp.path().join("legacy.txt"), b"legacy zipcrypto data").unwrap();
    let archive = tmp.path().join("legacy.zip");
    // `zip -P` uses the legacy ZipCrypto stream cipher (read-only support
    // on our side; we never write it).
    let out = Command::new("zip")
        .arg("-P")
        .arg("oldpass")
        .arg(&archive)
        .arg("legacy.txt")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());

    let eng = engine();
    let ctl = ControlToken::new();
    let entries = eng.list(&archive, &open_with(None)).unwrap();
    assert!(entries[0].encrypted);

    // No password → PasswordRequired.
    let err = eng
        .extract(
            &archive,
            &tmp.path().join("no-pw"),
            None,
            &open_with(None),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");

    // Correct password decrypts.
    let dest = tmp.path().join("dest");
    eng.extract(
        &archive,
        &dest,
        None,
        &open_with(Some("oldpass")),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(
        fs::read(dest.join("legacy.txt")).unwrap(),
        b"legacy zipcrypto data"
    );
}
