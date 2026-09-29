use std::path::Path;

use crate::api::{
    ControlToken, EntryMeta, FormatError, LimitsAccountant, OpenOptions, SafetyLimits,
};
use crate::controlled_io::controlled_result;
use crate::Engine;

impl Engine {
    /// Verifies a supplied password against encrypted headers or one complete
    /// encrypted entry. A readable, unencrypted directory is not proof that a
    /// password works. Returns `false` when no unambiguous encrypted content
    /// can be used as proof.
    ///
    /// This is not a whole-archive integrity test: entries may use different
    /// passwords. Payload verification selects the smallest encrypted entry
    /// and discards its decoded bytes while applying the supplied limits.
    pub fn verify_password(
        &self,
        path: &Path,
        opts: &OpenOptions,
        limits: SafetyLimits,
        control: &ControlToken,
    ) -> Result<bool, FormatError> {
        control.checkpoint()?;
        if opts.password.is_none() {
            return Err(FormatError::PasswordRequired);
        }
        let source = self.inspect_archive_source_state(path, control)?;
        let without_password = OpenOptions {
            password: None,
            encoding_override: opts.encoding_override.clone(),
        };
        let encrypted_header = match self.open_with_control(path, &without_password, control) {
            Ok(_) => false,
            Err(FormatError::PasswordRequired | FormatError::WrongPassword) => true,
            Err(error) => return Err(error),
        };
        let mut opened = self.open_identified_with_control(path, opts, control)?;
        if opened.inspect_source_state(path, control)? != source {
            return Err(FormatError::input_changed());
        }
        let verified = if encrypted_header {
            true
        } else {
            let mut smallest: Option<EntryMeta> = None;
            for (index, entry) in opened.reader.entries().enumerate() {
                control.checkpoint()?;
                if index as u64 >= limits.max_entries {
                    return Err(FormatError::ResourceLimitExceeded(format!(
                        "archive contains more than {} entries",
                        limits.max_entries
                    )));
                }
                let entry = entry?;
                if entry.encrypted
                    && smallest
                        .as_ref()
                        .is_none_or(|other| entry.size < other.size)
                {
                    smallest = Some(entry);
                }
            }
            if let Some(entry) = smallest {
                // Readers may resolve repeated names to a different entry.
                // Revisit metadata without retaining a second path index;
                // both raw-name and display-name lookups must be unique.
                let mut matches = 0;
                for (index, candidate) in opened.reader.entries().enumerate() {
                    control.checkpoint()?;
                    if index as u64 >= limits.max_entries {
                        return Err(FormatError::ResourceLimitExceeded(format!(
                            "archive contains more than {} entries",
                            limits.max_entries
                        )));
                    }
                    let candidate = candidate?;
                    if candidate.path.raw == entry.path.raw
                        || candidate.path.display == entry.path.display
                    {
                        matches += 1;
                        if matches > 1 {
                            break;
                        }
                    }
                }
                if matches == 1 {
                    let mut accountant = LimitsAccountant::new(limits);
                    accountant.check_entry(&entry)?;
                    let mut decoded = 0;
                    let result = opened.reader.read_entry(&entry.path, &mut |reader| {
                        let mut buffer = [0_u8; 64 * 1024];
                        loop {
                            control.checkpoint()?;
                            let count = reader.read(&mut buffer)?;
                            if count == 0 {
                                break;
                            }
                            accountant.add_entry_output_bytes(
                                &entry,
                                &mut decoded,
                                count as u64,
                            )?;
                        }
                        Ok(())
                    });
                    controlled_result(control, result)?;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if opened.inspect_source_state(path, control)? != source {
            return Err(FormatError::input_changed());
        }
        control.checkpoint()?;
        Ok(verified)
    }
}
