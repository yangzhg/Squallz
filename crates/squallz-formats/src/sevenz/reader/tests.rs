use super::*;
use sevenz_rust2::{ArchiveWriter, EncoderConfiguration, EncoderMethod};
use squallz_format_api::NoProgress;
use std::io::Cursor;

struct CountedSource {
    inner: Cursor<Vec<u8>>,
    bytes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    cancel_after: Option<(ControlToken, usize)>,
}

impl Read for CountedSource {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        let before = self
            .bytes
            .fetch_add(read, std::sync::atomic::Ordering::Relaxed);
        if let Some((ctl, limit)) = &self.cancel_after {
            if before + read >= *limit {
                ctl.cancel();
                return Err(std::io::Error::other("cancelled source read"));
            }
        }
        Ok(read)
    }
}

impl std::io::Seek for CountedSource {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut self.inner, position)
    }
}

#[test]
fn entry_prefix_does_not_decode_the_complete_file() {
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("large"),
            Some(std::io::repeat(b'x').take(8 * 1024 * 1024)),
        )
        .unwrap();
    writer
        .push_archive_entry(ArchiveEntry::new_file("next"), Some(Cursor::new(b"next")))
        .unwrap();
    writer
        .push_archive_entry(ArchiveEntry::new_file("empty"), None::<Cursor<Vec<u8>>>)
        .unwrap();
    let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut reader = SevenZArchiveReader::open(
        Box::new(CountedSource {
            inner: Cursor::new(writer.finish().unwrap().into_inner()),
            bytes: bytes.clone(),
            cancel_after: None,
        }),
        &OpenOptions::default(),
    )
    .unwrap();
    bytes.store(0, std::sync::atomic::Ordering::Relaxed);
    let mut prefix = [0; 16];
    reader
        .read_entry(&EntryPath::from_utf8("large"), &mut |entry| {
            entry.read_exact(&mut prefix)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(prefix, [b'x'; 16]);
    assert!(
        bytes.load(std::sync::atomic::Ordering::Relaxed) < 64 * 1024,
        "reading a prefix must not decode the complete entry"
    );
    bytes.store(0, std::sync::atomic::Ordering::Relaxed);
    let error = reader
        .read_entry(&EntryPath::from_utf8("large"), &mut |entry| {
            entry.read_exact(&mut prefix)?;
            Err(FormatError::ResourceLimitExceeded(
                "preview size limit".into(),
            ))
        })
        .unwrap_err();
    assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
    assert!(bytes.load(std::sync::atomic::Ordering::Relaxed) < 64 * 1024);
    let mut next = Vec::new();
    reader
        .read_entry(&EntryPath::from_utf8("next"), &mut |entry| {
            entry.read_to_end(&mut next)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(next, b"next");
    let mut calls = 0;
    reader
        .read_entry(&EntryPath::from_utf8("empty"), &mut |entry| {
            calls += 1;
            assert_eq!(entry.read(&mut [0])?, 0);
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 1);
    assert!(reader
        .read_entry(&EntryPath::from_utf8("missing"), &mut |_| {
            panic!("missing entries must not call the consumer")
        })
        .is_err());
    reader
        .read_entry(&EntryPath::from_utf8("large"), &mut |entry| {
            assert_eq!(std::io::copy(entry, &mut std::io::sink())?, 8 * 1024 * 1024);
            Ok(())
        })
        .unwrap();
}

#[test]
fn entry_stream_checks_crc_when_consumed_completely() {
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("file"),
            Some(std::io::repeat(b'x').take(16 * 1024)),
        )
        .unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    bytes[32 + 16 * 1024 - 1] ^= 1;
    let mut reader =
        SevenZArchiveReader::open(Box::new(Cursor::new(bytes)), &OpenOptions::default()).unwrap();
    reader
        .read_entry(&EntryPath::from_utf8("file"), &mut |entry| {
            let mut prefix = [0; 16];
            entry.read_exact(&mut prefix)?;
            assert_eq!(prefix, [b'x'; 16]);
            Ok(())
        })
        .unwrap();
    let result = reader.read_entry(&EntryPath::from_utf8("file"), &mut |entry| {
        std::io::copy(entry, &mut std::io::sink())?;
        Ok(())
    });
    assert!(matches!(
        result,
        Err(FormatError::Io(_)) | Err(FormatError::CorruptArchive(_))
    ));
}

#[test]
fn encrypted_entry_stream_preserves_consumer_errors_and_password_classification() {
    use sevenz_rust2::encoder_options::AesEncoderOptions;
    use squallz_format_api::Password;
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![
        AesEncoderOptions::new("entry-test-password".into()).into(),
        EncoderConfiguration::new(EncoderMethod::COPY),
    ]);
    writer.set_encrypt_header(false);
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("file"),
            Some(std::io::repeat(b'x').take(4096)),
        )
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    for password in [None, Some("incorrect"), Some("entry-test-password")] {
        let mut reader = SevenZArchiveReader::open(
            Box::new(Cursor::new(bytes.clone())),
            &OpenOptions {
                password: password.map(Password::new),
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let result = reader.read_entry(&EntryPath::from_utf8("file"), &mut |entry| {
            std::io::copy(entry, &mut std::io::sink())?;
            Ok(())
        });
        match password {
            None => assert!(
                matches!(result, Err(FormatError::PasswordRequired)),
                "{result:?}"
            ),
            Some("incorrect") => assert!(
                matches!(result, Err(FormatError::WrongPassword)),
                "{result:?}"
            ),
            _ => {
                result.unwrap();
                let error = reader
                    .read_entry(&EntryPath::from_utf8("file"), &mut |entry| {
                        entry.read_exact(&mut [0; 16])?;
                        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into())
                    })
                    .unwrap_err();
                assert!(
                    matches!(error, FormatError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied)
                );
            }
        }
    }
}

#[test]
fn solid_entry_read_cancels_while_skipping_preceding_data_and_while_consuming() {
    use sevenz_rust2::SourceReader;
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    writer
        .push_archive_entries(
            vec![
                ArchiveEntry::new_file("first"),
                ArchiveEntry::new_file("second"),
            ],
            vec![
                SourceReader::new(std::io::repeat(b'a').take(1024 * 1024)),
                SourceReader::new(std::io::repeat(b'b').take(1024 * 1024)),
            ],
        )
        .unwrap();
    let data = writer.finish().unwrap().into_inner();
    let ctl = ControlToken::default();
    let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut reader = SevenZArchiveReader::open_controlled(
        Box::new(CountedSource {
            inner: Cursor::new(data.clone()),
            bytes: bytes.clone(),
            cancel_after: Some((ctl.clone(), 256 * 1024)),
        }),
        &OpenOptions::default(),
        &ctl,
    )
    .unwrap();
    bytes.store(0, std::sync::atomic::Ordering::Relaxed);
    let result = reader.read_entry(&EntryPath::from_utf8("second"), &mut |_| {
        panic!("cancellation during the preceding entry must prevent consuming the target")
    });
    assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
    assert!(bytes.load(std::sync::atomic::Ordering::Relaxed) < 512 * 1024);
    let ctl = ControlToken::default();
    let mut reader = SevenZArchiveReader::open_controlled(
        Box::new(Cursor::new(data)),
        &OpenOptions::default(),
        &ctl,
    )
    .unwrap();
    let result = reader.read_entry(&EntryPath::from_utf8("second"), &mut |entry| {
        let mut prefix = [0; 16];
        entry.read_exact(&mut prefix)?;
        assert_eq!(prefix, [b'b'; 16]);
        ctl.cancel();
        entry.read_exact(&mut prefix)?;
        Ok(())
    });
    assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
}

fn link_archive(target: &[u8]) -> Vec<u8> {
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    let mut entry = ArchiveEntry::new_file("link");
    entry.has_windows_attributes = true;
    entry.windows_attributes = FILE_ATTRIBUTE_UNIX_EXTENSION | (0o120777 << 16);
    writer
        .push_archive_entry(entry, Some(Cursor::new(target)))
        .unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn symlink_targets_are_bounded_validated_and_cancellable() {
    let ctl = ControlToken::default();
    assert_eq!(
        read_symlink_target(&mut &b"../target"[..], &ctl).unwrap(),
        b"../target"
    );
    for target in [&b""[..], &b"a\0b"[..], &b"\xff"[..]] {
        assert!(matches!(
            read_symlink_target(&mut &*target, &ctl),
            Err(FormatError::CorruptArchive(_))
        ));
        let mut reader = SevenZArchiveReader::open(
            Box::new(Cursor::new(link_archive(target))),
            &OpenOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            reader.entries().next().unwrap(),
            Err(FormatError::CorruptArchive(_))
        ));
    }
    let mut oversized = std::io::repeat(b'a');
    assert!(matches!(
        read_symlink_target(&mut oversized, &ctl),
        Err(FormatError::ResourceLimitExceeded(_))
    ));
    ctl.cancel();
    assert!(matches!(
        read_symlink_target(&mut &b"target"[..], &ctl),
        Err(FormatError::Cancelled)
    ));
}

#[test]
fn symlink_listing_validates_crc_and_resource_limits() {
    for oversized in [false, true] {
        let mut bytes = link_archive(&vec![
            b'a';
            if oversized {
                MAX_SYMLINK_TARGET_BYTES + 1
            } else {
                16
            }
        ]);
        if !oversized {
            bytes[32] ^= 1;
        }
        let mut reader =
            SevenZArchiveReader::open(Box::new(Cursor::new(bytes)), &OpenOptions::default())
                .unwrap();
        let error = reader.entries().next().unwrap().unwrap_err();
        if oversized {
            assert!(
                matches!(error, FormatError::ResourceLimitExceeded(_)),
                "{error:?}"
            );
        } else {
            assert!(
                matches!(error, FormatError::Io(_) | FormatError::CorruptArchive(_)),
                "{error:?}"
            );
        }
    }
}

#[test]
fn damaged_symlinks_never_publish_a_partial_target() {
    let root =
        std::env::temp_dir().join(format!("squallz-7z-damaged-links-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("link"), b"original").unwrap();
    let mut bytes = link_archive(b"target");
    bytes[32] ^= 1;
    for best_effort in [false, true] {
        let mut reader = SevenZArchiveReader::open(
            Box::new(Cursor::new(bytes.clone())),
            &OpenOptions::default(),
        )
        .unwrap();
        let result = reader.extract_with_report(
            &root,
            None,
            &ExtractOptions {
                best_effort,
                overwrite: squallz_format_api::OverwritePolicy::Overwrite,
                ..ExtractOptions::default()
            },
            &NoProgress,
            &ControlToken::default(),
        );
        if best_effort {
            assert_eq!(result.unwrap().failed, 1);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(std::fs::read(root.join("link")).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_listing_returns_cancellation() {
    let ctl = ControlToken::default();
    let mut reader = SevenZArchiveReader::open_controlled(
        Box::new(Cursor::new(link_archive(b"target"))),
        &OpenOptions::default(),
        &ctl,
    )
    .unwrap();
    ctl.cancel();
    assert!(matches!(
        reader.entries().next().unwrap(),
        Err(FormatError::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_descendants_cannot_write_outside_the_destination() {
    let root =
        std::env::temp_dir().join(format!("squallz-7z-link-breakout-{}", std::process::id()));
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("payload"), b"original").unwrap();
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    let mut link = ArchiveEntry::new_file("link");
    link.has_windows_attributes = true;
    link.windows_attributes = FILE_ATTRIBUTE_UNIX_EXTENSION | (0o120777 << 16);
    writer
        .push_archive_entry(link, Some(Cursor::new(b"../outside")))
        .unwrap();
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("link/payload"),
            Some(Cursor::new(b"changed")),
        )
        .unwrap();
    let mut reader = SevenZArchiveReader::open(
        Box::new(Cursor::new(writer.finish().unwrap().into_inner())),
        &OpenOptions::default(),
    )
    .unwrap();
    let result = reader.extract(
        &root.join("out"),
        None,
        &ExtractOptions {
            overwrite: squallz_format_api::OverwritePolicy::Overwrite,
            best_effort: true,
            ..ExtractOptions::default()
        },
        &NoProgress,
        &ControlToken::default(),
    );
    assert!(
        matches!(result, Err(FormatError::SymlinkBreakout(_))),
        "{result:?}"
    );
    assert_eq!(std::fs::read(outside.join("payload")).unwrap(), b"original");
    std::fs::remove_dir_all(root).unwrap();
}
