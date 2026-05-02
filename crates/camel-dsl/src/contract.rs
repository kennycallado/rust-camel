#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarativeStepKind {
    To,
    Log,
    SetHeader,
    SetBody,
    Filter,
    Choice,
    Split,
    Aggregate,
    WireTap,
    Multicast,
    Stop,
    Script,
    ConvertBodyTo,
    Marshal,
    Unmarshal,
    Bean,
    DynamicRouter,
    LoadBalance,
    RoutingSlip,
    Throttle,
    RecipientList,
    Process,
    ProcessFn,
    MapBody,
    SetBodyFn,
    SetHeaderFn,
    StreamCache,
}

pub const MANDATORY_DECLARATIVE_STEP_KINDS: [DeclarativeStepKind; 22] = [
    DeclarativeStepKind::To,
    DeclarativeStepKind::Log,
    DeclarativeStepKind::SetHeader,
    DeclarativeStepKind::SetBody,
    DeclarativeStepKind::Filter,
    DeclarativeStepKind::Choice,
    DeclarativeStepKind::Split,
    DeclarativeStepKind::Aggregate,
    DeclarativeStepKind::WireTap,
    DeclarativeStepKind::Multicast,
    DeclarativeStepKind::Stop,
    DeclarativeStepKind::Script,
    DeclarativeStepKind::StreamCache,
    DeclarativeStepKind::ConvertBodyTo,
    DeclarativeStepKind::Marshal,
    DeclarativeStepKind::Unmarshal,
    DeclarativeStepKind::Bean,
    DeclarativeStepKind::DynamicRouter,
    DeclarativeStepKind::LoadBalance,
    DeclarativeStepKind::RoutingSlip,
    DeclarativeStepKind::Throttle,
    DeclarativeStepKind::RecipientList,
];

pub fn is_rust_only_kind(kind: DeclarativeStepKind) -> bool {
    matches!(
        kind,
        DeclarativeStepKind::Process
            | DeclarativeStepKind::ProcessFn
            | DeclarativeStepKind::MapBody
            | DeclarativeStepKind::SetBodyFn
            | DeclarativeStepKind::SetHeaderFn
    )
}

pub fn mandatory_declarative_step_kinds() -> &'static [DeclarativeStepKind] {
    &MANDATORY_DECLARATIVE_STEP_KINDS
}

pub const fn assert_contract_coverage(implemented: &[DeclarativeStepKind]) {
    let mut i = 0;
    while i < MANDATORY_DECLARATIVE_STEP_KINDS.len() {
        let required = MANDATORY_DECLARATIVE_STEP_KINDS[i];
        let mut found = false;

        let mut j = 0;
        while j < implemented.len() {
            if implemented[j] as u8 == required as u8 {
                found = true;
                break;
            }
            j += 1;
        }

        if !found {
            panic!("DSL is missing mandatory declarative step");
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_rust_only_kind_true() {
        assert!(is_rust_only_kind(DeclarativeStepKind::Process));
        assert!(is_rust_only_kind(DeclarativeStepKind::ProcessFn));
        assert!(is_rust_only_kind(DeclarativeStepKind::MapBody));
        assert!(is_rust_only_kind(DeclarativeStepKind::SetBodyFn));
        assert!(is_rust_only_kind(DeclarativeStepKind::SetHeaderFn));
    }

    #[test]
    fn is_rust_only_kind_false() {
        assert!(!is_rust_only_kind(DeclarativeStepKind::To));
        assert!(!is_rust_only_kind(DeclarativeStepKind::Log));
        assert!(!is_rust_only_kind(DeclarativeStepKind::Filter));
        assert!(!is_rust_only_kind(DeclarativeStepKind::Choice));
        assert!(!is_rust_only_kind(DeclarativeStepKind::Multicast));
    }

    #[test]
    fn mandatory_kinds_has_22_entries() {
        assert_eq!(MANDATORY_DECLARATIVE_STEP_KINDS.len(), 22);
        assert_eq!(mandatory_declarative_step_kinds().len(), 22);
    }

    #[test]
    fn assert_contract_coverage_passes_with_all() {
        assert_contract_coverage(&MANDATORY_DECLARATIVE_STEP_KINDS);
    }

    #[test]
    #[should_panic(expected = "DSL is missing mandatory declarative step")]
    fn assert_contract_coverage_panics_on_missing() {
        assert_contract_coverage(&[DeclarativeStepKind::To, DeclarativeStepKind::Log]);
    }

    #[test]
    fn kind_equality() {
        assert_eq!(DeclarativeStepKind::To, DeclarativeStepKind::To);
        assert_ne!(DeclarativeStepKind::To, DeclarativeStepKind::Log);
    }
}
