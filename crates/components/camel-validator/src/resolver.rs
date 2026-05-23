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

impl ResourceResolver for FilesystemResolver {
    fn resolve(&self, path: &str) -> Result<Vec<u8>, CamelError> {
        std::fs::read(path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!("failed to read schema file '{path}': {e}"))
        })
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
}
