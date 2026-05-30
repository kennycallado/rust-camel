pub mod authorization_policy;
pub mod bean;
pub mod embedded;
pub mod processor;

use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, clap::ValueEnum)]
pub enum ProfileLayout {
    Simple,
    #[default]
    Env,
}

impl fmt::Display for ProfileLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileLayout::Simple => write!(f, "simple"),
            ProfileLayout::Env => write!(f, "env"),
        }
    }
}

pub struct TemplateFile {
    pub path: String,
    pub content: String,
}

pub struct TemplateContext {
    pub project_name: String,
    pub profile_layout: ProfileLayout,
}

pub trait TemplateProvider {
    fn name(&self) -> &str;
    fn files(&self, ctx: &TemplateContext)
    -> Result<Vec<TemplateFile>, Box<dyn std::error::Error>>;
}

pub fn cargo_toml(plugin_name: &str) -> String {
    format!(
        "[package]\nname = \"{plugin_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nwit-bindgen = \"0.57\"\n"
    )
}

pub fn plugin_toml(plugin_name: &str, plugin_type: &str) -> String {
    format!(
        "name = \"{plugin_name}\"\nversion = \"0.1.0\"\ntype = \"{plugin_type}\"\nentry = \"{plugin_name}.wasm\"\n"
    )
}

pub fn readme_md(plugin_name: &str, plugin_type: &str) -> String {
    format!(
        "# {plugin_name}\n\nWASM {plugin_type} plugin for Camel.\n\n## Build\n\n```bash\ncamel plugin build\n```\n\n## Files\n\n- `src/lib.rs`: plugin entrypoint implementing {plugin_type} Guest methods\n- `wit/`: WIT definitions used for guest bindings generation\n- `Camel.plugin.toml`: plugin metadata for Camel\n"
    )
}

pub fn gitignore() -> &'static str {
    "target\nplugins/\n"
}

pub fn to_pascal_case(name: &str) -> String {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let mut out = String::new();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.extend(chars.flat_map(|c| c.to_lowercase()));
            }
            out
        })
        .collect()
}
