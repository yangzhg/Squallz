//! Integrity-test policy for native entry streams.

use std::io::Read;

use squallz_format_api::{
    test_entry_data, ControlToken, EntryMeta, FormatError, LimitsAccountant, ProgressSink,
};

/// Read through decoder EOF and record damage while preserving hard failures.
pub(crate) fn test_entry_with_problems(
    data: Result<impl Read, FormatError>,
    meta: &EntryMeta,
    accountant: &mut LimitsAccountant,
    total: u64,
    progress: &dyn ProgressSink,
    control: &ControlToken,
    mut record_problem: impl FnMut(String),
) -> Result<(), FormatError> {
    let result = match data {
        Ok(mut data) => test_entry_data(&mut data, meta, accountant, total, progress, control)
            .map_err(|error| match error {
                // Keep read diagnostics distinct from errors opening the entry.
                FormatError::Io(error) => FormatError::Other(error.to_string()),
                error => error,
            }),
        Err(error) => {
            progress.on_progress(accountant.output_bytes(), total, &meta.path);
            Err(error)
        }
    };
    match result {
        Ok(()) => {}
        Err(
            error @ (FormatError::Cancelled
            | FormatError::ResourceLimitExceeded(_)
            | FormatError::PasswordRequired
            | FormatError::WrongPassword),
        ) => return Err(error),
        Err(error) => record_problem(format!("{}: {error}", meta.path)),
    }
    Ok(())
}
