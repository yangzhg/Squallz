use std::io::{self, Read};
use std::path::Path;

use crate::api::{
    ControlToken, EntryMeta, EntryPath, FormatError, LimitsAccountant, OpenOptions, SafetyLimits,
};
use crate::controlled_io::controlled_result;
use crate::Engine;

impl Engine {
    /// Reads an entry and reports whether this read verified the supplied
    /// password. Only a complete, unambiguous encrypted entry is proof;
    /// successful prefix reads and unencrypted entries return `false`.
    pub fn read_entry_verifying_password(
        &self,
        path: &Path,
        entry: &EntryPath,
        opts: &OpenOptions,
        control: &ControlToken,
        consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<bool, FormatError> {
        let mut archive = self.open_with_control(path, opts, control)?;
        let mut matches = 0;
        let mut encrypted = false;
        if opts.password.is_some() {
            for candidate in archive.entries() {
                control.checkpoint()?;
                let candidate = candidate?;
                if candidate.path.raw == entry.raw || candidate.path.display == entry.display {
                    matches += 1;
                    encrypted = candidate.encrypted;
                    if matches > 1 {
                        break;
                    }
                }
            }
        }
        let mut reached_end = false;
        let result = archive.read_entry(entry, &mut |reader| {
            let mut reader = EntryReadTracker {
                reader,
                control,
                reached_end: false,
            };
            consume(&mut reader)?;
            reached_end = reader.reached_end;
            Ok(())
        });
        controlled_result(control, result)?;
        Ok(matches == 1 && encrypted && reached_end)
    }

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

struct EntryReadTracker<'a> {
    reader: &'a mut dyn Read,
    control: &'a ControlToken,
    reached_end: bool,
}

impl Read for EntryReadTracker<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.control.checkpoint().map_err(io::Error::other)?;
        let length = buffer.len().min(64 * 1024);
        let count = self.reader.read(&mut buffer[..length])?;
        self.reached_end |= count == 0;
        Ok(count)
    }
}
