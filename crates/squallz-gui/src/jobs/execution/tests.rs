use super::super::test_support::{compress_file_job, temp_dir};
use super::*;

#[test]
fn extract_result_keeps_problem_preview_separate_from_core_counts() {
    let destination = PathBuf::from("output/archive");
    let result = extract_result_json(
        ExtractPlan {
            requested_destination: PathBuf::from("output"),
            destination: destination.clone(),
            layout: squallz_core::SmartLayout::WrapInFolder,
            scope: squallz_core::ExtractScope {
                entries: 9,
                files: 5,
                directories: 1,
                symlinks: 1,
                hardlinks: 1,
                other: 1,
                total_bytes: 8192,
            },
            estimated_conflicts: 4,
        },
        ExtractReport {
            destination: destination.clone(),
            selected_entries: 9,
            created: 2,
            directories: 1,
            skipped: 3,
            replaced: 1,
            renamed: 1,
            failed: 1,
            output_bytes: 4096,
        },
        ArchiveStructureStatus::Complete,
        true,
        ProblemPreview {
            total: 1,
            messages: vec!["broken.txt: invalid data".to_owned()],
        },
    );

    assert_eq!(result["dest"], destination.to_string_lossy().as_ref());
    assert!(result.get("skipped").is_none());
    assert_eq!(result["problems"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["problems_total"], 1);
    assert_eq!(result["problems_truncated"], false);
    assert_eq!(result["plan"]["estimated_conflicts"], 4);
    assert_eq!(result["counts"]["destination"], result["dest"]);
    assert_eq!(result["counts"]["selected_entries"], 9);
    assert_eq!(result["counts"]["created"], 2);
    assert_eq!(result["counts"]["directories"], 1);
    assert_eq!(result["counts"]["skipped"], 3);
    assert_eq!(result["counts"]["replaced"], 1);
    assert_eq!(result["counts"]["renamed"], 1);
    assert_eq!(result["counts"]["failed"], 1);
    assert_eq!(result["counts"]["output_bytes"], 4096);
}

#[test]
fn create_job_request_is_shared_for_plan_and_worker_options() {
    let spec = JobSpec::Compress {
        inputs: vec!["source-a".into(), "source-b".into()],
        dest: "archive.sqz".into(),
        level: 8,
        password: Some("secret".into()),
        encrypt_names: true,
        split_size: Some(32 * 1024),
        split_mode: squallz_core::api::SplitOutputMode::Generic,
        excludes: vec!["*.tmp".into()],
        content_policy: squallz_core::CreateContentPolicy::CrossPlatformClean,
        sqz_inner_format: Some(squallz_core::SqzInnerFormat::Zip),
        sfx_target: None,
        completion: squallz_core::CreateCompletionAction::None,
        post_success: PostSuccessAction::KeepSource,
        test_after_create: true,
        replace_existing: false,
        replacement_guard: None,
    };
    let settings = SettingsDto {
        performance_threads: Some(3),
        performance_memory_limit_bytes: Some(32 * 1024),
        ..SettingsDto::default()
    };

    let request = create_job_request(&spec, &settings).unwrap();
    assert_eq!(
        request.inputs,
        vec![PathBuf::from("source-a"), PathBuf::from("source-b")]
    );
    assert_eq!(request.dest, PathBuf::from("archive.sqz"));
    assert_eq!(request.options.level, CompressionLevel::from_numeric(8));
    assert!(request.options.password.is_some());
    assert!(request.options.encrypt_filenames);
    assert_eq!(request.options.split_size, Some(32 * 1024));
    assert_eq!(
        request.options.excludes,
        vec![".DS_Store", "._*", "__MACOSX", "*.tmp"]
    );
    assert_eq!(
        request.options.sqz.inner_format,
        squallz_core::SqzInnerFormat::Zip
    );
    assert_eq!(request.options.resources.threads, Some(3));
    assert_eq!(request.options.resources.memory_limit, Some(32 * 1024));
    assert!(request.sfx_options().is_none());
    assert_eq!(request.post_success, PostSuccessAction::KeepSource);
    assert!(request.test_after_create);
    assert!(!request.replace_existing);
    assert_eq!(request.commit_policy, CreateCommitPolicy::NoReplace);

    let not_create = JobSpec::Test {
        path: "archive.zip".into(),
        encoding: None,
        password: None,
    };
    assert!(matches!(
        create_job_request(&not_create, &SettingsDto::default()),
        Err(FormatError::Unsupported(_))
    ));
}

#[test]
fn convert_create_options_are_shared_for_plan_and_worker() {
    let spec = JobSpec::Convert {
        src: "source.zip".into(),
        dest: "converted.7z".into(),
        level: 8,
        src_encoding: Some("GBK".into()),
        src_password: Some("source secret".into()),
        dest_password: Some("destination secret".into()),
        encrypt_names: true,
        split_size: Some(256 * 1024),
        split_mode: squallz_core::api::SplitOutputMode::Generic,
        replace_existing: false,
        replacement_guard: None,
    };
    let settings = SettingsDto {
        performance_threads: Some(3),
        performance_memory_limit_bytes: Some(32 * 1024),
        ..SettingsDto::default()
    };

    let options = convert_create_options(&spec, &settings).unwrap();
    assert_eq!(options.level, CompressionLevel::from_numeric(8));
    assert!(options.password.is_some());
    assert!(options.encrypt_filenames);
    assert_eq!(options.split_size, Some(256 * 1024));
    assert_eq!(options.resources.threads, Some(3));
    assert_eq!(options.resources.memory_limit, Some(32 * 1024));

    let not_convert = JobSpec::Test {
        path: "archive.zip".into(),
        encoding: None,
        password: None,
    };
    assert!(matches!(
        convert_create_options(&not_convert, &SettingsDto::default()),
        Err(FormatError::Unsupported(_))
    ));
}

#[test]
fn create_job_request_preserves_the_confirmed_replacement_guard() {
    let dir = temp_dir("create-request-replacement-guard");
    let destination = dir.join("archive.zip");
    std::fs::write(&destination, b"existing archive").unwrap();
    let guard = squallz_core::inspect_create_destination(
        &destination,
        squallz_core::CreateArtifactKind::Archive,
    )
    .unwrap()
    .guard
    .unwrap();
    let mut spec = compress_file_job(Path::new("source.txt"), &destination);
    let JobSpec::Compress {
        replace_existing,
        replacement_guard,
        ..
    } = &mut spec
    else {
        unreachable!();
    };
    *replace_existing = true;
    *replacement_guard = Some(guard);

    let request = create_job_request(&spec, &SettingsDto::default()).unwrap();

    assert_eq!(
        request.commit_policy,
        CreateCommitPolicy::ReplaceIfUnchanged(guard)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn create_job_request_rejects_a_guard_on_a_no_replace_job() {
    let dir = temp_dir("create-request-invalid-guard");
    let destination = dir.join("archive.zip");
    std::fs::write(&destination, b"existing archive").unwrap();
    let guard = squallz_core::inspect_create_destination(
        &destination,
        squallz_core::CreateArtifactKind::Archive,
    )
    .unwrap()
    .guard
    .unwrap();
    let mut spec = compress_file_job(Path::new("source.txt"), &destination);
    let JobSpec::Compress {
        replace_existing,
        replacement_guard,
        ..
    } = &mut spec
    else {
        unreachable!();
    };
    *replace_existing = false;
    *replacement_guard = Some(guard);

    assert!(matches!(
        create_job_request(&spec, &SettingsDto::default()),
        Err(FormatError::Unsupported(_))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn output_jobs_require_a_guard_for_replacement() {
    for operation in ["create", "convert", "export"] {
        assert_eq!(
            job_output_commit_policy(false, None, operation).unwrap(),
            CreateCommitPolicy::NoReplace
        );
        let error = job_output_commit_policy(true, None, operation).unwrap_err();
        assert!(matches!(
            error,
            FormatError::Unsupported(detail)
                if detail.contains(operation) && detail.contains("destination guard")
        ));
    }
}

#[test]
fn create_result_reports_preserved_split_outputs_for_review() {
    let primary = PathBuf::from("新归档.zip.001");
    let preserved = vec![
        PathBuf::from(".新归档.zip.001.split-backup-7"),
        PathBuf::from(".新归档.zip.002.split-backup-7"),
    ];
    let result = create_report_result(
        CreateReport {
            primary_output: primary.clone(),
            outputs: vec![primary.clone()],
            preserved_outputs: preserved.clone(),
            total_output_bytes: 42,
            split_volume_count: Some(1),
        },
        "create",
        SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, 1),
        None,
    );

    assert_eq!(
        result["primary_output"].as_str(),
        Some(primary.to_string_lossy().as_ref())
    );
    assert_eq!(
        result["preserved_outputs"],
        serde_json::json!([
            preserved[0].to_string_lossy(),
            preserved[1].to_string_lossy(),
        ])
    );
    assert_eq!(result["split"], true);
    assert_eq!(result["tested_after_create"], false);
    assert!(result["entries_tested_after_create"].is_null());
}

#[test]
fn sfx_result_reports_preserved_previous_outputs_for_the_shared_warning_ui() {
    let primary = PathBuf::from("Installer.app");
    let preserved = PathBuf::from(".squallz-sfx-a18e9f52-7-1/previous");
    let result = sfx_report_result(
        SfxBuildReport {
            path: primary.clone(),
            target: SfxTarget::Macos,
            layout: squallz_core::SfxLayout::MacosApp,
            stub_bytes: 12,
            payload_bytes: 30,
            total_bytes: 42,
            payload_crc32: 0,
            payload_sha256: Some([7; 32]),
            requires_signing: true,
            preserved_outputs: vec![preserved.clone()],
        },
        SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, 1),
        Some(7),
    );

    assert_eq!(
        result["primary_output"].as_str(),
        Some(primary.to_string_lossy().as_ref())
    );
    assert_eq!(
        result["preserved_outputs"],
        serde_json::json!([preserved.to_string_lossy()])
    );
    assert_eq!(result["operation"], "create_sfx");
    assert_eq!(result["requires_signing"], true);
    assert_eq!(result["tested_after_create"], true);
    assert_eq!(result["entries_tested_after_create"], 7);
}

#[test]
fn batch_extract_collapses_only_confirmed_volume_members() {
    let part1 = PathBuf::from("/archives/sample.part1.rar");
    let part2 = PathBuf::from("/archives/sample.part2.rar");
    let part3 = PathBuf::from("/archives/sample.part3.rar");
    let zip = PathBuf::from("/archives/other.zip");
    let source_set =
        ArchiveSourceSet::from_ordered_members(vec![part1.clone(), part2.clone(), part3.clone()])
            .unwrap();
    let make_item = |path: &Path, dest: &str| BatchExtractItem {
        path: path.to_string_lossy().into_owned(),
        dest: dest.to_owned(),
        encoding: None,
        password: None,
        best_effort: false,
    };
    let items = vec![
        make_item(&part3, "/output/part3"),
        make_item(&zip, "/output/zip"),
        make_item(&part1, "/output/part1"),
        make_item(&part2, "/output/part2"),
    ];
    let display_items = vec![
        make_item(Path::new("shown-part3.rar"), "/output/part3"),
        make_item(Path::new("shown-other.zip"), "/output/zip"),
        make_item(Path::new("shown-part1.rar"), "/output/part1"),
        make_item(Path::new("shown-part2.rar"), "/output/part2"),
    ];

    let normalized = normalize_batch_extract_items_with(&items, &display_items, |path| {
        if source_set.members().iter().any(|member| member == path) {
            Ok(Some(source_set.clone()))
        } else {
            Ok(None)
        }
    });

    assert_eq!(normalized.len(), 2);
    assert_eq!(normalized[0].execution.path, part1.to_string_lossy());
    assert_eq!(normalized[0].execution.dest, "/output/part1");
    assert_eq!(normalized[0].display.path, "shown-part1.rar");
    assert_eq!(normalized[1].execution.path, zip.to_string_lossy());
}

#[test]
fn batch_extract_keeps_candidates_separate_when_source_probe_fails() {
    let make_item = |path: &str| BatchExtractItem {
        path: path.to_owned(),
        dest: format!("/output/{path}"),
        encoding: None,
        password: None,
        best_effort: false,
    };
    let items = vec![make_item("sample.part1.rar"), make_item("sample.part2.rar")];

    let normalized = normalize_batch_extract_items_with(&items, &items, |_| {
        Err(FormatError::CorruptArchive(
            "source set is not confirmed".into(),
        ))
    });

    assert_eq!(normalized.len(), 2);
    assert_eq!(normalized[0].execution.path, "sample.part1.rar");
    assert_eq!(normalized[1].execution.path, "sample.part2.rar");
}

#[test]
fn batch_extract_prefers_a_selected_primary_after_native_volume_collapse() {
    let first = PathBuf::from("/archives/sample.z01");
    let second = PathBuf::from("/archives/sample.z02");
    let primary = PathBuf::from("/archives/sample.zip");
    let source_set = ArchiveSourceSet::from_primary_and_ordered_members(
        primary.clone(),
        vec![first.clone(), second.clone(), primary.clone()],
    )
    .unwrap();
    let make_item = |path: &Path| BatchExtractItem {
        path: path.to_string_lossy().into_owned(),
        dest: format!("/output/{}", path.file_name().unwrap().to_string_lossy()),
        encoding: None,
        password: None,
        best_effort: false,
    };
    let items = vec![make_item(&second), make_item(&primary), make_item(&first)];

    let normalized = normalize_batch_extract_items_with(&items, &items, |path| {
        if source_set.members().iter().any(|member| member == path) {
            Ok(Some(source_set.clone()))
        } else {
            Ok(None)
        }
    });

    assert_eq!(normalized.len(), 1);
    assert_eq!(normalized[0].execution.path, primary.to_string_lossy());
    assert_eq!(normalized[0].execution.dest, "/output/sample.zip");
}

#[test]
fn failed_recovery_report_keeps_metrics_for_the_result_ui() {
    let report = squallz_recovery::RecoveryReport {
        ok: false,
        operation: "verify",
        archive: PathBuf::from("damaged.zip"),
        recovery: PathBuf::from("damaged.zip.par2"),
        outputs: Vec::new(),
        output: None,
        tool: PathBuf::from("rust-par2"),
        redundancy_percent: None,
        source_file_count: 1,
        status_code: None,
        metrics: Some(squallz_recovery::RecoveryMetrics {
            all_correct: false,
            repair_possible: true,
            blocks_needed: 3,
            recovery_blocks_available: 4,
            blocks_repaired: None,
            files_repaired: None,
            no_damage: false,
        }),
        stdout: "repair_possible=true".to_owned(),
        stderr: "damage found".to_owned(),
    };

    let result = recovery_report_json(report).unwrap().unwrap();
    assert_eq!(result["ok"].as_bool(), Some(false));
    assert_eq!(result["metrics"]["repair_possible"].as_bool(), Some(true));
    assert_eq!(result["metrics"]["blocks_needed"].as_u64(), Some(3));
    assert_eq!(
        result["metrics"]["recovery_blocks_available"].as_u64(),
        Some(4)
    );
}

/// Selection expansion: directories by prefix, files exactly.
#[test]
fn selection_expansion() {
    let metas: Vec<EntryMeta> = ["a/x.txt", "a/y.txt", "b/z.txt", "top.txt"]
        .iter()
        .map(|n| EntryMeta {
            path: EntryPath::from_utf8(*n),
            entry_type: squallz_core::api::EntryType::File,
            size: 1,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        })
        .collect();
    let control = ControlToken::new();
    let selection = ["a/".to_owned(), "top.txt".to_owned()];
    let sel = expand_selection_with_control(&metas, &selection, &control).unwrap();
    let names: Vec<&str> = sel.iter().map(|p| p.display.as_str()).collect();
    assert_eq!(names, vec!["a/x.txt", "a/y.txt", "top.txt"]);
    control.cancel();
    assert!(matches!(
        expand_selection_with_control(&metas, &selection, &control),
        Err(FormatError::Cancelled)
    ));
}
