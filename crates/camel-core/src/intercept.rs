//! Route send-point interception rules.

use camel_api::CamelError;

/// Rule that maps a send URI to an interception action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptRule {
    /// Source URI to intercept (exact match).
    pub uri: String,
    /// Action to apply when the source matches.
    pub action: InterceptAction,
}

/// Action to apply when a send URI matches a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterceptAction {
    /// Skip the original send and redirect to the target.
    SkipTo { uri: String },
    /// Copy the exchange to the target and continue to the real destination.
    DivertCopyTo { uri: String },
}

/// Ordered collection of interception rules.
#[derive(Debug, Clone, Default)]
pub struct InterceptRules {
    rules: Vec<InterceptRule>,
}

impl InterceptRules {
    /// Create rules from a vector.
    ///
    /// Validates that every action target starts with `mock:`.
    /// Returns `CamelError::Config` that contains the rule index and the
    /// offending target URI when validation fails.
    pub fn new(rules: Vec<InterceptRule>) -> Result<Self, CamelError> {
        for (idx, rule) in rules.iter().enumerate() {
            let target = match &rule.action {
                InterceptAction::SkipTo { uri } => uri,
                InterceptAction::DivertCopyTo { uri } => uri,
            };
            if !target.starts_with("mock:") {
                return Err(CamelError::Config(format!(
                    "rule {idx}: intercept target '{target}' must start with 'mock:'"
                )));
            }
        }
        Ok(Self { rules })
    }

    /// Return the first matching action for `send_uri`.
    ///
    /// Uses exact string equality and respects declaration order.
    pub fn lookup(&self, send_uri: &str) -> Option<&InterceptAction> {
        for rule in &self.rules {
            if rule.uri == send_uri {
                return Some(&rule.action);
            }
        }
        None
    }

    /// Return true when no rules are present.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_mock_action_targets_are_rejected_at_rule_construction() {
        let skip_bad = InterceptRule {
            uri: "kafka:x".into(),
            action: InterceptAction::SkipTo {
                uri: "direct:y".into(),
            },
        };
        let divert_bad = InterceptRule {
            uri: "kafka:z".into(),
            action: InterceptAction::DivertCopyTo {
                uri: "seda:w".into(),
            },
        };

        let err = InterceptRules::new(vec![skip_bad.clone()]).unwrap_err(); // allow-unwrap
        match err {
            CamelError::Config(msg) => {
                assert!(msg.contains("0"));
                assert!(msg.contains("direct:y"));
            }
            other => panic!("expected Config, got {other:?}"),
        }

        let err = InterceptRules::new(vec![divert_bad.clone()]).unwrap_err(); // allow-unwrap
        match err {
            CamelError::Config(msg) => {
                assert!(msg.contains("0"));
                assert!(msg.contains("seda:w"));
            }
            other => panic!("expected Config, got {other:?}"),
        }

        let err = InterceptRules::new(vec![skip_bad.clone(), divert_bad.clone()]).unwrap_err(); // allow-unwrap
        match err {
            CamelError::Config(msg) => {
                assert!(msg.contains("0"));
                assert!(msg.contains("direct:y"));
            }
            other => panic!("expected Config, got {other:?}"),
        }

        // Rule-index propagation must reflect position, not a hardcoded "0".
        let valid = InterceptRule {
            uri: "kafka:ok".into(),
            action: InterceptAction::SkipTo {
                uri: "mock:ok".into(),
            },
        };
        let err = InterceptRules::new(vec![valid, divert_bad]).unwrap_err(); // allow-unwrap
        match err {
            CamelError::Config(msg) => {
                assert!(msg.contains("rule 1:"));
                assert!(msg.contains("seda:w"));
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_uris_preserve_declaration_order() {
        assert!(InterceptRules::default().is_empty());
        assert!(InterceptRules::default().lookup("x").is_none());

        let r1 = InterceptRule {
            uri: "seda:out".into(),
            action: InterceptAction::SkipTo {
                uri: "mock:a".into(),
            },
        };
        let r2 = InterceptRule {
            uri: "seda:out".into(),
            action: InterceptAction::SkipTo {
                uri: "mock:b".into(),
            },
        };
        let rules = InterceptRules::new(vec![r1, r2]).expect("valid mock targets"); // allow-unwrap
        assert_eq!(
            rules.lookup("seda:out"),
            Some(&InterceptAction::SkipTo {
                uri: "mock:a".into()
            })
        );
        assert_eq!(rules.lookup("seda:out2"), None);
    }

    #[test]
    fn mock_targets_accepted() {
        let rules = InterceptRules::new(vec![
            InterceptRule {
                uri: "kafka:x".into(),
                action: InterceptAction::SkipTo {
                    uri: "mock:y".into(),
                },
            },
            InterceptRule {
                uri: "kafka:z".into(),
                action: InterceptAction::DivertCopyTo {
                    uri: "mock:w".into(),
                },
            },
        ])
        .expect("valid mock targets"); // allow-unwrap
        assert_eq!(
            rules.lookup("kafka:x"),
            Some(&InterceptAction::SkipTo {
                uri: "mock:y".into()
            })
        );
        assert_eq!(
            rules.lookup("kafka:z"),
            Some(&InterceptAction::DivertCopyTo {
                uri: "mock:w".into()
            })
        );
    }
}
