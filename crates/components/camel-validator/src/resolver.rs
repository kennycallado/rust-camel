//! Resource resolution for schema files.
//!
//! Currently only filesystem paths are supported.

use camel_component_api::CamelError;

// TODO(VAL-013): Resource resolution currently filesystem-only.
// Future: support classpath:, http:, and data: URIs.

/// Resolves schema resources. Currently only filesystem paths are supported.
/// TODO(VAL-013): Implement URL and classpath resolvers.
pub trait ResourceResolver: Send + Sync {
    /// Read the resource at `path` into bytes.
    fn resolve(&self, path: &str) -> Result<Vec<u8>, CamelError>;
}

/// Default filesystem-based resolver.
pub struct FilesystemResolver;

/// Maximum schema size read from disk (audit 2026-08-31, F4-6). Schemas are
/// operator config; the cap is defense-in-depth against a URI pointing at a
/// huge/unbounded file (e.g. a device file or a multi-GB log).
const MAX_SCHEMA_BYTES: u64 = 16 * 1024 * 1024;

impl ResourceResolver for FilesystemResolver {
    fn resolve(&self, path: &str) -> Result<Vec<u8>, CamelError> {
        use std::io::Read;
        // Re-review of F4-6: enforce the cap on the READ, not on metadata —
        // a file can grow between a stat check and fs::read (TOCTOU), and
        // device files report len 0. take(MAX+1) detects an over-cap file
        // instead of silently truncating it.
        let file = std::fs::File::open(path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!("failed to open schema file '{path}': {e}"))
        })?;
        let mut buf = Vec::new();
        file.take(MAX_SCHEMA_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!(
                    "failed to read schema file '{path}': {e}"
                ))
            })?;
        if buf.len() as u64 > MAX_SCHEMA_BYTES {
            return Err(CamelError::EndpointCreationFailed(format!(
                "schema file '{path}' exceeds {} bytes",
                MAX_SCHEMA_BYTES
            )));
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_resolver_reads_existing_file() {
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        use std::io::Write;
        f.write_all(b"hello").unwrap();
        let resolver = FilesystemResolver;
        let data = resolver.resolve(f.path().to_str().unwrap()).unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn filesystem_resolver_errors_on_missing_file() {
        let resolver = FilesystemResolver;
        let result = resolver.resolve("/nonexistent/file.json");
        assert!(result.is_err());
    }

    /// Re-review of F4-6: the size cap must be enforced on the read itself.
    /// A file grown past the cap after any metadata pre-check (or a device
    /// file reporting len 0) must be rejected, not read unbounded.
    #[test]
    fn filesystem_resolver_rejects_file_past_cap_on_read() {
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        // Extend past the cap without materializing content in the test body.
        f.as_file().set_len(super::MAX_SCHEMA_BYTES + 1).unwrap();
        let resolver = FilesystemResolver;
        let err = resolver
            .resolve(f.path().to_str().unwrap())
            .expect_err("over-cap file must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("exceeds"), "cap rejection: {msg}");
    }
}
