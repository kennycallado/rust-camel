use include_dir::{Dir, DirEntry, include_dir};

use super::{TemplateContext, TemplateFile, TemplateProvider};

static BASIC_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates/basic");

pub struct EmbeddedTemplate {
    name: &'static str,
    dir: &'static Dir<'static>,
}

impl EmbeddedTemplate {
    pub fn basic() -> Self {
        Self {
            name: "basic",
            dir: &BASIC_DIR,
        }
    }
}

fn collect_files(
    dir: &Dir<'_>,
    files: &mut Vec<TemplateFile>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                collect_files(subdir, files)?;
            }
            DirEntry::File(f) => {
                let path = f.path().to_str().unwrap_or("");
                if path.starts_with("Camel.toml.") || path == "README.md.tpl" {
                    continue;
                }
                let content = f
                    .contents_utf8()
                    .ok_or_else(|| format!("file is not valid UTF-8: {path}"))?;
                files.push(TemplateFile {
                    path: path.to_string(),
                    content: content.to_string(),
                });
            }
        }
    }
    Ok(())
}

impl TemplateProvider for EmbeddedTemplate {
    fn name(&self) -> &str {
        self.name
    }

    fn files(
        &self,
        ctx: &TemplateContext,
    ) -> Result<Vec<TemplateFile>, Box<dyn std::error::Error>> {
        let mut files = Vec::new();

        let camel_toml_name = match ctx.profile_layout {
            super::ProfileLayout::Simple => "Camel.toml.simple",
            super::ProfileLayout::Env => "Camel.toml.env",
        };
        let camel_content = self
            .dir
            .get_file(camel_toml_name)
            .ok_or_else(|| format!("{camel_toml_name} template not found"))?
            .contents_utf8()
            .ok_or_else(|| format!("{camel_toml_name} template is not valid UTF-8"))?;
        files.push(TemplateFile {
            path: "Camel.toml".to_string(),
            content: camel_content.to_string(),
        });

        collect_files(self.dir, &mut files)?;

        let readme_template = self
            .dir
            .get_file("README.md.tpl")
            .ok_or("README.md.tpl not found")?
            .contents_utf8()
            .ok_or("README.md.tpl is not valid UTF-8")?;
        // Use only the final directory name for the README title
        let display_name = std::path::Path::new(&ctx.project_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&ctx.project_name);
        files.push(TemplateFile {
            path: "README.md".to_string(),
            content: readme_template.replace("<name>", display_name),
        });

        Ok(files)
    }
}
