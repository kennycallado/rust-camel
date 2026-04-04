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
    Process,
    ProcessFn,
    MapBody,
    SetBodyFn,
    SetHeaderFn,
}

pub const MANDATORY_DECLARATIVE_STEP_KINDS: [DeclarativeStepKind; 15] = [
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
    DeclarativeStepKind::ConvertBodyTo,
    DeclarativeStepKind::Marshal,
    DeclarativeStepKind::Unmarshal,
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
