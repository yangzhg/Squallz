//! Native opening is restricted to completed outputs in the requesting window's queue.

use std::path::PathBuf;

use super::{JobManager, JobStateSnapshot};
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
    let (path, directory) = match &snapshot.spec {
        JobSpec::Extract { .. } | JobSpec::ExtractNested { .. } => (result["dest"].as_str(), true),
        JobSpec::BatchExtract { .. } => (result["outputs"][0]["dest"].as_str(), true),
        JobSpec::PublishMacosSfx { .. } => (result["primary_output"].as_str(), true),
        JobSpec::Convert { .. } if result["split"].as_bool() == Some(false) => {
            (result["primary_output"].as_str(), false)
        }
        JobSpec::ExportSqz { .. } | JobSpec::RepairSqz { .. } | JobSpec::RepairZip { .. } => {
            (result["dest"].as_str(), false)
        }
        JobSpec::RepairRecovery {
            output_directory, ..
        } if result["ok"].as_bool() == Some(true) => (
            result["output"]
                .as_str()
                .or_else(|| result["archive"].as_str()),
            *output_directory,
        ),
        JobSpec::Update { .. } => (result["archive"].as_str(), false),
        _ => return Err(unavailable()),
    };
    let path = PathBuf::from(
        path.filter(|path| !path.is_empty())
            .ok_or_else(unavailable)?,
    );
    let metadata = std::fs::metadata(&path).map_err(|_| unavailable())?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(unavailable());
    }
    Ok(path)
}

#[cfg(test)]
mod tests;
