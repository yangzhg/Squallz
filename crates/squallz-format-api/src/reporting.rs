//! Paths in reports use the same lossy display text on CLI and desktop surfaces.

use serde::Serializer;
use std::path::Path;

pub fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}
