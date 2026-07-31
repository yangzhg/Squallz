//! Resolves the preset snapshot used by Finder, Explorer and Linux file-manager
//! CLI fallbacks. Paths and layout remain owned by the generated integration
//! scripts; this module only supplies reusable archive policy.

use squallz_core::api::FormatError;
use squallz_core::{
    CreateCredential, CreateOutput, EntryNameEncoding, ExistingOutputPolicy, NamedPreset,
    PresetDocument, PresetError, PresetId, PresetStore, SymlinkHandling, VolumeMode,
};

use crate::args::{OverwriteArg, SymlinkArg};
use crate::errors::CliError;

pub(crate) struct FileManagerCreateOptions {
    pub level: u8,
    pub split: Option<u64>,
    pub excludes: Vec<String>,
    pub test_after_create: bool,
}

pub(crate) struct FileManagerExtractOptions {
    pub overwrite: OverwriteArg,
    pub encoding: Option<String>,
    pub symlinks: SymlinkArg,
}

pub(crate) fn load_create_options() -> Result<FileManagerCreateOptions, CliError> {
    let document = load_document()?;
    resolve_create_options(&document)
}

pub(crate) fn load_extract_options() -> Result<FileManagerExtractOptions, CliError> {
    let document = load_document()?;
    resolve_extract_options(&document)
}

fn load_document() -> Result<PresetDocument, CliError> {
    preset_store()?.load().map_err(preset_load_error)
}

pub(crate) fn preset_store() -> Result<PresetStore, CliError> {
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .ok_or_else(|| {
            CliError::from(FormatError::Other(
                "no private user configuration directory is available for shared presets"
                    .to_owned(),
            ))
        })?;
    Ok(PresetStore::new(base.join("Squallz").join("presets.json")))
}

fn preset_load_error(error: PresetError) -> CliError {
    match error {
        PresetError::Io(error) => FormatError::from(error).into(),
        error => FormatError::Other(format!(
            "cannot load the shared file-manager preset: {error}"
        ))
        .into(),
    }
}

fn resolve_create_options(document: &PresetDocument) -> Result<FileManagerCreateOptions, CliError> {
    let Some(id) = document.bindings.file_manager_create.as_ref() else {
        return Ok(safe_create_defaults());
    };
    let options = find_preset(document, id)
        .and_then(NamedPreset::create_options)
        .ok_or_else(|| invalid_binding("create"))?;
    if options.format.as_str() != "7z"
        || options.credential != CreateCredential::None
        || options.output != CreateOutput::Archive
    {
        return Err(invalid_binding("create"));
    }
    Ok(FileManagerCreateOptions {
        level: options.level.get(),
        split: match options.volumes {
            VolumeMode::Single => None,
            VolumeMode::Split { size_bytes } => Some(size_bytes.get()),
        },
        excludes: options.content_policy.resolve_excludes(&options.excludes),
        test_after_create: options.test_after_create,
    })
}

fn resolve_extract_options(
    document: &PresetDocument,
) -> Result<FileManagerExtractOptions, CliError> {
    let Some(id) = document.bindings.file_manager_extract.as_ref() else {
        return Ok(safe_extract_defaults());
    };
    let options = find_preset(document, id)
        .and_then(NamedPreset::extract_options)
        .ok_or_else(|| invalid_binding("extract"))?;
    Ok(FileManagerExtractOptions {
        overwrite: match options.existing_output {
            ExistingOutputPolicy::Ask => OverwriteArg::Ask,
            ExistingOutputPolicy::Skip => OverwriteArg::Skip,
            ExistingOutputPolicy::Overwrite => OverwriteArg::All,
            ExistingOutputPolicy::Rename => OverwriteArg::Rename,
        },
        encoding: match &options.encoding {
            EntryNameEncoding::Auto => None,
            EntryNameEncoding::Named { label } => Some(label.clone()),
        },
        symlinks: match options.symlinks {
            SymlinkHandling::Preserve => SymlinkArg::Preserve,
            SymlinkHandling::Skip => SymlinkArg::Skip,
            SymlinkHandling::Follow => SymlinkArg::Follow,
        },
    })
}

fn find_preset<'a>(document: &'a PresetDocument, id: &PresetId) -> Option<&'a NamedPreset> {
    document.presets.iter().find(|preset| preset.id() == id)
}

fn invalid_binding(kind: &str) -> CliError {
    FormatError::Other(format!(
        "the shared file-manager {kind} preset binding is invalid"
    ))
    .into()
}

fn safe_create_defaults() -> FileManagerCreateOptions {
    FileManagerCreateOptions {
        level: 5,
        split: None,
        excludes: Vec::new(),
        test_after_create: false,
    }
}

fn safe_extract_defaults() -> FileManagerExtractOptions {
    FileManagerExtractOptions {
        overwrite: OverwriteArg::Ask,
        encoding: None,
        symlinks: SymlinkArg::Preserve,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use squallz_core::{
        ByteSize, EntryNameEncoding, ExistingOutputPolicy, NamedPreset, PresetCompressionLevel,
        PresetDocument, PresetId, PresetLabel, PresetStore, SymlinkHandling, VolumeMode,
    };

    use super::{
        preset_load_error, resolve_create_options, resolve_extract_options, OverwriteArg,
        SymlinkArg,
    };

    #[test]
    fn unbound_file_manager_slots_use_independent_safe_defaults() {
        let mut document = PresetDocument::seeded();
        document.bindings.file_manager_create = None;
        document.bindings.file_manager_extract = None;

        let Ok(create) = resolve_create_options(&document) else {
            panic!("safe create defaults should resolve")
        };
        assert_eq!(create.level, 5);
        assert_eq!(create.split, None);
        assert!(create.excludes.is_empty());
        assert!(!create.test_after_create);

        let Ok(extract) = resolve_extract_options(&document) else {
            panic!("safe extract defaults should resolve")
        };
        assert!(matches!(extract.overwrite, OverwriteArg::Ask));
        assert!(extract.encoding.is_none());
        assert!(matches!(extract.symlinks, SymlinkArg::Preserve));
    }

    #[test]
    fn bound_presets_map_archive_policy_without_paths_or_layout() {
        let mut document = PresetDocument::seeded();

        let mut create_options = document
            .presets
            .iter()
            .find_map(NamedPreset::create_options)
            .expect("built-in create preset")
            .clone();
        create_options.level = PresetCompressionLevel::new(8).expect("valid level");
        create_options.volumes = VolumeMode::Split {
            size_bytes: ByteSize::new(512 * 1024),
        };
        create_options.excludes = vec!["*.tmp".to_owned(), ".git".to_owned(), "*.tmp".to_owned()];
        create_options.test_after_create = true;
        let create_id = PresetId::new("user.create.file-manager").expect("valid id");
        document.presets.push(NamedPreset::Create {
            id: create_id.clone(),
            label: PresetLabel::new("File manager create").expect("valid label"),
            built_in: false,
            options: create_options,
        });
        document.bindings.file_manager_create = Some(create_id);

        let mut extract_options = document
            .presets
            .iter()
            .find_map(NamedPreset::extract_options)
            .expect("built-in extract preset")
            .clone();
        extract_options.existing_output = ExistingOutputPolicy::Rename;
        extract_options.symlinks = SymlinkHandling::Skip;
        extract_options.encoding = EntryNameEncoding::Named {
            label: "shift_jis".to_owned(),
        };
        let extract_id = PresetId::new("user.extract.file-manager").expect("valid id");
        document.presets.push(NamedPreset::Extract {
            id: extract_id.clone(),
            label: PresetLabel::new("File manager extract").expect("valid label"),
            built_in: false,
            options: extract_options,
        });
        document.bindings.file_manager_extract = Some(extract_id);
        document.validate().expect("valid preset document");

        let Ok(create) = resolve_create_options(&document) else {
            panic!("bound create preset should resolve")
        };
        assert_eq!(create.level, 8);
        assert_eq!(create.split, Some(512 * 1024));
        assert_eq!(create.excludes, ["*.tmp", ".git"]);
        assert!(create.test_after_create);

        let Ok(extract) = resolve_extract_options(&document) else {
            panic!("bound extract preset should resolve")
        };
        assert!(matches!(extract.overwrite, OverwriteArg::Rename));
        assert_eq!(extract.encoding.as_deref(), Some("shift_jis"));
        assert!(matches!(extract.symlinks, SymlinkArg::Skip));
    }

    #[test]
    fn malformed_preset_store_fails_instead_of_silently_using_defaults() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("squallz-cli-preset-test-{nonce}"));
        let path = root.join("presets.json");
        fs::create_dir_all(&root).expect("create fixture directory");
        fs::write(&path, b"{not json").expect("write malformed preset fixture");

        let error = PresetStore::new(&path)
            .load()
            .map_err(preset_load_error)
            .expect_err("malformed preset must fail");
        match error {
            crate::errors::CliError::Format(squallz_core::api::FormatError::Other(message)) => {
                assert!(message.contains("cannot load the shared file-manager preset"));
            }
            crate::errors::CliError::Format(other) => {
                panic!("unexpected format error: {other}")
            }
            crate::errors::CliError::Update(other) => {
                panic!("unexpected update error: {other}")
            }
            crate::errors::CliError::Exit(code) => panic!("unexpected exit code: {code}"),
        }

        fs::remove_dir_all(root).expect("remove fixture directory");
    }
}
