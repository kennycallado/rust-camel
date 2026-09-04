//! Tier filters for `camel test` (ADR-0069 section 1, spec delta
//! "Symmetric tier filters").
//!
//! `--unit` selects documents whose derived tier is LEAN;
//! `--integration` selects FULL. The two flags are symmetric and
//! mutually exclusive: supplying both is a misuse error surfaced by
//! `config_from_args` before any document is read.
//!
//! Filtering happens by derived tier, so it necessarily happens after
//! the document is read, parsed, and its route source loaded — unlike
//! `--filter-file`, which applies before reading. A nonmatching
//! document admitted through directory expansion is excluded silently;
//! a nonmatching document named explicitly on the command line fails
//! with the `tier-filter-collision` class and exit 2.

use camel_integration_test::Tier;

/// The selected tier filter: `--unit` (LEAN documents only) or
/// `--integration` (FULL documents only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierFilter {
    /// `--unit`: admit only documents deriving [`Tier::Lean`].
    Unit,
    /// `--integration`: admit only documents deriving [`Tier::Full`].
    Integration,
}

impl TierFilter {
    /// Whether a document with this derived tier runs under the filter.
    pub(crate) fn selects(&self, tier: Tier) -> bool {
        match self {
            Self::Unit => tier == Tier::Lean,
            Self::Integration => tier == Tier::Full,
        }
    }

    /// The CLI flag name, for misuse and collision messages.
    pub(crate) fn flag_name(&self) -> &'static str {
        match self {
            Self::Unit => "--unit",
            Self::Integration => "--integration",
        }
    }

    /// The tier name the filter selects, for collision messages.
    pub(crate) fn tier_name(&self) -> &'static str {
        match self {
            Self::Unit => "lean",
            Self::Integration => "full",
        }
    }
}

/// The misuse error for supplying both tier flags: printed to stderr,
/// exit 2, before any document is read.
pub(crate) fn both_flags_message() -> String {
    "--unit and --integration are mutually exclusive tier filters; give at most one".to_string()
}

/// The collision error for an explicitly named document whose derived
/// tier does not match the selected filter (class
/// `tier-filter-collision`, exit 2).
pub(crate) fn collision_message(filter: TierFilter, derived: &str) -> String {
    format!(
        "tier-filter-collision: document derives {derived}, {} selects {}",
        filter.flag_name(),
        filter.tier_name()
    )
}

#[cfg(test)]
mod tests {
    use super::{TierFilter, both_flags_message, collision_message};
    use camel_integration_test::Tier;

    #[test]
    fn filters_select_their_own_tier_only() {
        assert!(TierFilter::Unit.selects(Tier::Lean));
        assert!(!TierFilter::Unit.selects(Tier::Full));
        assert!(TierFilter::Integration.selects(Tier::Full));
        assert!(!TierFilter::Integration.selects(Tier::Lean));
    }

    #[test]
    fn collision_message_names_class_flag_and_tiers() {
        let message = collision_message(TierFilter::Unit, "full");
        assert!(message.starts_with("tier-filter-collision"), "{message}");
        assert!(message.contains("--unit"), "{message}");
        assert!(message.contains("full"), "{message}");
        assert!(message.contains("lean"), "{message}");
    }

    #[test]
    fn both_flags_message_names_both_flags() {
        let message = both_flags_message();
        assert!(
            message.contains("--unit") && message.contains("--integration"),
            "{message}"
        );
    }
}
