//! Internal utility functions for route DSL file handling.

use std::fs;
use std::path::Path;

use camel_api::CamelError;

/// Maximum size for individual route files (YAML/JSON) loaded via file-based parsers.
/// Prevents OOM from abnormally large files.
pub(crate) const MAX_ROUTE_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// Read a file with a size cap. Stats first, rejects if too large.
pub(crate) fn read_route_file_capped(path: &Path) -> Result<String, CamelError> {
    let metadata = fs::metadata(path).map_err(|e| {
        CamelError::Io(format!(
            "Failed to read metadata of {}: {e}",
            path.display()
        ))
    })?;
    if metadata.len() > MAX_ROUTE_FILE_SIZE {
        return Err(CamelError::Io(format!(
            "Route file `{}` is {} bytes, exceeds max {} bytes",
            path.display(),
            metadata.len(),
            MAX_ROUTE_FILE_SIZE
        )));
    }
    fs::read_to_string(path)
        .map_err(|e| CamelError::Io(format!("Failed to read {}: {e}", path.display())))
}
