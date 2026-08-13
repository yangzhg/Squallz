use std::ffi::{OsStr, OsString};

use serde::{Deserialize, Serialize};
use squallz_format_api::FormatError;

/// Lossless, platform-tagged `OsString` representation shared by private
/// journals across the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "units", rename_all = "snake_case")]
pub enum StoredOsString {
    Unix(Vec<u8>),
    Windows(Vec<u16>),
    Utf8(String),
}

impl StoredOsString {
    pub fn from_os_str(value: &OsStr) -> Result<Self, FormatError> {
        Self::from_os_string(value.to_os_string())
    }

    pub fn from_path(value: &std::path::Path) -> Result<Self, FormatError> {
        Self::from_os_str(value.as_os_str())
    }

    #[cfg(unix)]
    pub fn from_os_string(value: OsString) -> Result<Self, FormatError> {
        use std::os::unix::ffi::OsStringExt;

        let bytes = value.into_vec();
        if bytes.contains(&0) {
            return Err(FormatError::Unsupported(
                "stored path contains a null byte".into(),
            ));
        }
        Ok(Self::Unix(bytes))
    }

    #[cfg(windows)]
    pub fn from_os_string(value: OsString) -> Result<Self, FormatError> {
        use std::os::windows::ffi::OsStrExt;

        let units = value.encode_wide().collect::<Vec<_>>();
        if units.contains(&0) {
            return Err(FormatError::Unsupported(
                "stored path contains a null unit".into(),
            ));
        }
        Ok(Self::Windows(units))
    }

    #[cfg(not(any(unix, windows)))]
    pub fn from_os_string(value: OsString) -> Result<Self, FormatError> {
        value.into_string().map(Self::Utf8).map_err(|_| {
            FormatError::Unsupported("stored paths must be UTF-8 on this platform".into())
        })
    }

    #[cfg(unix)]
    pub fn to_os_string(&self) -> Result<OsString, FormatError> {
        use std::os::unix::ffi::OsStringExt;

        match self {
            Self::Unix(bytes) if !bytes.contains(&0) => Ok(OsString::from_vec(bytes.clone())),
            Self::Unix(_) => Err(FormatError::Unsupported(
                "stored path contains a null byte".into(),
            )),
            _ => Err(FormatError::Unsupported(
                "stored path belongs to another platform".into(),
            )),
        }
    }

    #[cfg(windows)]
    pub fn to_os_string(&self) -> Result<OsString, FormatError> {
        use std::os::windows::ffi::OsStringExt;

        match self {
            Self::Windows(units) if !units.contains(&0) => Ok(OsString::from_wide(units)),
            Self::Windows(_) => Err(FormatError::Unsupported(
                "stored path contains a null unit".into(),
            )),
            _ => Err(FormatError::Unsupported(
                "stored path belongs to another platform".into(),
            )),
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn to_os_string(&self) -> Result<OsString, FormatError> {
        match self {
            Self::Utf8(value) if !value.as_bytes().contains(&0) => Ok(OsString::from(value)),
            Self::Utf8(_) => Err(FormatError::Unsupported(
                "stored path contains a null byte".into(),
            )),
            _ => Err(FormatError::Unsupported(
                "stored path belongs to another platform".into(),
            )),
        }
    }

    pub fn into_path_buf(self) -> Result<std::path::PathBuf, FormatError> {
        self.to_os_string().map(std::path::PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_os_string_round_trips_the_current_platform() {
        let value = OsString::from("archive.sqz.001");
        let stored = StoredOsString::from_os_string(value.clone()).unwrap();
        assert_eq!(stored.to_os_string().unwrap(), value);
    }
}
