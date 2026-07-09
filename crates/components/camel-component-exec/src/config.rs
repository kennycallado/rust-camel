//! Configuration types for the exec component. Lives under
//! `[components.exec]` / `[[components.exec.profiles]]` in Camel.toml.

use std::collections::HashMap;
use std::path::PathBuf;

/// How args are validated against a profile. Applied **per argv element**.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, tag = "allow", rename_all = "lowercase")]
pub enum ArgPolicy {
    /// Every element accepted (explicit opt-in; operator-curated).
    Any,
    /// Every element must string-equal one of `values`.
    Exact { values: Vec<String> },
    /// Every element must byte-start-with one of `values`.
    Prefix { values: Vec<String> },
}

impl Default for ArgPolicy {
    fn default() -> Self {
        // Omitted `args` => deny all non-empty args (safest, fail-closed).
        // Represented as Exact with empty values: any non-empty arg fails.
        ArgPolicy::Exact { values: Vec::new() }
    }
}

/// Optional flag denylist; applied first regardless of `allow`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenyFlags(pub Vec<String>);

/// Environment policy for the child.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvPolicy {
    /// Var names the child may receive from the host env.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Explicit var=value pairs to set.
    #[serde(default)]
    pub set: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Materialized,
    /// Reserved for future use; validate() rejects at startup in v1.
    Stream,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum Sandbox {
    #[default]
    None,
    /// Reserved for future use; validate() rejects at startup in v1.
    WorkspaceRead,
    /// Reserved for future use; validate() rejects at startup in v1.
    WorkspaceWrite,
}

/// A single named execution profile.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecProfile {
    pub name: String,
    pub executable: String, // name (PATH-resolved) or absolute path
    #[serde(default)]
    pub args: ArgPolicy,
    #[serde(default)]
    pub deny_flags: DenyFlags,
    #[serde(default)]
    pub allow_shell: bool,
    #[serde(default)]
    pub env: EnvPolicy,
    #[serde(default = "default_dot")]
    pub working_dir: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_accepted")]
    pub accepted_exit_codes: Vec<i32>,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_stdin_cap")]
    pub stdin_max_bytes: usize,
    #[serde(default = "default_stdout_cap")]
    pub stdout_max_bytes: usize,
    #[serde(default = "default_stderr_cap")]
    pub stderr_max_bytes: usize,
    #[serde(default)]
    pub sandbox: Sandbox,
    #[serde(default)]
    pub output_mode: OutputMode,

    // Resolved at startup pinning (not deserialized):
    #[serde(skip)]
    pub canonical_executable: Option<PathBuf>,
}

fn default_dot() -> String {
    ".".into()
}
fn default_timeout() -> u64 {
    30
}
fn default_accepted() -> Vec<i32> {
    vec![0]
}
fn default_concurrency() -> usize {
    1
}
fn default_stdin_cap() -> usize {
    1_048_576 // 1 MiB
}
fn default_stdout_cap() -> usize {
    10_485_760 // 10 MiB
}
fn default_stderr_cap() -> usize {
    1_048_576 // 1 MiB
}

/// Global `[components.exec]` config.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecGlobalConfig {
    #[serde(default = "default_dot")]
    pub workspace_root: String,
    #[serde(default = "default_timeout")]
    pub default_timeout_secs: u64,
    #[serde(default = "default_concurrency")]
    pub default_concurrency: usize,
    #[serde(default = "default_deny_env")]
    pub deny_env: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<ExecProfile>,
    /// Resolved at startup pinning (not deserialized). Runtime confinement uses
    /// THIS, never a `"."` fallback (I-6: fail-closed, no silent weakening).
    #[serde(skip)]
    pub canonical_workspace_root: Option<std::path::PathBuf>,
}

impl ExecGlobalConfig {
    /// Fail-fast validation + canonical pinning. Must run at startup (ADR-0033).
    pub fn validate(&mut self, base: &std::path::Path) -> Result<(), camel_api::CamelError> {
        if self.profiles.is_empty() {
            return Err(camel_api::CamelError::Config(
                "exec: no profiles configured (fail-closed: refusing to execute anything)".into(),
            ));
        }
        let ws_root = base.join(&self.workspace_root);
        let canonical_root = ws_root.canonicalize().map_err(|e| {
            camel_api::CamelError::Config(format!("exec workspace_root unresolvable: {e}"))
        })?;
        // I-6: pin once, reuse at runtime (never fall back to ".").
        self.canonical_workspace_root = Some(canonical_root.clone());

        let mut seen = std::collections::HashSet::new();
        for p in &mut self.profiles {
            if !seen.insert(p.name.clone()) {
                return Err(camel_api::CamelError::Config(format!(
                    "dup profile name {:?}",
                    p.name
                )));
            }
            if p.concurrency == 0 {
                return Err(camel_api::CamelError::Config(
                    "concurrency must be > 0".into(),
                ));
            }
            if p.timeout_secs == 0 {
                return Err(camel_api::CamelError::Config(
                    "timeout_secs must be > 0".into(),
                ));
            }
            if p.accepted_exit_codes.is_empty() {
                return Err(camel_api::CamelError::Config(
                    "accepted_exit_codes must be non-empty".into(),
                ));
            }

            // v1 reserved enums: reject future values at startup.
            if p.output_mode != OutputMode::Materialized {
                return Err(camel_api::CamelError::Config(
                    "output_mode: only \"materialized\" supported in v1".into(),
                ));
            }
            if p.sandbox != Sandbox::None {
                return Err(camel_api::CamelError::Config(
                    "sandbox: only \"none\" supported in v1".into(),
                ));
            }

            // Canonical-pin the executable (no PATH lookup at spawn time).
            let resolved = resolve_executable(&p.executable)
                .map_err(|e| camel_api::CamelError::Config(format!("profile {:?}: {e}", p.name)))?;
            p.canonical_executable = Some(resolved);

            // Confine working_dir under canonical_root.
            confine(&canonical_root, &p.working_dir)
                .map_err(|e| camel_api::CamelError::Config(format!("profile {:?}: {e}", p.name)))?;
        }
        Ok(())
    }

    /// Look up a profile by logical name (used by endpoint + producer).
    pub fn profile(&self, name: &str) -> Option<&ExecProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

/// Resolve an executable to a canonical path. Name → PATH lookup; absolute → as-is.
fn resolve_executable(spec: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::Path::new(spec);
    if p.is_absolute() {
        return p
            .canonicalize()
            .map_err(|e| format!("absolute executable unresolvable: {e}"));
    }
    which(spec).ok_or_else(|| format!("executable {spec:?} not found on PATH"))
}

/// Minimal PATH lookup (avoid pulling a crate; std-only).
///
/// NOTE: does NOT canonicalize — multi-call binaries (e.g. NixOS coreutils)
/// need argv[0] to match the symlink name, not the resolved multi-call binary
/// path. Startup pinning stores the directory-path as-is.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Confine `rel` under `root`: reject absolute and `..`, canonicalize, require starts_with.
///
/// NOTE (M-3): for v1, `confine` does NOT create dirs; it canonicalizes existing
/// paths and errors if the working_dir does not exist. (Operator must pre-create
/// working dirs; fail-closed over silent creation.)
pub(crate) fn confine(root: &std::path::Path, rel: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::Path::new(rel);
    if p.is_absolute() {
        return Err(format!("working_dir must be relative, got {rel:?}"));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "working_dir escapes workspace root (must not contain '..'): {rel:?}"
        ));
    }
    let joined = root.join(p);
    let canon = joined
        .canonicalize()
        .map_err(|e| format!("working_dir unresolvable: {e}"))?;
    if !canon.starts_with(root) {
        return Err(format!(
            "working_dir escapes workspace root: {}",
            canon.display()
        ));
    }
    Ok(canon)
}

fn default_deny_env() -> Vec<String> {
    vec![
        "LD_*".into(),
        "DYLD_*".into(),
        "PYTHONPATH".into(),
        "RUSTFLAGS".into(),
        "GIT_*".into(),
        "SSH_AUTH_SOCK".into(),
        "*_TOKEN".into(),
        "*_KEY".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gcfg() -> ExecGlobalConfig {
        toml::from_str(
            r#"
workspace_root = "."
[[profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
working_dir = "."
"#,
        )
        .unwrap()
    }

    #[test]
    fn validate_rejects_empty_profiles_fail_closed() {
        let mut c = ExecGlobalConfig::default();
        let err = c.validate(&std::env::current_dir().unwrap()).unwrap_err();
        assert!(err.to_string().contains("no profiles"), "{err}");
    }

    #[test]
    fn validate_pins_executable_canonical() {
        let mut c = gcfg();
        c.validate(&std::env::current_dir().unwrap()).unwrap();
        assert!(
            c.profiles[0].canonical_executable.is_some(),
            "must be pinned"
        );
    }

    #[test]
    fn validate_rejects_unknown_output_mode_stream() {
        let mut c = toml::from_str::<ExecGlobalConfig>(
            r#"
[[profiles]]
name = "x"
executable = "echo"
output_mode = "stream"
"#,
        )
        .unwrap();
        let err = c.validate(&std::env::current_dir().unwrap());
        assert!(err.is_err(), "stream must be rejected at startup in v1");
    }

    #[test]
    fn validate_rejects_working_dir_escape() {
        let mut c = toml::from_str::<ExecGlobalConfig>(
            r#"
workspace_root = "."
[[profiles]]
name = "x"
executable = "echo"
working_dir = "../escape"
"#,
        )
        .unwrap();
        let err = c.validate(&std::env::current_dir().unwrap()).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("workspace"),
            "{err}"
        );
    }
}
