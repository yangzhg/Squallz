//! Native opening is restricted to completed outputs in the requesting window's queue.

use std::path::PathBuf;

use super::{JobManager, JobResult, JobStateSnapshot};
use crate::dto::{ErrorDto, JobSpec};

impl JobManager {
    pub(crate) fn openable_output_for_window(
        &self,
        requester: &str,
        id: u64,
    ) -> Result<PathBuf, ErrorDto> {
        output_path(&self.snapshot_for_window(requester, id)?)
    }
}

fn unavailable() -> ErrorDto {
    ErrorDto::other("task output is unavailable")
}

fn output_path(snapshot: &JobStateSnapshot) -> Result<PathBuf, ErrorDto> {
    if snapshot.state != "done" {
        return Err(unavailable());
    }
    let result = snapshot.result.as_ref().ok_or_else(unavailable)?;
    let spec = snapshot.spec.as_ref().ok_or_else(unavailable)?;
    let (path, directory) = match (spec, result) {
        (JobSpec::Extract { .. }, JobResult::Extract(report))
        | (JobSpec::ExtractNested { .. }, JobResult::ExtractNested(report)) => {
            (PathBuf::from(&report.dest), true)
        }
        (JobSpec::BatchExtract { .. }, JobResult::BatchExtract(report)) => (
            PathBuf::from(&report.outputs.first().ok_or_else(unavailable)?.result.dest),
            true,
        ),
        (JobSpec::PublishMacosSfx { .. }, JobResult::PublishMacosSfx(report)) => {
            (PathBuf::from(&report.primary_output), true)
        }
        (JobSpec::Convert { .. }, JobResult::Convert(report)) if !report.output.split => {
            (PathBuf::from(&report.output.primary_output), false)
        }
        (JobSpec::ExportSqz { .. }, JobResult::ExportSqz(report)) => {
            (PathBuf::from(&report.primary_output), false)
        }
        (JobSpec::RepairSqz { .. }, JobResult::RepairSqz { dest, .. })
        | (JobSpec::RepairZip { .. }, JobResult::RepairZip { dest, .. }) => {
            (PathBuf::from(dest), false)
        }
        (
            JobSpec::RepairRecovery {
                output_directory, ..
            },
            JobResult::RepairRecovery(report),
        ) if report.ok => (
            report.output.as_ref().unwrap_or(&report.archive).clone(),
            *output_directory,
        ),
        (JobSpec::Update { .. }, JobResult::Update { archive, .. }) => {
            (PathBuf::from(archive), false)
        }
        _ => return Err(unavailable()),
    };
    if path.as_os_str().is_empty() {
        return Err(unavailable());
    }
    let metadata = std::fs::metadata(&path).map_err(|_| unavailable())?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(unavailable());
    }
    Ok(path)
}

#[cfg(test)]
mod tests;
