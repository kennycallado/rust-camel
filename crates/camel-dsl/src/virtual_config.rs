//! Virtual-store configuration assembly for embedded documents.

use crate::DiscoveryError;
use crate::config_semantics::{
    has_profile_structure, has_selected_profile, merge_toml_values, select_profile_sections,
    strip_include_keys,
};
use crate::embedded_store::{StoreEntryKind, StoreError, VirtualDocumentStore};

/// Classified configuration references of a store (in index order).
struct VirtualConfigRefs {
    /// The `Camel.toml` document path, when the store embeds one.
    config: Option<String>,
    /// Include fragment paths in declaration order.
    includes: Vec<String>,
    /// Selected profile names in selection order, from the synthesized
    /// `<name>.profile.toml` fragment paths.
    profiles: Vec<String>,
}

/// Classify `config_references` by document kind. Every reference must
/// name a config, include, or profile entry; violations surface as the
/// named store errors (missing reference, kind mismatch) and structural
/// breakage (duplicate config document, malformed profile fragment
/// path) as `MalformedVirtualConfig`.
fn classify_virtual_config(
    store: &VirtualDocumentStore,
) -> Result<VirtualConfigRefs, DiscoveryError> {
    let mut refs = VirtualConfigRefs {
        config: None,
        includes: Vec::new(),
        profiles: Vec::new(),
    };
    for path in &store.index.config_references {
        let entry = store
            .index
            .entry(path)
            .ok_or_else(|| StoreError::MissingReference(path.clone()))?;
        match entry.kind {
            StoreEntryKind::Config => {
                if refs.config.replace(path.clone()).is_some() {
                    return Err(DiscoveryError::MalformedVirtualConfig {
                        path: path.clone(),
                        error: "duplicate configuration document".to_string(),
                    });
                }
            }
            StoreEntryKind::Include => refs.includes.push(path.clone()),
            StoreEntryKind::Profile => {
                let Some(name) = path.strip_suffix(".profile.toml").filter(|n| !n.is_empty())
                else {
                    return Err(DiscoveryError::MalformedVirtualConfig {
                        path: path.clone(),
                        error: "profile entry path must be `<name>.profile.toml`".to_string(),
                    });
                };
                refs.profiles.push(name.to_string());
            }
            kind => {
                return Err(StoreError::KindMismatch {
                    path: path.clone(),
                    expected: "config, include, or profile",
                    got: kind.as_str(),
                }
                .into());
            }
        }
    }
    Ok(refs)
}

/// Build the merged configuration value from the indexed
/// config/include/profile texts, mirroring camel-config's
/// `load_includes` + `build_from_toml_value_inner` ordering: includes
/// are pre-sources in declaration order (lowest priority), the
/// configuration document sits above them, and profile-section
/// selection applies per document before merging.
///
/// The shared TOML semantics (deep merge, profile-section selection,
/// include-key stripping, profile predicates) come from
/// [`crate::config_semantics`], the canonical implementation shared
/// with camel-config's filesystem loader and `camel-cli`'s
/// `compile::sources`.
pub(crate) fn build_virtual_config(
    store: &VirtualDocumentStore,
) -> Result<toml::Value, DiscoveryError> {
    let refs = classify_virtual_config(store)?;

    // Includes (lowest priority), in declaration order. Each fragment
    // drops any `include` key (recursive includes are unsupported, as
    // in camel-config's loader) and applies lenient profile-section
    // selection before merging.
    let mut merged = toml::Value::Table(toml::Table::new());
    for path in &refs.includes {
        let text = virtual_config_text(store, path)?;
        let mut value = parse_virtual_config_toml(path, &text)?;
        if let toml::Value::Table(table) = &mut value
            && table.remove("include").is_some()
        {
            tracing::warn!(
                path,
                "embedded include declares 'include'; recursive includes are unsupported — ignoring"
            );
        }
        select_profile_sections(&mut value, &refs.profiles);
        merge_toml_values(&mut merged, &value);
    }

    // The configuration document above the includes: strip `include`
    // keys from every declaring location (top-level, `[default]`, and
    // the selected profile sections), enforce the strict unknown-profile
    // rule (a configuration with `[default]` must carry at least one
    // of the selected profile sections; the error fires only when none
    // is present — the filesystem loader's error), then apply the
    // profile-section selection and merge.
    if let Some(path) = &refs.config {
        let text = virtual_config_text(store, path)?;
        let mut value = parse_virtual_config_toml(path, &text)?;
        strip_include_keys(&mut value, &refs.profiles);
        if let toml::Value::Table(table) = &value
            && has_profile_structure(&value, &refs.profiles)
            && !refs.profiles.is_empty()
            && table.contains_key("default")
            && !has_selected_profile(&value, &refs.profiles)
        {
            return Err(DiscoveryError::MalformedVirtualConfig {
                path: path.clone(),
                error: format!(
                    "unknown profile: none of the selected profiles ({}) exist in the \
                     configuration",
                    refs.profiles.join(", ")
                ),
            });
        }
        select_profile_sections(&mut value, &refs.profiles);
        merge_toml_values(&mut merged, &value);
    }

    Ok(merged)
}

/// Read one configuration document as UTF-8 text (named failure for
/// missing-validity).
fn virtual_config_text(store: &VirtualDocumentStore, path: &str) -> Result<String, DiscoveryError> {
    store.read_text(path).map(str::to_string).ok_or_else(|| {
        DiscoveryError::MalformedVirtualConfig {
            path: path.to_string(),
            error: "configuration document is not valid UTF-8".to_string(),
        }
    })
}

/// Parse one configuration document as TOML (named failure).
fn parse_virtual_config_toml(path: &str, text: &str) -> Result<toml::Value, DiscoveryError> {
    toml::from_str(text).map_err(|e| DiscoveryError::MalformedVirtualConfig {
        path: path.to_string(),
        error: e.to_string(),
    })
}
