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
fn encrypted_infozip_native_split_uses_the_secure_password_bridge() {
    if !command_exists("zip") || !command_exists("7zz") {
        eprintln!("skipped: Info-ZIP zip or 7zz not found");
        return;
    }

    let tmp = TempDir::new("encrypted-native-split");
    let mut payload = vec![0u8; 200 * 1024];
    let mut state = 0x6a09_e667_f3bc_c909u64;
    for byte in &mut payload {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    let source = tmp.path().join("payload.bin");
    fs::write(&source, &payload).unwrap();

    let eng = engine();
    let ctl = ControlToken::new();
    let encrypted = tmp.path().join("encrypted.zip");
    eng.create(
        &encrypted,
        &[source],
        &CreateOptions {
            password: Some(Password::new("native-split-password")),
            ..CreateOptions::default()
        },
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let output = Command::new("zip")
        .args(["-q", "-s", "64k", "encrypted.zip", "--out", "native.zip"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Info-ZIP split conversion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let first = tmp.path().join("native.z01");
    let final_path = tmp.path().join("native.zip");
    assert!(first.is_file());
    assert!(final_path.is_file());

    let entries = eng.list(&first, &open_with(None)).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].encrypted);

    let error = eng
        .test(&first, &open_with(None), &NoProgress, &ctl)
        .unwrap_err();
    assert!(matches!(error, FormatError::PasswordRequired), "{error:?}");
    let error = eng
        .test(
            &first,
            &open_with(Some("wrong-native-split-password")),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::WrongPassword), "{error:?}");

    let wrong_dest = tmp.path().join("wrong-password");
    let error = eng
        .extract(
            &first,
            &wrong_dest,
            None,
            &open_with(Some("wrong-native-split-password")),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::WrongPassword), "{error:?}");
    assert!(!wrong_dest.join("payload.bin").exists());

    let dest = tmp.path().join("dest");
    eng.extract(
        &first,
        &dest,
        None,
        &open_with(Some("native-split-password")),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("payload.bin")).unwrap(), payload);
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
