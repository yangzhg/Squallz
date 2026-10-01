//! Encryption tests: AES-256 write/read and legacy ZipCrypto read-only
//! interop.

mod common;

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::Command;

use common::{command_exists, engine, TempDir};
use squallz_format_api::{
    ControlToken, CreateOptions, EntryPath, ExtractOptions, FormatError, NoProgress, OpenOptions,
    Password, SafetyLimits,
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

    assert!(matches!(
        eng.verify_password(&archive, &open_with(None), SafetyLimits::default(), &ctl),
        Err(FormatError::PasswordRequired)
    ));
    assert!(matches!(
        eng.verify_password(
            &archive,
            &open_with(Some("wrong password")),
            SafetyLimits::default(),
            &ctl
        ),
        Err(FormatError::WrongPassword)
    ));
    assert!(eng
        .verify_password(
            &archive,
            &open_with(Some("correct horse")),
            SafetyLimits::default(),
            &ctl
        )
        .unwrap());

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
        .test_summary(
            &archive,
            &open_with(None),
            &squallz_format_api::SafetyLimits::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");
    let report = eng
        .test_summary(
            &archive,
            &open_with(Some("correct horse")),
            &squallz_format_api::SafetyLimits::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
}

#[test]
fn password_verification_checks_the_smallest_encrypted_entry_through_its_authentication_tag() {
    let tmp = TempDir::new("password-verification");
    let archive = tmp.path().join("mixed.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("public.txt", options).unwrap();
    writer.write_all(b"hello").unwrap();
    let encrypted = options.with_aes_encryption(zip::AesMode::Aes256, "correct");
    writer.start_file("large.bin", encrypted).unwrap();
    writer.write_all(&vec![1; 100_000]).unwrap();
    writer.start_file("small.bin", encrypted).unwrap();
    writer.write_all(&vec![2; 70_000]).unwrap();
    writer.finish().unwrap();

    let eng = engine();
    let options = open_with(Some("correct"));
    let control = ControlToken::default();
    let small = EntryPath::from_utf8("small.bin");
    assert!(!eng
        .read_entry_verifying_password(
            &archive,
            &EntryPath::from_utf8("public.txt"),
            &options,
            &control,
            &mut |reader| {
                std::io::copy(reader, &mut std::io::sink())?;
                Ok(())
            },
        )
        .unwrap());
    assert!(!eng
        .read_entry_verifying_password(&archive, &small, &options, &control, &mut |reader| {
            reader.read_exact(&mut [0; 8])?;
            Ok(())
        },)
        .unwrap());
    assert!(eng
        .read_entry_verifying_password(&archive, &small, &options, &control, &mut |reader| {
            std::io::copy(reader, &mut std::io::sink())?;
            Ok(())
        },)
        .unwrap());
    let interrupted = ControlToken::default();
    assert!(matches!(
        eng.read_entry_verifying_password(
            &archive,
            &small,
            &options,
            &interrupted,
            &mut |reader| {
                reader.read_exact(&mut [0; 8])?;
                interrupted.cancel();
                std::io::copy(reader, &mut std::io::sink())?;
                Ok(())
            },
        ),
        Err(FormatError::Cancelled)
    ));
    let limits = SafetyLimits {
        max_output_bytes: 70_000,
        ..SafetyLimits::default()
    };
    assert!(eng
        .verify_password(&archive, &options, limits, &control)
        .unwrap());
    for limits in [
        SafetyLimits {
            max_output_bytes: 69_999,
            ..limits
        },
        SafetyLimits {
            max_entries: 2,
            ..limits
        },
    ] {
        assert!(matches!(
            eng.verify_password(&archive, &options, limits, &control),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
    }
    let cancelled = ControlToken::default();
    cancelled.cancel();
    assert!(matches!(
        eng.verify_password(&archive, &options, limits, &cancelled),
        Err(FormatError::Cancelled)
    ));

    let mut raw = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let entry = raw.by_index_raw(2).unwrap();
    let tag_end = entry.data_start().unwrap() + entry.compressed_size() - 1;
    drop(entry);
    drop(raw);
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&archive)
        .unwrap();
    file.seek(SeekFrom::Start(tag_end)).unwrap();
    let mut byte = [0_u8];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 1;
    file.seek(SeekFrom::Start(tag_end)).unwrap();
    file.write_all(&byte).unwrap();
    drop(file);
    // The directory and password verifier remain intact; only the final
    // authentication tag fails, after all decoded bytes have been consumed.
    assert_eq!(
        eng.list(&archive, &OpenOptions::default()).unwrap().len(),
        3
    );
    assert!(eng
        .verify_password(&archive, &options, limits, &control)
        .is_err());
    assert!(eng
        .read_entry_verifying_password(&archive, &small, &options, &control, &mut |reader| {
            std::io::copy(reader, &mut std::io::sink())?;
            Ok(())
        },)
        .is_err());
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
        .test_summary(
            &first,
            &open_with(None),
            &squallz_format_api::SafetyLimits::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::PasswordRequired), "{error:?}");
    let error = eng
        .test_summary(
            &first,
            &open_with(Some("wrong-native-split-password")),
            &squallz_format_api::SafetyLimits::default(),
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

    assert!(eng
        .verify_password(
            &archive,
            &open_with(Some("oldpass")),
            SafetyLimits::default(),
            &ctl
        )
        .unwrap());
    assert!(eng
        .verify_password(
            &archive,
            &open_with(Some("incorrect")),
            SafetyLimits::default(),
            &ctl
        )
        .is_err());

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
