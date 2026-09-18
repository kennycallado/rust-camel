//! External template component (ADR-0047 Stage 2).
//!
//! Phase 1 (this task) defines the public types — `TemplateReloadError` and
//! `ExternalTemplateLimitsConfig` — without the Component/Endpoint/lifecycle
//! implementation (those land in Phase 4).
//!
//! The engine gate is all-or-nothing: default features build the full
//! minijinja-backed component, while `--no-default-features` builds an
//! engine-free crate exposing only the config and error surface.

#[cfg(feature = "default")]
pub mod bundle;
#[cfg(feature = "default")]
mod closure;
#[cfg(feature = "default")]
pub(crate) mod component;
#[cfg(feature = "default")]
pub(crate) mod endpoint;
#[cfg(feature = "default")]
pub(crate) mod lifecycle;
pub(crate) mod path_util;
#[cfg(feature = "default")]
pub(crate) mod producer;
#[cfg(feature = "default")]
pub(crate) mod reload;
#[cfg(feature = "default")]
pub(crate) mod template_set;
pub(crate) mod uri;

pub mod config;
pub mod error;

#[cfg(feature = "default")]
pub use bundle::{TemplateBundle, TemplateBundleConfig};
#[cfg(feature = "default")]
pub use component::TemplateComponent;
pub use config::{ExternalTemplateLimitsConfig, ResolvedExternalTemplateLimits};
pub use error::TemplateReloadError;
