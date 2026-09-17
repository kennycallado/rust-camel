//! Canonical config-loader TOML semantics (bd rc-io2zl).
//!
//! This module is the single canonical implementation of the shared
//! configuration-loader semantics: deep merge, the ordered profile
//! section walk, ordered include-declaration collection, include-key
//! stripping, profile-section selection, and the profile
//! structure/presence predicates. Every helper is a pure
//! [`toml::Value`] transform with no I/O and no error policy of its
//! own.
//!
//! Consumers:
//! - `camel-config`'s filesystem loader (profile resolution, include
//!   extraction)
//! - `camel-dsl`'s virtual-store assembly (`discovery`)
//! - `camel-cli`'s `compile::sources` (config/profile source
//!   resolution)
//!
//! Consumer-specific validation and error strings stay local to each
//! consumer; only the semantics live here.

/// Deep-merge `overlay` into `base`: tables merge recursively, every
/// other value (arrays included) is replaced by the overlay — array
/// replacement is what gives profile overlays such as `routes` their
/// replace, never concatenate, semantics.
pub fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                if let Some(base_value) = base_table.get_mut(key) {
                    merge_toml_values(base_value, value);
                } else {
                    base_table.insert(key.clone(), value.clone());
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// Ordered profile section walk: `[default]` first, then each
/// selected profile in selection order, deduplicated preserving first
/// occurrence (a literal `default` selection collapses into the
/// leading entry).
pub fn section_walk(profiles: &[String]) -> Vec<String> {
    let mut sections = vec!["default".to_string()];
    for profile in profiles {
        if !sections.contains(profile) {
            sections.push(profile.clone());
        }
    }
    sections
}

/// Ordered include-declaration collection: the top-level `include`
/// key first (label `""`), then each walked section's `include` key
/// (label = section name), skipping absent keys. Values borrow from
/// `value`.
pub fn include_declarations<'a>(
    value: &'a toml::Value,
    profiles: &[String],
) -> Vec<(String, &'a toml::Value)> {
    let Some(table) = value.as_table() else {
        return Vec::new();
    };
    let mut declarations = Vec::new();
    if let Some(include) = table.get("include") {
        declarations.push((String::new(), include));
    }
    for section in section_walk(profiles) {
        if let Some(toml::Value::Table(section_table)) = table.get(&section)
            && let Some(include) = section_table.get("include")
        {
            declarations.push((section, include));
        }
    }
    declarations
}

/// Remove `include` keys from the top-level table and from every
/// walked section; the walk order is `section_walk`.
pub fn strip_include_keys(value: &mut toml::Value, profiles: &[String]) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    table.remove("include");
    for section in section_walk(profiles) {
        if let Some(toml::Value::Table(section_table)) = table.get_mut(&section) {
            section_table.remove("include");
        }
    }
}

/// Apply the filesystem profile-section selection to one document,
/// generalized to the store's ordered selected profiles: the
/// `[default]` section forms the base when present (else the first
/// selected section), every selected profile section overlays it in
/// selection order, and the selected content REPLACES the document
/// root. A document with neither `[default]` nor any selected section
/// stays as-is (flat config).
pub fn select_profile_sections(value: &mut toml::Value, profiles: &[String]) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    let mut base = match table.get("default").cloned() {
        Some(default) => default,
        None => match profiles.iter().find(|p| table.contains_key(p.as_str())) {
            Some(first) => match table.get(first.as_str()) {
                Some(section) => section.clone(),
                // `find` proved presence; unreachable in practice.
                None => return,
            },
            // Flat document with no profile structure: keep as-is.
            None => return,
        },
    };
    for profile in profiles {
        if let Some(section) = table.get(profile.as_str()) {
            merge_toml_values(&mut base, section);
        }
    }
    *value = base;
}

/// Whether the document carries any profile structure: a `[default]`
/// section or any selected profile section.
pub fn has_profile_structure(value: &toml::Value, profiles: &[String]) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    table.contains_key("default") || has_selected_profile(value, profiles)
}

/// Whether any selected profile section is present in the document.
pub fn has_selected_profile(value: &toml::Value, profiles: &[String]) -> bool {
    let Some(table) = value.as_table() else {
        return false;
    };
    profiles.iter().any(|p| table.contains_key(p.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_value(text: &str) -> toml::Value {
        toml::from_str(text).expect("valid TOML test fixture")
    }

    fn profiles(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn merge_recurses_tables_and_replaces_arrays() {
        let mut base = toml_value("a = { x = 1, y = 2 }\nr = [1]");
        let overlay = toml_value("a = { y = 9 }\nr = [2, 3]");
        merge_toml_values(&mut base, &overlay);
        let expected = toml_value("a = { x = 1, y = 9 }\nr = [2, 3]");
        assert_eq!(base, expected);
    }

    #[test]
    fn section_walk_orders_default_first_and_dedups() {
        assert_eq!(
            section_walk(&profiles(&["b", "default", "a", "b"])),
            vec!["default", "b", "a"]
        );
        // A literal `default` selection collapses into the leading entry.
        assert_eq!(
            section_walk(&profiles(&["default", "b"])),
            vec!["default", "b"]
        );
        assert_eq!(section_walk(&profiles(&[])), vec!["default"]);
    }

    #[test]
    fn include_declarations_walk_order_and_labels() {
        let doc = toml_value(
            "include = \"top.toml\"\n\
             [default]\n\
             include = \"default.toml\"\n\
             [production]\n\
             include = \"prod.toml\"\n\
             [other]\n\
             include = \"other.toml\"\n",
        );
        let declarations = include_declarations(&doc, &profiles(&["production"]));
        let labels: Vec<&str> = declarations
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, vec!["", "default", "production"]);
        assert_eq!(
            declarations[0].1,
            &toml::Value::String("top.toml".to_string())
        );
        assert_eq!(
            declarations[1].1,
            &toml::Value::String("default.toml".to_string())
        );
        assert_eq!(
            declarations[2].1,
            &toml::Value::String("prod.toml".to_string())
        );
    }

    #[test]
    fn strip_include_keys_removes_from_all_walked_sections() {
        let mut doc = toml_value(
            "include = \"top.toml\"\n\
             root_key = 1\n\
             [default]\n\
             include = \"default.toml\"\n\
             [production]\n\
             include = \"prod.toml\"\n\
             [other]\n\
             include = \"other.toml\"\n",
        );
        strip_include_keys(&mut doc, &profiles(&["production"]));
        let expected = toml_value(
            "root_key = 1\n\
             [default]\n\
             [production]\n\
             [other]\n\
             include = \"other.toml\"\n",
        );
        assert_eq!(doc, expected);
    }

    #[test]
    fn select_uses_default_base_else_first_selected() {
        let mut doc = toml_value(
            "[p1]\n\
             shared = \"p1\"\n\
             routes = [\"a\"]\n\
             [p2]\n\
             shared = \"p2\"\n\
             routes = [\"b\"]\n",
        );
        select_profile_sections(&mut doc, &profiles(&["p1", "p2"]));
        // `p1` is the base; `p2` overlays with replace semantics (the
        // `routes` array is replaced, never concatenated).
        let expected = toml_value("shared = \"p2\"\nroutes = [\"b\"]");
        assert_eq!(doc, expected);
    }

    #[test]
    fn select_flat_document_stays_as_is() {
        let mut doc = toml_value("flat = true\nother = \"value\"");
        select_profile_sections(&mut doc, &profiles(&["production"]));
        let expected = toml_value("flat = true\nother = \"value\"");
        assert_eq!(doc, expected);
    }

    #[test]
    fn select_empty_profiles_keeps_default_section() {
        let mut doc = toml_value("[default]\nx = 1\n[production]\nx = 2");
        select_profile_sections(&mut doc, &profiles(&[]));
        let expected = toml_value("x = 1");
        assert_eq!(doc, expected);
    }

    #[test]
    fn has_profile_structure_and_selected_profile_predicates() {
        let default_only = toml_value("[default]\nx = 1");
        assert!(has_profile_structure(
            &default_only,
            &profiles(&["production"])
        ));
        assert!(!has_selected_profile(
            &default_only,
            &profiles(&["production"])
        ));

        let selected_only = toml_value("[production]\nx = 1");
        assert!(has_profile_structure(
            &selected_only,
            &profiles(&["production"])
        ));
        assert!(has_selected_profile(
            &selected_only,
            &profiles(&["production"])
        ));
        assert!(!has_profile_structure(&selected_only, &profiles(&["qa"])));
        assert!(!has_selected_profile(&selected_only, &profiles(&["qa"])));

        let both = toml_value("[default]\nx = 1\n[production]\nx = 2");
        assert!(has_profile_structure(&both, &profiles(&["production"])));
        assert!(has_selected_profile(&both, &profiles(&["production"])));

        let flat = toml_value("x = 1");
        assert!(!has_profile_structure(&flat, &profiles(&["production"])));
        assert!(!has_selected_profile(&flat, &profiles(&["production"])));
    }
}
