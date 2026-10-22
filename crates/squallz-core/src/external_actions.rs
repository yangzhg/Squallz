//! One policy snapshot for GUI tasks and file-manager CLI fallbacks.

use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::api::FormatError;
use crate::{
    find_available_create_destination, parent_or_current, CreateArtifactKind,
    CreateCompletionAction, CreateContentPolicy, CreateCredential, CreateOutput, Engine,
    EntryNameEncoding, OverwritePolicy, PostSuccessAction, PresetDocument, SymlinkPolicy,
    VolumeMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalTaskAction {
    Checksum,
    ExtractHere,
    ExtractToFolder,
    ExtractSfx,
    #[serde(rename = "compress-to-7z")]
    CompressTo7z,
    TestArchive,
}

impl FromStr for ExternalTaskAction {
    type Err = FormatError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "checksum" => Ok(Self::Checksum),
            "extract-here" => Ok(Self::ExtractHere),
            "extract-to-folder" => Ok(Self::ExtractToFolder),
            "compress-to-7z" => Ok(Self::CompressTo7z),
            "test-archive" => Ok(Self::TestArchive),
            _ => Err(FormatError::Unsupported(format!(
                "unknown file-manager action: {value}"
            ))),
        }
    }
}

#[derive(Debug)]
pub struct FileManagerCreateOptions {
    pub level: u8,
    pub encrypt_names: bool,
    pub split: Option<u64>,
    pub content_policy: CreateContentPolicy,
    pub excludes: Vec<String>,
    pub completion: CreateCompletionAction,
    pub post_success: PostSuccessAction,
    pub test_after_create: bool,
}

#[derive(Debug)]
pub struct FileManagerExtractOptions {
    pub overwrite: OverwritePolicy,
    pub encoding: Option<String>,
    pub symlinks: SymlinkPolicy,
}

#[derive(Debug)]
pub enum ExternalTaskPlan {
    Compress {
        inputs: Vec<PathBuf>,
        output: PathBuf,
        options: FileManagerCreateOptions,
    },
    Extract {
        items: Vec<(PathBuf, PathBuf)>,
        options: FileManagerExtractOptions,
        smart: bool,
        verify_sfx: bool,
    },
    Checksum {
        inputs: Vec<PathBuf>,
    },
    Test {
        paths: Vec<PathBuf>,
    },
}

impl Engine {
    /// Resolve once, before queueing. Publication still checks for conflicts;
    /// choosing an available name never authorizes replacing a racing output.
    pub fn resolve_external_task(
        &self,
        document: &PresetDocument,
        action: ExternalTaskAction,
        paths: Vec<PathBuf>,
        output: Option<PathBuf>,
    ) -> Result<Option<ExternalTaskPlan>, FormatError> {
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| match action {
                ExternalTaskAction::ExtractHere
                | ExternalTaskAction::ExtractToFolder
                | ExternalTaskAction::TestArchive => path.is_file(),
                _ => path.exists(),
            })
            .collect();
        let Some(first) = paths.first() else {
            return Ok(None);
        };
        let plan = match action {
            ExternalTaskAction::Checksum => ExternalTaskPlan::Checksum { inputs: paths },
            ExternalTaskAction::TestArchive => ExternalTaskPlan::Test { paths },
            ExternalTaskAction::CompressTo7z => {
                let options = file_manager_create_options(document)?;
                let output = match output {
                    Some(output) => output,
                    None => {
                        let mut name = if paths.len() == 1 {
                            let stem = self.archive_stem(first);
                            if first
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_none_or(|name| name == stem)
                            {
                                first.file_stem().unwrap_or_default().to_owned()
                            } else {
                                stem.into()
                            }
                        } else {
                            "Archive".into()
                        };
                        name.push(".7z");
                        let candidate = parent_or_current(first).join(name);
                        find_available_create_destination(
                            &candidate,
                            if options.split.is_some() {
                                CreateArtifactKind::SplitArchive
                            } else {
                                CreateArtifactKind::Archive
                            },
                        )?
                    }
                };
                ExternalTaskPlan::Compress {
                    inputs: paths,
                    output,
                    options,
                }
            }
            ExternalTaskAction::ExtractHere
            | ExternalTaskAction::ExtractToFolder
            | ExternalTaskAction::ExtractSfx => {
                let verify_sfx = action == ExternalTaskAction::ExtractSfx;
                if verify_sfx && paths.len() != 1 {
                    return Err(FormatError::Unsupported(
                        "SFX extraction requires one source per task".into(),
                    ));
                }
                let options = if verify_sfx {
                    FileManagerExtractOptions {
                        overwrite: OverwritePolicy::Ask,
                        symlinks: SymlinkPolicy::Skip,
                        encoding: None,
                    }
                } else {
                    file_manager_extract_options(document)?
                };
                let smart = action == ExternalTaskAction::ExtractHere;
                let items = paths
                    .into_iter()
                    .map(|path| {
                        let parent = parent_or_current(&path);
                        let dest =
                            if verify_sfx { output.clone() } else { None }.unwrap_or_else(|| {
                                if smart {
                                    parent.to_owned()
                                } else {
                                    parent.join(self.archive_stem(&path))
                                }
                            });
                        (path, dest)
                    })
                    .collect();
                ExternalTaskPlan::Extract {
                    items,
                    options,
                    smart,
                    verify_sfx,
                }
            }
        };
        Ok(Some(plan))
    }
}

fn invalid_binding(kind: &str) -> FormatError {
    FormatError::Other(format!(
        "the shared file-manager {kind} preset binding is invalid"
    ))
}

fn file_manager_create_options(
    document: &PresetDocument,
) -> Result<FileManagerCreateOptions, FormatError> {
    let Some(id) = document.bindings.file_manager_create.as_ref() else {
        return Ok(FileManagerCreateOptions {
            level: 5,
            encrypt_names: false,
            split: None,
            content_policy: CreateContentPolicy::KeepAllFiles,
            excludes: Vec::new(),
            completion: CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
        });
    };
    let options = document
        .preset(id)
        .and_then(|preset| preset.create_options())
        .ok_or_else(|| invalid_binding("create"))?;
    if options.format.as_str() != "7z"
        || options.credential != CreateCredential::None
        || options.output != CreateOutput::Archive
        || options.post_success != PostSuccessAction::KeepSource
        || options.completion == CreateCompletionAction::OpenInSquallz
    {
        return Err(invalid_binding("create"));
    }
    Ok(FileManagerCreateOptions {
        level: options.level.get(),
        encrypt_names: options.encrypt_names,
        split: match options.volumes {
            VolumeMode::Single => None,
            VolumeMode::Split { size_bytes } => Some(size_bytes.get()),
        },
        content_policy: options.content_policy,
        excludes: options.excludes.clone(),
        completion: options.completion,
        post_success: options.post_success,
        test_after_create: options.test_after_create,
    })
}

fn file_manager_extract_options(
    document: &PresetDocument,
) -> Result<FileManagerExtractOptions, FormatError> {
    let Some(id) = document.bindings.file_manager_extract.as_ref() else {
        return Ok(FileManagerExtractOptions {
            overwrite: OverwritePolicy::Ask,
            encoding: None,
            symlinks: SymlinkPolicy::Preserve,
        });
    };
    let options = document
        .preset(id)
        .and_then(|preset| preset.extract_options())
        .ok_or_else(|| invalid_binding("extract"))?;
    Ok(FileManagerExtractOptions {
        overwrite: options.existing_output,
        symlinks: options.symlinks,
        encoding: match &options.encoding {
            EntryNameEncoding::Auto => None,
            EntryNameEncoding::Named { label } => Some(label.clone()),
        },
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::{
        ByteSize, EntryNameEncoding, NamedPreset, OverwritePolicy, PresetCompressionLevel,
        PresetDocument, PresetId, PresetLabel, PresetStore, SymlinkPolicy, VolumeMode,
    };

    use super::{
        file_manager_create_options as resolve_create_options,
        file_manager_extract_options as resolve_extract_options,
    };

    #[test]
    fn external_action_wire_names_match_desktop_and_cli_arguments() {
        use super::ExternalTaskAction::*;
        for (action, name) in [
            (Checksum, "checksum"),
            (ExtractHere, "extract-here"),
            (ExtractToFolder, "extract-to-folder"),
            (ExtractSfx, "extract-sfx"),
            (CompressTo7z, "compress-to-7z"),
            (TestArchive, "test-archive"),
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<super::ExternalTaskAction>(serde_json::json!(name))
                    .unwrap(),
                action
            );
            if action != ExtractSfx {
                assert_eq!(name.parse::<super::ExternalTaskAction>().unwrap(), action);
            }
        }
        assert!(serde_json::from_str::<super::ExternalTaskAction>("\"compress-to7z\"").is_err());
    }

    #[test]
    fn create_selection_preserves_names_and_avoids_split_sets_and_symlinks() {
        use crate::{api::FormatRegistry, Engine, ExternalTaskAction, ExternalTaskPlan};
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("-中文 notes.txt");
        fs::write(&input, b"source").unwrap();
        let engine = Engine::new(FormatRegistry::new());
        let mut document = PresetDocument::seeded();
        let id = document.bindings.file_manager_create.clone().unwrap();
        let preset = document
            .presets
            .iter_mut()
            .find(|preset| preset.id() == &id)
            .unwrap();
        let NamedPreset::Create { options, .. } = preset else {
            panic!("create binding")
        };
        options.volumes = VolumeMode::Split {
            size_bytes: ByteSize::new(512 * 1024),
        };
        fs::write(root.path().join("-中文 notes.7z.002"), b"existing volume").unwrap();
        let ExternalTaskPlan::Compress { output, inputs, .. } = engine
            .resolve_external_task(
                &document,
                ExternalTaskAction::CompressTo7z,
                vec![input.clone()],
                None,
            )
            .unwrap()
            .unwrap()
        else {
            panic!("compress plan")
        };
        assert_eq!(inputs, std::slice::from_ref(&input));
        assert_eq!(output, root.path().join("-中文 notes (2).7z"));
        assert_eq!(
            fs::read(root.path().join("-中文 notes.7z.002")).unwrap(),
            b"existing volume"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("missing"), &output).unwrap();
            let ExternalTaskPlan::Compress { output, .. } = engine
                .resolve_external_task(
                    &document,
                    ExternalTaskAction::CompressTo7z,
                    vec![input],
                    None,
                )
                .unwrap()
                .unwrap()
            else {
                panic!("compress plan")
            };
            assert_eq!(output, root.path().join("-中文 notes (3).7z"));
        }
        // macOS filesystems reject non-UTF8 names before the planner is called.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::os::unix::ffi::{OsStrExt, OsStringExt};
            let input = root
                .path()
                .join(std::ffi::OsString::from_vec(b"source-\xff.txt".to_vec()));
            fs::write(&input, b"non-UTF8 source").unwrap();
            let ExternalTaskPlan::Compress { output, .. } = engine
                .resolve_external_task(
                    &document,
                    ExternalTaskAction::CompressTo7z,
                    vec![input],
                    None,
                )
                .unwrap()
                .unwrap()
            else {
                panic!("compress plan")
            };
            assert_eq!(output.file_name().unwrap().as_bytes(), b"source-\xff.7z");
        }
    }

    #[test]
    fn archive_selection_filters_directories_and_preserves_all_files() {
        use crate::{api::FormatRegistry, Engine, ExternalTaskAction, ExternalTaskPlan};
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.zip");
        let second = root.path().join("second.7z");
        fs::write(&first, b"archive").unwrap();
        fs::write(&second, b"archive").unwrap();
        let engine = Engine::new(FormatRegistry::new());
        let document = PresetDocument::seeded();
        let inputs = vec![
            first.clone(),
            root.path().to_owned(),
            second.clone(),
            root.path().join("missing"),
        ];
        let ExternalTaskPlan::Test { paths } = engine
            .resolve_external_task(
                &document,
                ExternalTaskAction::TestArchive,
                inputs.clone(),
                None,
            )
            .unwrap()
            .unwrap()
        else {
            panic!("test plan")
        };
        assert_eq!(paths, [first.clone(), second.clone()]);
        let ExternalTaskPlan::Extract { items, smart, .. } = engine
            .resolve_external_task(&document, ExternalTaskAction::ExtractHere, inputs, None)
            .unwrap()
            .unwrap()
        else {
            panic!("extract plan")
        };
        assert!(smart);
        assert_eq!(
            items,
            [
                (first, root.path().to_owned()),
                (second, root.path().to_owned())
            ]
        );
        assert!(engine
            .resolve_external_task(
                &document,
                ExternalTaskAction::TestArchive,
                vec![root.path().to_owned()],
                None
            )
            .unwrap()
            .is_none());
    }

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
        assert!(matches!(extract.overwrite, OverwritePolicy::Ask));
        assert!(extract.encoding.is_none());
        assert!(matches!(extract.symlinks, SymlinkPolicy::Preserve));
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
        extract_options.existing_output = OverwritePolicy::RenameBoth;
        extract_options.symlinks = SymlinkPolicy::Skip;
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
        assert_eq!(
            create.content_policy.resolve_excludes(&create.excludes),
            ["*.tmp", ".git"]
        );
        assert!(create.test_after_create);

        let Ok(extract) = resolve_extract_options(&document) else {
            panic!("bound extract preset should resolve")
        };
        assert!(matches!(extract.overwrite, OverwritePolicy::RenameBoth));
        assert_eq!(extract.encoding.as_deref(), Some("shift_jis"));
        assert!(matches!(extract.symlinks, SymlinkPolicy::Skip));
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

        assert!(
            PresetStore::new(&path).load().is_err(),
            "malformed preset must fail"
        );

        fs::remove_dir_all(root).expect("remove fixture directory");
    }
}
