//! Desktop integrations share the GUI's resolved action and preset snapshot.

use std::path::PathBuf;

use squallz_core::{
    ChecksumAlgorithm, ExternalTaskAction, ExternalTaskPlan, OverwritePolicy, SymlinkPolicy,
};

use super::{checksum, compress, extract, preset, test, Ctx};
use crate::args::{OverwriteArg, SymlinkArg};
use crate::errors::CliError;

pub(super) fn run(
    ctx: &Ctx,
    action: ExternalTaskAction,
    paths: Vec<PathBuf>,
    json: bool,
) -> Result<(), CliError> {
    let (_, document) = preset::load_document(ctx)?;
    let Some(plan) = ctx
        .engine
        .resolve_external_task(&document, action, paths, None)?
    else {
        return Ok(());
    };
    match plan {
        ExternalTaskPlan::Compress {
            inputs,
            output,
            options,
        } => compress::run_file_manager(ctx, inputs, output, options, json),
        ExternalTaskPlan::Checksum { inputs } => checksum::run(
            ctx,
            inputs,
            ChecksumAlgorithm::Sha256,
            None,
            Vec::new(),
            json,
        ),
        ExternalTaskPlan::Test { paths } => {
            for path in paths {
                test::run(ctx, path, None, None, Default::default(), json)?;
            }
            Ok(())
        }
        ExternalTaskPlan::Extract {
            items,
            options,
            smart,
            verify_sfx,
        } => {
            if verify_sfx {
                return Err(squallz_core::api::FormatError::Unsupported(
                    "SFX extraction requires the desktop task entry".into(),
                )
                .into());
            }
            for (path, dest) in items {
                extract::run(
                    ctx,
                    extract::CliExtractRequest {
                        source: extract::CliExtractSource::Archive(path),
                        dest: Some(dest),
                        includes: Vec::new(),
                        overwrite: match options.overwrite {
                            OverwritePolicy::Ask => OverwriteArg::Ask,
                            OverwritePolicy::Skip => OverwriteArg::Skip,
                            OverwritePolicy::Overwrite => OverwriteArg::All,
                            OverwritePolicy::RenameBoth => OverwriteArg::Rename,
                        },
                        symlinks: match options.symlinks {
                            SymlinkPolicy::Preserve => SymlinkArg::Preserve,
                            SymlinkPolicy::Skip => SymlinkArg::Skip,
                            SymlinkPolicy::Follow => SymlinkArg::Follow,
                        },
                        password: None,
                        encoding: options.encoding.clone(),
                        smart,
                        best_effort: false,
                        resources: Default::default(),
                        limits: Default::default(),
                        json_output: json,
                    },
                )?;
            }
            Ok(())
        }
    }
}
