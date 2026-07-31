mod common;

use std::fs;

use common::{engine, TempDir};
#[cfg(unix)]
use squallz_core::api::CompressionLevel;
use squallz_core::api::{ControlToken, CreateOptions, FormatError, NoProgress};

#[test]
fn wim_create_plan_reserves_staging_and_temporary_image() {
    let tmp = TempDir::new("create-plan-wim-temp");
    let input = tmp.path().join("payload.bin");
    fs::write(&input, vec![0x5a; 2 * 1024 * 1024])
        .unwrap_or_else(|error| panic!("failed to write WIM plan input: {error}"));
    let dest = tmp.path().join("archive.wim");

    let plan = engine()
        .plan_create(
            &dest,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
        )
        .unwrap_or_else(|error| panic!("failed to build WIM create plan: {error}"));
    let folded_temp = plan
        .workspace_budget_bytes
        .saturating_sub(plan.final_output_budget_bytes);
    let reserved_temp = folded_temp.saturating_add(plan.system_temp_budget_bytes);

    assert!(plan.archive_output_budget_bytes > plan.inputs.total_bytes);
    assert_eq!(
        plan.final_output_budget_bytes,
        plan.archive_output_budget_bytes
    );
    assert!(reserved_temp >= plan.archive_output_budget_bytes.saturating_mul(2));
}

#[test]
fn split_wim_requires_native_mode_before_scanning_or_writing() {
    let tmp = TempDir::new("create-plan-split-wim");
    let input = tmp.path().join("payload.bin");
    let dest = tmp.path().join("archive.swm");
    fs::write(&input, b"split WIM input").unwrap();
    let engine = engine();
    let opts = CreateOptions::default();

    let plan_error = engine
        .plan_create(&dest, std::slice::from_ref(&input), &opts)
        .unwrap_err();
    assert!(plan_error.is_split_wim_creation_unsupported());
    let explicit_first = tmp.path().join("archive.swm.001");
    let explicit_error = engine
        .plan_create(&explicit_first, std::slice::from_ref(&input), &opts)
        .unwrap_err();
    assert!(explicit_error.is_split_wim_creation_unsupported());

    let create_error = engine
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap_err();
    assert!(create_error.is_split_wim_creation_unsupported());
    assert!(!dest.exists());

    let missing_parent = tmp.path().join("missing-parent");
    let split_dest = missing_parent.join("archive.swm");
    let split_opts = CreateOptions {
        split_size: Some(1024),
        ..CreateOptions::default()
    };
    let split_error = engine
        .create(
            &split_dest,
            &[tmp.path().join("payload.bin")],
            &split_opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(split_error.is_split_wim_creation_unsupported());
    assert!(!missing_parent.exists());

    let native_opts = CreateOptions {
        split_size: Some(1024 * 1024),
        split_mode: squallz_core::api::SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let plan = engine
        .plan_create(
            &dest,
            std::slice::from_ref(&tmp.path().join("payload.bin")),
            &native_opts,
        )
        .unwrap();
    assert_eq!(plan.primary_output, dest);
    assert!(plan.split_volume_count_budget.is_some());

    let wrong_extension = tmp.path().join("archive.wim");
    let wrong_extension_error = engine
        .plan_create(
            &wrong_extension,
            std::slice::from_ref(&tmp.path().join("payload.bin")),
            &native_opts,
        )
        .unwrap_err();
    assert!(matches!(
        wrong_extension_error,
        FormatError::Unsupported(detail) if detail.contains(".swm")
    ));
}

#[cfg(unix)]
#[test]
fn create_plan_covers_long_zip_entry_names() {
    let tmp = TempDir::new("create-plan-long-names");
    let root = tmp.path().join("root");
    let mut nested = root.clone();
    for index in 0..3 {
        nested = nested.join(format!("dir-{index}-{}", "d".repeat(174)));
    }
    fs::create_dir_all(&nested).unwrap();
    for index in 0..3000 {
        let name = format!("{index:04}-{}", "f".repeat(195));
        fs::write(nested.join(name), []).unwrap();
    }

    let dest = tmp.path().join("long-names.zip");
    let opts = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };
    let engine = engine();
    let plan = engine
        .plan_create(&dest, std::slice::from_ref(&root), &opts)
        .unwrap();
    let generic_budget = plan.inputs.output_budget_bytes();
    let report = engine
        .create_with_report(&dest, &[root], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    assert!(report.total_output_bytes > generic_budget);
    assert!(plan.archive_output_budget_bytes >= report.total_output_bytes);
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
}
