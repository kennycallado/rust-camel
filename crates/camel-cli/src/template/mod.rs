pub mod embedded;

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
