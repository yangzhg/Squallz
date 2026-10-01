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
fn solid_entries_are_read_once_with_their_own_metadata() {
    use sevenz_rust2::SourceReader;
    use std::sync::atomic::Ordering;

    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    writer
        .push_archive_entry(
            ArchiveEntry::new_directory("empty-dir"),
            None::<Cursor<Vec<u8>>>,
        )
        .unwrap();
    writer
        .push_archive_entries(
            (0..4)
                .map(|index| ArchiveEntry::new_file(&format!("file-{index}")))
                .collect(),
            (0..4)
                .map(|index| SourceReader::new(std::io::repeat(b'a' + index).take(1024 * 1024)))
                .collect(),
        )
        .unwrap();
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("empty-file"),
            None::<Cursor<Vec<u8>>>,
        )
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
    assert_eq!(reader.inner.archive().blocks.len(), 1);
    let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
    bytes.store(0, Ordering::Relaxed);
    let mut visited = HashSet::new();
    reader
        .read_entries(
            &entries,
            &mut |meta, data| {
                assert!(visited.insert(meta.path.display.clone()));
                match meta.path.display.as_str() {
                    "empty-dir" => {
                        assert!(matches!(meta.entry_type, EntryType::Dir));
                        assert!(data.is_none());
                    }
                    "empty-file" => {
                        assert!(matches!(meta.entry_type, EntryType::File));
                        assert_eq!(data.unwrap().read(&mut [0])?, 0);
                    }
                    name => {
                        let index = name.strip_prefix("file-").unwrap().parse::<u8>().unwrap();
                        assert_eq!(meta.size, 1024 * 1024);
                        let data = data.unwrap();
                        let mut buffer = [0; 64 * 1024];
                        let mut read = 0;
                        loop {
                            let count = data.read(&mut buffer)?;
                            if count == 0 {
                                break;
                            }
                            assert!(buffer[..count].iter().all(|byte| *byte == b'a' + index));
                            read += count;
                        }
                        assert_eq!(read, meta.size as usize);
                    }
                }
                Ok(())
            },
            &ControlToken::default(),
        )
        .unwrap();
    assert_eq!(visited.len(), entries.len());
    assert!(
        bytes.load(Ordering::Relaxed) <= 4 * 1024 * 1024 + 64 * 1024,
        "the source must not be decoded again for preceding entries: {} bytes read",
        bytes.load(Ordering::Relaxed)
    );
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
fn complete_entry_reads_preserve_duplicate_names_drain_tails_and_check_crc() {
    use sevenz_rust2::SourceReader;

    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    writer
        .push_archive_entries(
            vec![
                ArchiveEntry::new_file("same"),
                ArchiveEntry::new_file("same"),
            ],
            vec![
                SourceReader::new(std::io::repeat(b'a').take(4096)),
                SourceReader::new(std::io::repeat(b'b').take(4096)),
            ],
        )
        .unwrap();
    let original = writer.finish().unwrap().into_inner();
    for corrupt in [false, true] {
        let mut bytes = original.clone();
        if corrupt {
            bytes[32 + 4096 - 1] ^= 1;
        }
        let mut reader =
            SevenZArchiveReader::open(Box::new(Cursor::new(bytes)), &OpenOptions::default())
                .unwrap();
        let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
        let mut calls = 0;
        let result = reader.read_entries(
            &entries,
            &mut |meta, data| {
                assert_eq!(meta.path.display, "same");
                let mut prefix = [0; 16];
                data.unwrap().read_exact(&mut prefix)?;
                assert_eq!(prefix, [b'a' + calls; 16]);
                calls += 1;
                Ok(())
            },
            &ControlToken::default(),
        );
        if corrupt {
            assert!(matches!(
                result,
                Err(FormatError::Io(_) | FormatError::CorruptArchive(_))
            ));
            assert_eq!(calls, 1, "a damaged tail must stop before the next entry");
        } else {
            result.unwrap();
            assert_eq!(calls, 2);
        }
    }
}

#[test]
fn complete_entry_reads_preserve_password_errors_and_stop_after_consumer_failure() {
    use sevenz_rust2::encoder_options::AesEncoderOptions;
    use squallz_format_api::Password;
    use std::sync::atomic::Ordering;

    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![
        AesEncoderOptions::new("entry-test-password".into()).into(),
        EncoderConfiguration::new(EncoderMethod::COPY),
    ]);
    writer.set_encrypt_header(false);
    for name in ["first", "later"] {
        writer
            .push_archive_entry(
                ArchiveEntry::new_file(name),
                Some(std::io::repeat(b'x').take(1024 * 1024)),
            )
            .unwrap();
    }
    let archive = writer.finish().unwrap().into_inner();
    for password in [None, Some("incorrect"), Some("entry-test-password")] {
        let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = SevenZArchiveReader::open(
            Box::new(CountedSource {
                inner: Cursor::new(archive.clone()),
                bytes: bytes.clone(),
                cancel_after: None,
            }),
            &OpenOptions {
                password: password.map(Password::new),
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
        let result = reader.read_entries(
            &entries,
            &mut |_, data| {
                assert_eq!(
                    std::io::copy(data.unwrap(), &mut std::io::sink())?,
                    1024 * 1024
                );
                Ok(())
            },
            &ControlToken::default(),
        );
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
                for cancel in [false, true] {
                    bytes.store(0, Ordering::Relaxed);
                    let ctl = ControlToken::default();
                    let mut calls = 0;
                    let result = reader.read_entries(
                        &entries,
                        &mut |_, data| {
                            calls += 1;
                            data.unwrap().read_exact(&mut [0; 16])?;
                            if cancel {
                                ctl.cancel();
                                Ok(())
                            } else {
                                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)
                                    .into())
                            }
                        },
                        &ctl,
                    );
                    if cancel {
                        assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
                    } else {
                        assert!(
                            matches!(result, Err(FormatError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
                        );
                    }
                    assert_eq!(calls, 1, "a failed consumer must not visit later blocks");
                    assert!(
                        bytes.load(Ordering::Relaxed) < 64 * 1024,
                        "failed consumers must not drain data or decode later blocks"
                    );
                }
            }
        }
    }
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
fn compressed_entry_prefix_and_cancellation_do_not_read_the_complete_solid_block() {
    use sevenz_rust2::SourceReader;
    use std::sync::atomic::Ordering;

    let mut seed = 0x6b8b4567u32;
    let content: Vec<_> = (0..4 * 1024 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::LZMA2)]);
    writer
        .push_archive_entries(
            vec![
                ArchiveEntry::new_file("large"),
                ArchiveEntry::new_file("next"),
            ],
            vec![
                SourceReader::new(Cursor::new(&content[..])),
                SourceReader::new(Cursor::new(&b"next"[..])),
            ],
        )
        .unwrap();
    let archive = writer.finish().unwrap().into_inner();
    assert!(archive.len() > 1024 * 1024);
    for cancel in [false, true] {
        let ctl = ControlToken::default();
        let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = SevenZArchiveReader::open_controlled(
            Box::new(CountedSource {
                inner: Cursor::new(archive.clone()),
                bytes: bytes.clone(),
                cancel_after: None,
            }),
            &OpenOptions::default(),
            &ctl,
        )
        .unwrap();
        bytes.store(0, Ordering::Relaxed);
        let mut prefix = [0; 16];
        let result = reader.read_entry(&EntryPath::from_utf8("large"), &mut |data| {
            data.read_exact(&mut prefix)?;
            if cancel {
                ctl.cancel();
            }
            Ok(())
        });
        assert_eq!(prefix, content[..16]);
        if cancel {
            assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
        } else {
            result.unwrap();
        }
        assert!(
            bytes.load(Ordering::Relaxed) < 256 * 1024,
            "a compressed prefix must not read the complete solid block: {} bytes",
            bytes.load(Ordering::Relaxed)
        );
    }
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
