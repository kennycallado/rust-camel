use crate::template::processor::processor_files;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Subcommand, Debug)]
pub enum PluginAction {
    New(PluginNewArgs),
    Build(PluginBuildArgs),
}

#[derive(Args, Debug)]
pub struct PluginNewArgs {
    pub name: String,
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct PluginBuildArgs {
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
    let PluginNewArgs { name, force } = args;

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        eprintln!("Error: plugin name must contain only alphanumeric characters, hyphens, or underscores");
        std::process::exit(1);
    }

    let files = processor_files(&name);
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

    println!("Created camel processor plugin '{}'\n", name);
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  camel plugin build");
}

fn run_plugin_build(args: PluginBuildArgs) {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: failed to get current directory: {e}");
        std::process::exit(1);
    });

    let cargo_toml_path = cwd.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).unwrap_or_else(|e| {
        eprintln!("Error: failed to read '{}': {}", cargo_toml_path.display(), e);
        std::process::exit(1);
    });

    let parsed: toml::Value = toml::from_str(&cargo_toml).unwrap_or_else(|e| {
        eprintln!("Error: failed to parse '{}': {}", cargo_toml_path.display(), e);
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
        .arg("wasm32-wasip2");

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

    let built_wasm = build_output_path(&cwd, &plugin_name, args.debug);
    if !built_wasm.exists() {
        eprintln!(
            "Error: built wasm not found at '{}'",
            built_wasm.display()
        );
        std::process::exit(1);
    }

    let camel_root = find_camel_root(&cwd).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });

    let plugins_dir = camel_root.join(".camel").join("plugins");
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
            let contents = std::fs::read_to_string(&workspace_cargo).map_err(|e| {
                format!(
                    "failed to read '{}': {}",
                    workspace_cargo.display(),
                    e
                )
            })?;
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
            }
            _ => panic!("expected PluginAction::New"),
        }
    }

    #[test]
    fn plugin_action_parses_build_debug() {
        let cli = TestCli::try_parse_from(["test", "build", "--debug"])
            .expect("expected parse success");
        match cli.action {
            PluginAction::Build(args) => {
                assert!(args.debug);
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
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]\nmembers = []\n")
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
}
