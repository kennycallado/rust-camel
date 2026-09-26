//! Root-level key policy for `CamelConfig` documents (cfgdrop2).
//!
//! One copy of the name policy shared by both config front doors: the
//! camel-config filesystem loader (`config.rs` profile-structure guard and
//! rc-cflo warn) and the `camel compile --config` mirror guard in
//! `camel-cli::compile::sources` (which imports this module through the
//! public `camel_config::root_key_policy` path).
//!
//! The policy is name-level: apart from the entry classifier's table-ness
//! check, it never inspects or transforms `toml::Value` trees — so it
//! lives in camel-config (the policy owner) rather than camel-dsl, keeping
//! the canonical-semantics single-source rule intact without a SYNC fork.
//!
//! `KNOWN_TOP_LEVEL_KEYS` MUST mirror `CamelConfig`'s serde field
//! names. Structural keys with their own consumers (`default` for
//! profiles, `include` for file composition) are excluded at the check
//! sites instead. The `config_ergonomics_tests::known_top_level_keys_*`
//! tripwires guard drift: a name here that stops being a real field fails
//! `_extra`; a new CamelConfig field missing from this list makes its
//! section warn like an unselected profile.

/// The one root-level key exempt from the profile-structure discard guard:
/// a root `routes` list beside profile sections takes effect through the
/// compile-identical overlay (lifted before profile selection, reinserted
/// after unless a walked section declared its own `routes`).
pub const ROOT_ROUTES_KEY: &str = "routes";

/// Top-level CamelConfig keys recognized by serde deserialization, used to
/// tell real config sections apart from profile-like tables (`[<name>]`) and
/// to detect root keys a strict profile selection would silently discard.
pub(crate) const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "routes",
    "watch",
    "runtime_journal",
    "idempotent_repo",
    "cache_repo",
    "log_level",
    "timeout_ms",
    "drain_timeout_ms",
    "watch_debounce_ms",
    "components",
    "observability",
    "supervision",
    "platform",
    "stream_caching",
    "beans",
    "languages",
    "security",
    "binds",
    "datasources",
    "jobs",
];

/// Whether `name` is a recognized `CamelConfig` top-level key.
pub fn is_known_top_level_key(name: &str) -> bool {
    KNOWN_TOP_LEVEL_KEYS.contains(&name)
}

/// Near-miss discriminator (cfgdrop2): `Some(target)` when `name` is NOT a
/// known key, is at least 4 chars, and sits within Levenshtein distance 2 of
/// the minimal-distance target among the length-≥ 8 subset of
/// `KNOWN_TOP_LEVEL_KEYS` (`runtime_journal`, `idempotent_repo`,
/// `drain_timeout_ms`, `watch_debounce_ms`, `observability`,
/// `stream_caching`, `datasources`, `supervision`, `components`,
/// `cache_repo`, `timeout_ms`, `log_level`, `languages`, `security`,
/// `platform`). The length filters keep short plausible profile names like
/// `job`/`bind`/`dev`/`qa` out of match range, so far names
/// (e.g. `staging`) keep their unselected-profile semantics: silently
/// dropped beside an active profile by design, rc-cflo warned when no
/// profile is active. Callers only consult this for TABLE-valued root keys.
pub fn near_miss_root_table(name: &str) -> Option<&'static str> {
    if is_known_top_level_key(name) || name.len() < 4 {
        return None;
    }
    let mut best: Option<&'static str> = None;
    let mut best_distance = usize::MAX;
    for target in KNOWN_TOP_LEVEL_KEYS.iter().filter(|t| t.len() >= 8) {
        let distance = levenshtein(name, target);
        if distance <= 2 && distance < best_distance {
            best_distance = distance;
            best = Some(target);
        }
    }
    best
}

/// Classification of a root document's entries against the key policy,
/// shared by both guard doors (the camel-config filesystem loader and the
/// `camel compile --config` mirror) so the two cannot drift. Consumers
/// reject from these lists in struct order — discarded keys first, then
/// misspelled tables — which keeps the guard ordering (discarded beats
/// near-miss, both before profile-selection errors) a single decision.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RootEntryClasses {
    /// Root keys that ARE known top-level keys (any value type) and not
    /// `ROOT_ROUTES_KEY`: strict profile selection would silently discard
    /// them. Sorted by name.
    pub discarded_keys: Vec<String>,
    /// Root TABLES that are not known keys but misspell one (near-miss):
    /// `(name, probable intended key)` pairs. Sorted by name.
    pub misspelled_tables: Vec<(String, &'static str)>,
}

/// Classify a root table's entries into [`RootEntryClasses`]. Both lists
/// are sorted by key name so every consumer inherits deterministic output
/// regardless of `toml::Map` iteration order.
pub fn classify_root_entries<'a>(
    entries: impl Iterator<Item = (&'a str, &'a toml::Value)>,
) -> RootEntryClasses {
    let mut classes = RootEntryClasses::default();
    for (name, value) in entries {
        if name != ROOT_ROUTES_KEY && is_known_top_level_key(name) {
            classes.discarded_keys.push(name.to_string());
        } else if value.is_table()
            && !is_known_top_level_key(name)
            && let Some(target) = near_miss_root_table(name)
        {
            classes.misspelled_tables.push((name.to_string(), target));
        }
    }
    classes.discarded_keys.sort();
    classes.misspelled_tables.sort_by(|a, b| a.0.cmp(&b.0));
    classes
}

/// Plain character-weighted Levenshtein distance, two-row DP (no
/// transposition operation): a swapped pair costs 2. Bounded inputs (the
/// target list is tiny, names are short), so no allocation-avoidance tricks.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_miss_typo_matrix() {
        assert_eq!(near_miss_root_table("obsevrability"), Some("observability"));
        assert_eq!(near_miss_root_table("componets"), Some("components"));
        assert_eq!(
            near_miss_root_table("runtime_jounal"),
            Some("runtime_journal")
        );
        assert_eq!(near_miss_root_table("datasorces"), Some("datasources"));
        assert_eq!(near_miss_root_table("supervison"), Some("supervision"));
    }

    #[test]
    fn near_miss_far_names_stay_none() {
        for name in [
            "staging", "prod", "qa", "canary", "eu_west", "job", "bind", "dev",
        ] {
            assert_eq!(
                near_miss_root_table(name),
                None,
                "{name} must stay far from every known key"
            );
        }
    }

    #[test]
    fn near_miss_known_keys_stay_none() {
        for name in ["routes", "watch", "runtime_journal"] {
            assert_eq!(
                near_miss_root_table(name),
                None,
                "{name} is itself a known key"
            );
        }
    }

    #[test]
    fn levenshtein_cases() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("", "x"), 1);
    }
}
