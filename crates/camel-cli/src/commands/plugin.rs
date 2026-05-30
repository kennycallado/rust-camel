use clap::{Args, Subcommand};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, clap::ValueEnum, PartialEq)]
pub enum PluginType {
    Processor,
    Bean,
    AuthorizationPolicy,
}

impl fmt::Display for PluginType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginType::Processor => write!(f, "processor"),
            PluginType::Bean => write!(f, "bean"),
            PluginType::AuthorizationPolicy => write!(f, "authorization-policy"),
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum PluginAction {
    New(PluginNewArgs),
    Build(PluginBuildArgs),
}

#[derive(Args, Debug)]
pub struct PluginNewArgs {
    pub name: String,
    #[arg(long, value_name = "TYPE", default_value_t = PluginType::Processor)]
    pub r#type: PluginType,
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct PluginBuildArgs {
    pub path: Option<String>,
    #[arg(long)]
    pub debug: bool,
}

pub fn run_plugin(action: PluginAction) {
    match action {
        PluginAction::New(args) => run_plugin_new(args),
        PluginAction::Build(args) => run_plugin_build(args),
    }
}

fn run_plugin_new(args: PluginNewArgs) {
    let PluginNewArgs {
        name,
        force,
        r#type: plugin_type,
    } = args;

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        eprintln!(
            "Error: plugin name must contain only alphanumeric characters, hyphens, or underscores"
        );
        std::process::exit(1);
    }

    let files = match plugin_type {
        PluginType::Bean => crate::template::bean::bean_files(&name),
        PluginType::Processor => crate::template::processor::processor_files(&name),
        PluginType::AuthorizationPolicy => {
            crate::template::authorization_policy::authorization_policy_files(&name)
        }
    };
    let target = Path::new(&name);

    if target.exists() && !force {
        let is_non_empty = target.read_dir().is_ok_and(|mut d| d.next().is_some());
        if is_non_empty {
            eprintln!(
                "Directory '{}' already exists and is not empty. Use --force to overwrite.",
                name
            );
            std::process::exit(1);
        }
    }

    std::fs::create_dir_all(target).unwrap_or_else(|e| {
        eprintln!("Failed to create directory '{}': {}", name, e);
        std::process::exit(1);
    });

    for file in &files {
        let file_path = target.join(&file.path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                eprintln!("Failed to create directory '{}': {}", parent.display(), e);
                std::process::exit(1);
            });
        }
        std::fs::write(&file_path, &file.content).unwrap_or_else(|e| {
            eprintln!("Failed to write '{}': {}", file_path.display(), e);
            std::process::exit(1);
        });
    }

    let type_label = match plugin_type {
        PluginType::Bean => "bean",
        PluginType::Processor => "processor",
        PluginType::AuthorizationPolicy => "authorization-policy",
    };
    println!("Created camel {} plugin '{}'\n", type_label, name);
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  camel plugin build");
}

fn run_plugin_build(args: PluginBuildArgs) {
    let plugin_dir = match args.path {
        Some(ref p) => {
            let canonical = std::path::Path::new(p).canonicalize().unwrap_or_else(|e| {
                eprintln!("Error: cannot resolve path '{}': {e}", p);
                std::process::exit(1);
            });
            if !canonical.join("Cargo.toml").exists() {
                eprintln!(
                    "Error: '{}' does not contain a Cargo.toml",
                    canonical.display()
                );
                std::process::exit(1);
            }
            canonical
        }
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("Error: failed to get current directory: {e}");
                std::process::exit(1);
            });
            let canonical = cwd.canonicalize().unwrap_or_else(|e| {
                eprintln!("Error: cannot resolve current directory: {e}");
                std::process::exit(1);
            });
            if !canonical.join("Cargo.toml").exists() {
                eprintln!(
                    "Error: current directory '{}' does not contain a Cargo.toml",
                    canonical.display()
                );
                std::process::exit(1);
            }
            canonical
        }
    };

    let cargo_toml_path = plugin_dir.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to read '{}': {}",
            cargo_toml_path.display(),
            e
        );
        std::process::exit(1);
    });

    let parsed: toml::Value = toml::from_str(&cargo_toml).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to parse '{}': {}",
            cargo_toml_path.display(),
            e
        );
        std::process::exit(1);
    });

    let plugin_name = parsed
        .get("package")
        .and_then(|pkg| pkg.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            eprintln!(
                "Error: missing [package].name in '{}'",
                cargo_toml_path.display()
            );
            std::process::exit(1);
        });

    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        .current_dir(&plugin_dir);

    if !args.debug {
        cmd.arg("--release");
    }

    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("Error: failed to execute build command: {e}");
        std::process::exit(1);
    });

    if !status.success() {
        eprintln!("Error: build failed");
        std::process::exit(1);
    }

    let built_wasm = build_output_path(&plugin_dir, &plugin_name, args.debug);
    if !built_wasm.exists() {
        eprintln!("Error: built wasm not found at '{}'", built_wasm.display());
        std::process::exit(1);
    }

    let camel_root = find_camel_root(&plugin_dir).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });

    let plugins_dir_relative = resolve_plugins_dir(&camel_root).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    let plugins_dir = camel_root.join(&plugins_dir_relative);

    std::fs::create_dir_all(&plugins_dir).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to create plugins directory '{}': {}",
            plugins_dir.display(),
            e
        );
        std::process::exit(1);
    });

    let installed_wasm = plugins_dir.join(format!("{plugin_name}.wasm"));
    std::fs::copy(&built_wasm, &installed_wasm).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to copy '{}' to '{}': {}",
            built_wasm.display(),
            installed_wasm.display(),
            e
        );
        std::process::exit(1);
    });

    println!("Built and installed plugin '{}'", plugin_name);
    println!("  source: {}", built_wasm.display());
    println!("  installed: {}", installed_wasm.display());
}

pub fn find_camel_root(start: &Path) -> Result<PathBuf, String> {
    for dir in start.ancestors() {
        if dir.join("Camel.toml").exists() {
            return Ok(dir.to_path_buf());
        }
        let workspace_cargo = dir.join("Cargo.toml");
        if workspace_cargo.exists() {
            let contents = std::fs::read_to_string(&workspace_cargo)
                .map_err(|e| format!("failed to read '{}': {}", workspace_cargo.display(), e))?;
            let parsed: toml::Value = toml::from_str(&contents)
                .map_err(|e| format!("failed to parse '{}': {}", workspace_cargo.display(), e))?;
            if parsed.get("workspace").is_some() {
                return Ok(dir.to_path_buf());
            }
        }
    }

    Err(format!(
        "could not find Camel.toml or workspace Cargo.toml from '{}'",
        start.display()
    ))
}

pub fn build_output_path(dir: &Path, plugin_name: &str, debug: bool) -> PathBuf {
    let profile = if debug { "debug" } else { "release" };
    let wasm_name = plugin_name.replace('-', "_");
    dir.join("target")
        .join("wasm32-wasip2")
        .join(profile)
        .join(format!("{wasm_name}.wasm"))
}

/// Validates a `plugins_dir` config value relative to `camel_root`.
///
/// Rejects empty strings, absolute paths, and any path containing `ParentDir` (`..`) components.
/// Also verifies that the resolved path stays within `camel_root` (catches symlink escapes).
pub fn validate_plugins_dir(camel_root: &Path, dir: &str) -> Result<(), String> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Err("plugins_dir must not be empty".to_string());
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!(
            "plugins_dir must be a relative path, got '{}'",
            dir
        ));
    }

    // Reject any ParentDir (..) component using Path::components()
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(format!("plugins_dir must not contain '..', got '{}'", dir));
        }
    }

    // Containment check via canonicalization
    let canonical_root = camel_root
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize project root: {e}"))?;

    let candidate = camel_root.join(trimmed);

    // Try to canonicalize the candidate directly
    if let Ok(canonical_candidate) = candidate.canonicalize() {
        if !canonical_candidate.starts_with(&canonical_root) {
            return Err("plugins_dir resolves outside project root".to_string());
        }
        return Ok(());
    }

    // Candidate doesn't exist yet — walk up ancestors until we find one that does
    let mut ancestor = candidate.as_path();
    let mut suffix = PathBuf::new();
    loop {
        if ancestor.exists() {
            match ancestor.canonicalize() {
                Ok(canonical_ancestor) => {
                    let resolved = canonical_ancestor.join(&suffix);
                    if !resolved.starts_with(&canonical_root) {
                        return Err("plugins_dir resolves outside project root".to_string());
                    }
                    return Ok(());
                }
                Err(e) => {
                    return Err(format!(
                        "failed to canonicalize ancestor '{}': {e}",
                        ancestor.display()
                    ));
                }
            }
        }
        if let Some(parent) = ancestor.parent() {
            if let Some(file_name) = ancestor.file_name() {
                // Prepend this component to the suffix
                let mut new_suffix = PathBuf::from(file_name);
                if !suffix.as_os_str().is_empty() {
                    new_suffix.push(&suffix);
                }
                suffix = new_suffix;
                ancestor = parent;
            } else {
                return Err("no existing ancestor found for plugins_dir".to_string());
            }
        } else {
            return Err("no existing ancestor found for plugins_dir".to_string());
        }
    }
}

/// Resolves the plugins directory from Camel.toml `[default.components.wasm].plugins_dir`.
///
/// Falls back to `"plugins"` when no Camel.toml or no `plugins_dir` key is present.
pub fn resolve_plugins_dir(camel_root: &Path) -> Result<PathBuf, String> {
    let toml_path = camel_root.join("Camel.toml");
    if toml_path.exists() {
        let contents = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("failed to read '{}': {e}", toml_path.display()))?;
        let parsed: toml::Value = toml::from_str(&contents)
            .map_err(|e| format!("failed to parse '{}': {e}", toml_path.display()))?;

        if let Some(plugins_dir) = parsed
            .get("default")
            .and_then(|d| d.get("components"))
            .and_then(|c| c.get("wasm"))
            .and_then(|w| w.get("plugins_dir"))
            .and_then(toml::Value::as_str)
        {
            validate_plugins_dir(camel_root, plugins_dir)?;
            return Ok(PathBuf::from(plugins_dir));
        }
    }

    // Default: "plugins"
    validate_plugins_dir(camel_root, "plugins")?;
    Ok(PathBuf::from("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tempfile::tempdir;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        action: PluginAction,
    }

    #[test]
    fn plugin_action_parses_new_with_force() {
        let cli = TestCli::try_parse_from(["test", "new", "my-plugin", "--force"])
            .expect("expected parse success");
        match cli.action {
            PluginAction::New(args) => {
                assert_eq!(args.name, "my-plugin");
                assert!(args.force);
                assert_eq!(args.r#type, PluginType::Processor);
            }
            _ => panic!("expected PluginAction::New"),
        }
    }

    #[test]
    fn plugin_action_parses_new_bean_type() {
        let cli = TestCli::try_parse_from(["test", "new", "my-bean", "--type", "bean"])
            .expect("expected parse success");
        match cli.action {
            PluginAction::New(args) => {
                assert_eq!(args.name, "my-bean");
                assert_eq!(args.r#type, PluginType::Bean);
            }
            _ => panic!("expected PluginAction::New"),
        }
    }

    #[test]
    fn plugin_action_default_type_is_processor() {
        let cli =
            TestCli::try_parse_from(["test", "new", "my-proc"]).expect("expected parse success");
        match cli.action {
            PluginAction::New(args) => {
                assert_eq!(args.name, "my-proc");
                assert_eq!(args.r#type, PluginType::Processor);
            }
            _ => panic!("expected PluginAction::New"),
        }
    }

    #[test]
    fn plugin_action_parses_build_debug() {
        let cli =
            TestCli::try_parse_from(["test", "build", "--debug"]).expect("expected parse success");
        match cli.action {
            PluginAction::Build(args) => {
                assert!(args.debug);
            }
            _ => panic!("expected PluginAction::Build"),
        }
    }

    #[test]
    fn plugin_build_accepts_optional_path() {
        let cli = TestCli::try_parse_from(["test", "build", "my-plugin/"])
            .expect("expected parse success");
        match cli.action {
            PluginAction::Build(args) => {
                assert_eq!(args.path.as_deref(), Some("my-plugin/"));
            }
            _ => panic!("expected PluginAction::Build"),
        }
    }

    #[test]
    fn plugin_build_defaults_path_to_none() {
        let cli = TestCli::try_parse_from(["test", "build"]).expect("expected parse success");
        match cli.action {
            PluginAction::Build(args) => {
                assert!(args.path.is_none());
            }
            _ => panic!("expected PluginAction::Build"),
        }
    }

    #[test]
    fn plugin_action_rejects_missing_name() {
        let result = TestCli::try_parse_from(["test", "new"]);
        assert!(result.is_err());
    }

    #[test]
    fn plugin_type_display_values() {
        assert_eq!(PluginType::Processor.to_string(), "processor");
        assert_eq!(PluginType::Bean.to_string(), "bean");
        assert_eq!(
            PluginType::AuthorizationPolicy.to_string(),
            "authorization-policy"
        );
    }

    #[test]
    fn plugin_type_authorization_policy_display() {
        assert_eq!(
            PluginType::AuthorizationPolicy.to_string(),
            "authorization-policy"
        );
    }

    #[test]
    fn plugin_action_rejects_invalid_type() {
        let result = TestCli::try_parse_from(["test", "new", "my-plugin", "--type", "unknown"]);
        assert!(result.is_err());
    }

    #[test]
    fn find_camel_root_finds_camel_toml() {
        let root = tempdir().expect("tempdir");
        std::fs::write(root.path().join("Camel.toml"), "name = \"x\"\n").expect("write");
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let found = find_camel_root(&nested).expect("find root");
        assert_eq!(found, root.path());
    }

    #[test]
    fn find_camel_root_finds_workspace_cargo_toml() {
        let root = tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .expect("write");
        let nested = root.path().join("x").join("y");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let found = find_camel_root(&nested).expect("find root");
        assert_eq!(found, root.path());
    }

    #[test]
    fn find_camel_root_errors_without_markers() {
        let root = tempdir().expect("tempdir");
        let nested = root.path().join("one").join("two");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let err = find_camel_root(&nested).expect_err("expected error");
        assert!(err.contains("could not find Camel.toml or workspace Cargo.toml"));
    }

    #[test]
    fn find_camel_root_prefers_nearest_ancestor_marker() {
        let root = tempdir().expect("tempdir");
        std::fs::write(root.path().join("Camel.toml"), "name = \"x\"\n").expect("write");
        let mid = root.path().join("mid");
        std::fs::create_dir_all(&mid).expect("mkdir");
        std::fs::write(mid.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("write");
        let nested = mid.join("deep");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let found = find_camel_root(&nested).expect("find root");
        assert_eq!(found, mid);
    }

    #[test]
    fn find_camel_root_returns_parse_error_for_invalid_workspace_toml() {
        let root = tempdir().expect("tempdir");
        std::fs::write(root.path().join("Cargo.toml"), "[workspace\ninvalid").expect("write");
        let nested = root.path().join("x").join("y");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let err = find_camel_root(&nested).expect_err("expected error");
        assert!(err.contains("failed to parse"));
        assert!(err.contains("Cargo.toml"));
    }

    #[test]
    fn find_camel_root_ignores_non_workspace_cargo_toml() {
        let root = tempdir().expect("tempdir");
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let err = find_camel_root(&nested).expect_err("expected error");
        assert!(err.contains("could not find Camel.toml or workspace Cargo.toml"));
    }

    #[test]
    fn find_camel_root_returns_read_error_for_unreadable_workspace_marker() {
        let root = tempdir().expect("tempdir");
        let cargo_as_dir = root.path().join("Cargo.toml");
        std::fs::create_dir_all(&cargo_as_dir).expect("mkdir");
        let nested = root.path().join("x").join("y");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let err = find_camel_root(&nested).expect_err("expected read error");
        assert!(err.contains("failed to read"), "got: {err}");
        assert!(err.contains("Cargo.toml"), "got: {err}");
    }

    #[test]
    fn build_output_path_release() {
        let dir = Path::new("/tmp/project");
        let path = build_output_path(dir, "my-plugin", false);
        assert!(
            path.ends_with(Path::new("target/wasm32-wasip2/release/my_plugin.wasm")),
            "got: {}",
            path.display()
        );
    }

    #[test]
    fn build_output_path_debug() {
        let dir = Path::new("/tmp/project");
        let path = build_output_path(dir, "my-plugin", true);
        assert!(
            path.ends_with(Path::new("target/wasm32-wasip2/debug/my_plugin.wasm")),
            "got: {}",
            path.display()
        );
    }

    #[test]
    fn build_output_path_keeps_existing_underscores() {
        let dir = Path::new("/tmp/project");
        let path = build_output_path(dir, "my_plugin", false);
        assert!(
            path.ends_with(Path::new("target/wasm32-wasip2/release/my_plugin.wasm")),
            "got: {}",
            path.display()
        );
    }

    #[test]
    fn build_output_path_replaces_all_hyphens() {
        let dir = Path::new("/tmp/project");
        let path = build_output_path(dir, "my-super-plugin", true);
        assert!(
            path.ends_with(Path::new("target/wasm32-wasip2/debug/my_super_plugin.wasm")),
            "got: {}",
            path.display()
        );
    }

    // validate_plugins_dir tests
    #[test]
    fn validate_plugins_dir_rejects_absolute_path() {
        let root = tempfile::tempdir().expect("tempdir");
        let err = super::validate_plugins_dir(root.path(), "/tmp/plugins").unwrap_err();
        assert!(err.contains("relative path"), "got: {err}");
    }

    #[test]
    fn validate_plugins_dir_rejects_parentdir_component() {
        let root = tempfile::tempdir().expect("tempdir");
        let err = super::validate_plugins_dir(root.path(), "../other").unwrap_err();
        assert!(err.contains("'..'"), "got: {err}");
    }

    #[test]
    fn validate_plugins_dir_rejects_parentdir_mid_path() {
        let root = tempfile::tempdir().expect("tempdir");
        let err = super::validate_plugins_dir(root.path(), "foo/../bar").unwrap_err();
        assert!(err.contains("'..'"), "got: {err}");
    }

    #[test]
    fn validate_plugins_dir_rejects_empty_string() {
        let root = tempfile::tempdir().expect("tempdir");
        let err = super::validate_plugins_dir(root.path(), "").unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn validate_plugins_dir_accepts_simple_relative() {
        let root = tempfile::tempdir().expect("tempdir");
        super::validate_plugins_dir(root.path(), "plugins").expect("should accept");
    }

    #[test]
    fn validate_plugins_dir_accepts_nested_relative() {
        let root = tempfile::tempdir().expect("tempdir");
        super::validate_plugins_dir(root.path(), ".camel/plugins").expect("should accept");
    }

    #[cfg(unix)]
    #[test]
    fn validate_plugins_dir_rejects_symlink_escape() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir outside");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let err = super::validate_plugins_dir(root.path(), "link/escape").unwrap_err();
        assert!(err.contains("outside project root"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn validate_plugins_dir_rejects_symlink_escape_missing_target() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir outside");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let err = super::validate_plugins_dir(root.path(), "link/sub/deep").unwrap_err();
        assert!(err.contains("outside project root"), "got: {err}");
    }

    // resolve_plugins_dir tests
    #[test]
    fn resolve_plugins_dir_defaults_to_plugins_when_no_camel_toml() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = super::resolve_plugins_dir(root.path()).expect("should resolve");
        assert_eq!(dir, std::path::PathBuf::from("plugins"));
    }

    #[test]
    fn resolve_plugins_dir_reads_default_components_wasm_plugins_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("Camel.toml"),
            "[default.components.wasm]\nplugins_dir = \".camel/plugins\"\n",
        )
        .expect("write config");
        let dir = super::resolve_plugins_dir(root.path()).expect("should resolve");
        assert_eq!(dir, std::path::PathBuf::from(".camel/plugins"));
    }

    #[test]
    fn resolve_plugins_dir_returns_error_on_invalid_toml() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("Camel.toml"), "[invalid\n").expect("write config");
        let err = super::resolve_plugins_dir(root.path()).unwrap_err();
        assert!(err.contains("failed to parse"), "got: {err}");
    }

    #[test]
    fn resolve_plugins_dir_rejects_invalid_plugins_dir_from_config() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("Camel.toml"),
            "[default.components.wasm]\nplugins_dir = \"/absolute/path\"\n",
        )
        .expect("write config");
        let err = super::resolve_plugins_dir(root.path()).unwrap_err();
        assert!(err.contains("relative path"), "got: {err}");
    }
}
