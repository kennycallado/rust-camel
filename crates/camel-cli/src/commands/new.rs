use crate::template::embedded::EmbeddedTemplate;
use crate::template::{ProfileLayout, TemplateContext, TemplateProvider};

#[derive(clap::Args)]
pub struct NewArgs {
    /// Project name and directory to create
    #[arg(value_parser = validate_project_name)]
    pub name: String,

    /// Template to use (available: basic)
    #[arg(long, default_value = "basic")]
    pub template: String,

    /// Overwrite files if the directory already exists
    #[arg(long)]
    pub force: bool,

    /// Profile layout: simple ([default] only) or env ([default], [development], [production])
    #[arg(long, value_name = "LAYOUT", default_value = "env")]
    pub profile_layout: ProfileLayout,
}

fn validate_project_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("project name must not be empty or whitespace".into());
    }

    let path = std::path::Path::new(trimmed);

    // Reject relative paths containing separators (e.g. "my/project").
    // Absolute paths are allowed so callers can specify a target directory.
    if !path.is_absolute() && (trimmed.contains('/') || trimmed.contains('\\')) {
        return Err(
            "relative path separators ('/' or '\\') are not allowed; use an absolute path or a plain project name".into(),
        );
    }

    // Reject '..' anywhere in path components.
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("project name must not contain '..'".into());
    }

    // Validate the final component (the actual project name).
    let project_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid project name: no valid final component".to_string())?;

    if !project_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "project name must contain only alphanumeric characters, hyphens, or underscores"
                .into(),
        );
    }

    Ok(trimmed.to_string())
}

fn resolve_template(name: &str) -> Result<Box<dyn TemplateProvider>, Box<dyn std::error::Error>> {
    match name {
        "basic" => Ok(Box::new(EmbeddedTemplate::basic())),
        other => Err(format!("Unknown template: '{other}'. Available templates: basic").into()),
    }
}

pub fn run_new(args: NewArgs) {
    let NewArgs {
        name,
        template,
        force,
        profile_layout,
    } = args;

    let ctx = TemplateContext {
        project_name: name.clone(),
        profile_layout,
    };

    let provider = resolve_template(&template).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });

    let files = provider.files(&ctx).unwrap_or_else(|e| {
        eprintln!("Error generating project: {e}");
        std::process::exit(1);
    });

    let target = std::path::Path::new(&name);
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

    let display_name = std::path::Path::new(&name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&name);
    println!("Created camel project: {}\n", display_name);
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  camel run");
    println!("  camel run --watch");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_template_basic_returns_ok() {
        let result = resolve_template("basic");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "basic");
    }

    #[test]
    fn resolve_template_unknown_returns_error() {
        let result = resolve_template("nonexistent");
        match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(err) => {
                let msg = err.to_string();
                assert!(msg.contains("Unknown template"), "got: {msg}");
                assert!(msg.contains("nonexistent"), "got: {msg}");
            }
        }
    }

    #[test]
    fn test_empty_name_rejected() {
        assert!(validate_project_name("").is_err());
    }

    #[test]
    fn test_whitespace_name_rejected() {
        assert!(validate_project_name("   ").is_err());
    }

    #[test]
    fn test_valid_name_accepted() {
        assert!(validate_project_name("my-project").is_ok());
        assert!(validate_project_name("my_project_123").is_ok());
    }

    #[test]
    fn test_name_with_special_chars_rejected() {
        assert!(validate_project_name("my project").is_err());
        assert!(validate_project_name("my/project").is_err());
    }

    #[test]
    fn test_name_with_backslash_rejected() {
        let result = validate_project_name("my\\project");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("path separators"),
            "expected path separator error"
        );
    }

    #[test]
    fn test_dot_and_dotdot_rejected() {
        assert!(validate_project_name(".").is_err());
        assert!(validate_project_name("..").is_err());
    }
}
