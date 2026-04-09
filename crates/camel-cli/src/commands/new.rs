use crate::template::embedded::EmbeddedTemplate;
use crate::template::{ProfileLayout, TemplateContext, TemplateProvider};

#[derive(clap::Args)]
pub struct NewArgs {
    /// Project name and directory to create
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

    if name.contains("..") || name.contains('\\') {
        eprintln!("Error: project name must not contain '..' or backslashes");
        std::process::exit(1);
    }

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
}
