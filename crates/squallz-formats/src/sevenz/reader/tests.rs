use super::*;
use sevenz_rust2::{ArchiveWriter, EncoderConfiguration, EncoderMethod};
use squallz_format_api::NoProgress;

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
