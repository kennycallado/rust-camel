//! Reserved document-suffix predicates (ADR-0062 single suffix rule).
//!
//! This is the single definition of the reserved-suffix rule, defined here
//! in `camel-api` (below the lint/runtime hex boundary) so that both
//! `camel-dsl` (runtime route discovery) and `camel-lint` consume one
//! definition. `camel_dsl::discovery` re-exports these predicates, so
//! existing `camel_dsl::discovery::*` import paths keep resolving to the
//! same functions.

use std::path::Path;

/// Returns true if the file name ends with the reserved `.test.yaml` or
/// `.test.yml` suffix. Such files name camel test documents (the
/// `camel test` family), not routes.
pub fn is_test_document(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with(".test.yaml") || name.ends_with(".test.yml")
    })
}

/// Returns true if the file name ends with the reserved `.job.yaml` or
/// `.job.yml` suffix. Such files name camel job documents (the
/// `camel job` family), not routes.
pub fn is_job_document(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with(".job.yaml") || name.ends_with(".job.yml")
    })
}

/// Returns true if the file name ends with any reserved document suffix:
/// `.test.yaml`/`.test.yml` (owned by `camel test`) or
/// `.job.yaml`/`.job.yml` (owned by `camel job`). Such files are never
/// routes; route discovery skips them under wildcard globs and errors on
/// literal naming.
pub fn is_reserved_document(path: &Path) -> bool {
    is_test_document(path) || is_job_document(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suffixes_match() {
        assert!(is_test_document(Path::new("a.test.yaml")));
        assert!(is_test_document(Path::new("a.test.yml")));
        assert!(is_job_document(Path::new("a.job.yaml")));
        assert!(is_job_document(Path::new("a.job.yml")));
        assert!(is_reserved_document(Path::new("a.test.yaml")));
        assert!(is_reserved_document(Path::new("a.test.yml")));
        assert!(is_reserved_document(Path::new("a.job.yaml")));
        assert!(is_reserved_document(Path::new("a.job.yml")));
    }

    #[test]
    fn non_reserved_names_rejected() {
        assert!(!is_test_document(Path::new("atest.yaml")));
        assert!(!is_job_document(Path::new("ajob.yaml")));
        assert!(!is_reserved_document(Path::new("atest.yaml")));
        assert!(!is_reserved_document(Path::new("ajob.yaml")));
        assert!(!is_test_document(Path::new("a.yaml")));
        assert!(!is_job_document(Path::new("a.yaml")));
        assert!(!is_reserved_document(Path::new("a.yaml")));
        assert!(!is_test_document(Path::new("x.test.json")));
        assert!(!is_job_document(Path::new("x.test.json")));
        assert!(!is_reserved_document(Path::new("x.test.json")));
        assert!(!is_test_document(Path::new("x.job.json")));
        assert!(!is_job_document(Path::new("x.job.json")));
        assert!(!is_reserved_document(Path::new("x.job.json")));
    }
}
