//! policy.rs — pure security core for the exec component.

use crate::config::{ArgPolicy, DenyFlags, EnvPolicy};
use crate::error::ExecError;
use std::collections::HashMap;

/// Validate an args slice against the profile's policy. Per-element. Order:
/// deny_flags first, then allow mode. Empty slice always passes.
pub fn eval_args(args: &[String], policy: &ArgPolicy, deny: &DenyFlags) -> Result<(), ExecError> {
    if args.is_empty() {
        return Ok(());
    }
    for a in args {
        for f in &deny.0 {
            if a == f || a.starts_with(f) {
                return Err(ExecError::ArgPolicyDenied {
                    arg: a.clone(),
                    reason: format!("matches deny_flags entry {f:?}"),
                });
            }
        }
        let ok = match policy {
            ArgPolicy::Any => true,
            ArgPolicy::Exact { values } => values.iter().any(|v| v == a),
            ArgPolicy::Prefix { values } => values.iter().any(|v| a.starts_with(v)),
        };
        if !ok {
            return Err(ExecError::ArgPolicyDenied {
                arg: a.clone(),
                reason: "not permitted by allow policy".into(),
            });
        }
    }
    Ok(())
}

/// Build the child env: start empty, copy allow-listed host vars, apply `set`,
/// then strip anything matching `deny_env` globs (deny wins, applied last).
pub fn build_env(
    policy: &EnvPolicy,
    deny_env: &[String],
    host: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for k in &policy.allow {
        if let Some(v) = host.get(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &policy.set {
        out.insert(k.clone(), v.clone());
    }
    // deny_env applied last, always wins.
    out.retain(|k, _| !deny_env.iter().any(|pat| glob_matches(pat, k)));
    out
}

/// Glob matcher backed by `glob::Pattern` (supports `*`, `?`, `[...]`).
/// Returns false on unparseable patterns (fail-safe: bad pattern matches nothing).
fn glob_matches(pat: &str, name: &str) -> bool {
    glob::Pattern::new(pat)
        .map(|p| p.matches(name))
        .unwrap_or(false)
}

/// Defense-in-depth: reject known shells unless `allow_shell`.
pub fn is_known_shell(exe: &str) -> bool {
    let base = std::path::Path::new(exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(exe)
        .to_ascii_lowercase();
    const SHELLS: &[&str] = &[
        "sh",
        "bash",
        "dash",
        "zsh",
        "ksh",
        "fish",
        "csh",
        "tcsh",
        "cmd.exe",
        "powershell.exe",
        "pwsh",
    ];
    SHELLS
        .iter()
        .any(|s| base == *s || base.starts_with(&format!("{s}.")))
}

#[cfg(test)]
mod arg_tests {
    use super::*;
    use crate::config::{ArgPolicy, DenyFlags};

    #[test]
    fn any_accepts_all() {
        assert!(
            eval_args(
                &["log", "--oneline"].map(String::from),
                &ArgPolicy::Any,
                &DenyFlags::default()
            )
            .is_ok()
        );
    }
    #[test]
    fn exact_rejects_unknown_element() {
        let p = ArgPolicy::Exact {
            values: ["check"].map(String::from).to_vec(),
        };
        assert!(eval_args(&["check"].map(String::from), &p, &DenyFlags::default()).is_ok());
        assert!(
            eval_args(
                &["check", "--release"].map(String::from),
                &p,
                &DenyFlags::default()
            )
            .is_err()
        );
    }
    #[test]
    fn prefix_matches_per_element() {
        let p = ArgPolicy::Prefix {
            values: ["log", "diff", "--"].map(String::from).to_vec(),
        };
        assert!(
            eval_args(
                &["log", "--oneline"].map(String::from),
                &p,
                &DenyFlags::default()
            )
            .is_ok()
        );
        assert!(eval_args(&["push"].map(String::from), &p, &DenyFlags::default()).is_err());
    }
    #[test]
    fn deny_flags_applied_first() {
        let p = ArgPolicy::Any;
        let d = DenyFlags(["--upload-pack"].map(String::from).to_vec());
        assert!(eval_args(&["--upload-pack=x"].map(String::from), &p, &d).is_err());
    }
    #[test]
    fn empty_args_always_allowed() {
        let p = ArgPolicy::Exact { values: vec![] };
        assert!(eval_args(&[], &p, &DenyFlags::default()).is_ok());
    }
}

#[cfg(test)]
mod env_shell_tests {
    use super::*;
    use crate::config::EnvPolicy;
    use std::collections::HashMap;

    fn env(allow: &[&str], set: &[(&str, &str)]) -> EnvPolicy {
        EnvPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            set: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn deny_env_globs_stripped_last() {
        // Host carries a token; allow-list ALSO permits it; set explicitly sets another token.
        // deny_env *_TOKEN must strip BOTH (copied-from-allow AND explicitly-set),
        // proving deny is applied last and always wins.
        let mut host = HashMap::new();
        host.insert("POLICY_TEST_GITHUB_TOKEN".to_string(), "leak".into());
        host.insert("POLICY_TEST_HOME".to_string(), "/h".into());
        let p = env(
            &["POLICY_TEST_HOME", "POLICY_TEST_GITHUB_TOKEN"], // token allowed (copied in)
            &[("CI", "1"), ("POLICY_TEST_AWS_TOKEN", "secret")], // AWS_TOKEN set explicitly
        );
        let out = build_env(&p, &["*_TOKEN".to_string()], &host);
        assert_eq!(out.get("CI"), Some(&"1".to_string()));
        assert_eq!(out.get("POLICY_TEST_HOME"), Some(&"/h".to_string()));
        assert!(
            !out.contains_key("POLICY_TEST_GITHUB_TOKEN"),
            "deny_env must strip allowed *_TOKEN"
        );
        assert!(
            !out.contains_key("POLICY_TEST_AWS_TOKEN"),
            "deny_env must strip explicitly-set *_TOKEN"
        );
    }

    #[test]
    fn known_shell_rejected_unless_allowed() {
        assert!(is_known_shell("/bin/sh"));
        assert!(is_known_shell("/usr/bin/bash"));
        assert!(is_known_shell("cmd.exe"));
        assert!(is_known_shell("/usr/bin/pwsh.exe")); // .exe-suffix branch
        assert!(!is_known_shell("/usr/bin/git"));
    }
}
