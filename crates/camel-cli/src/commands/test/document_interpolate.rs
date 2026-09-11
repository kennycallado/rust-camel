use std::collections::BTreeMap;

use camel_dsl::env_interpolation::interpolate_env_with;

use super::{SUPPORTED_REGISTRY_KINDS, TestDocError, TestDocument};

/// Interpolates `${env:NAME}` / `${env:NAME:-default}` placeholders in one
/// identifier field. The lookup is the document `env:` fixture closure
/// (rc-l7m7t): document env values first, then inline defaults — the
/// ambient environment is still never consulted in the unit tier, mirroring
/// how route sources resolve identifiers at parse time. An unresolved
/// variable surfaces as [`TestDocError::EnvUnresolved`] naming the variable
/// and the field.
fn interpolate_identifier(
    value: &str,
    position: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, TestDocError> {
    interpolate_env_with(value, lookup).map_err(|var| TestDocError::EnvUnresolved {
        var,
        field: position.to_string(),
    })
}

/// Rebuilds one identifier map key-by-key through
/// [`interpolate_identifier`] (values pass through untouched). An
/// already-present resolved key is a collision and is rejected with the
/// error built by `collision`. `lookup` is forwarded to every key
/// interpolation (the document `env:` closure).
fn rebuild_identifier_map<V>(
    map: BTreeMap<String, V>,
    position: &str,
    collision: impl Fn(&str) -> TestDocError,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<BTreeMap<String, V>, TestDocError> {
    let mut rebuilt = BTreeMap::new();
    for (key, value) in map {
        let resolved = interpolate_identifier(&key, position, lookup)?;
        if rebuilt.contains_key(&resolved) {
            return Err(collision(&resolved));
        }
        rebuilt.insert(resolved, value);
    }
    Ok(rebuilt)
}

/// Step (a0): identifier fields interpolate `${env:...}` through the
/// document `env:` fixture closure first, then inline defaults (parity
/// with route sources; see the mock-testkit spec requirement; rc-l7m7t) —
/// the ambient environment is still never consulted in the unit tier.
/// The identifier fields are exactly: (1) `repositories` map keys in every
/// registry map (`cache`, `idempotent`, `claimCheck`); (2) `beans` map
/// keys; (3) `intercepts` map keys (source URIs) and their action target
/// values (`skipTo`, `divertCopyTo`); (4) `mock:` references — `expects`
/// map keys and `sequence` entries; (5) `inputs[].to` values.
///
/// Map keys rebuild key-by-key, leaving values untouched; `sequence`
/// entries and `inputs[].to` interpolate in place. Assertion data (matcher
/// contents, input `body`/`headers`/`expectReply`) stays literal. Two keys
/// of one map that resolve to the same string collide and are rejected
/// with an error naming the map and the resolved value (no silent stub
/// shadowing); `sequence` duplicates stay allowed per the arrival-sequence
/// canon.
pub(super) fn interpolate_identifier_fields(doc: &mut TestDocument) -> Result<(), TestDocError> {
    // The clone is required: this pass mutates other `doc` fields while
    // the lookup closure reads the env map.
    let env = doc.env.clone();
    let lookup = &|name: &str| env.get(name).cloned();
    if let Some(repos) = doc.repositories.as_mut() {
        for (kind, map) in [
            (SUPPORTED_REGISTRY_KINDS[0], &mut repos.cache),
            (SUPPORTED_REGISTRY_KINDS[1], &mut repos.idempotent),
            (SUPPORTED_REGISTRY_KINDS[2], &mut repos.claim_check),
        ] {
            if let Some(map) = map.as_mut() {
                *map = rebuild_identifier_map(
                    std::mem::take(map),
                    &format!("repositories.{kind}"),
                    |resolved| {
                        TestDocError::InvalidRepositories(format!(
                            "repositories.{kind}: duplicate repository name \
                             `{resolved}` after interpolation"
                        ))
                    },
                    lookup,
                )?;
            }
        }
    }
    if let Some(beans) = doc.beans.as_mut() {
        *beans = rebuild_identifier_map(
            std::mem::take(beans),
            "beans",
            |resolved| {
                TestDocError::InvalidBeans(format!(
                    "beans: duplicate bean name `{resolved}` after interpolation"
                ))
            },
            lookup,
        )?;
    }
    // `intercepts:`: source keys interpolate first, then the action
    // targets interpolate in place; the collision error names the resolved
    // source URI.
    if let Some(intercepts) = doc.intercepts.as_mut() {
        let taken = std::mem::take(intercepts);
        *intercepts = rebuild_identifier_map(
            taken,
            "intercepts",
            |resolved| {
                TestDocError::InterceptInvalid(format!(
                    "intercepts: duplicate source `{resolved}` after interpolation"
                ))
            },
            lookup,
        )?;
        for action in intercepts.values_mut() {
            if let Some(target) = action.skip_to.as_mut() {
                *target = interpolate_identifier(target, "intercepts.skipTo", lookup)?;
            }
            if let Some(target) = action.divert_copy_to.as_mut() {
                *target = interpolate_identifier(target, "intercepts.divertCopyTo", lookup)?;
            }
        }
    }
    // `expects:`: keys are `mock:` references; the collision message names
    // the FULL resolved key — scheme stripping happens later, at step (c).
    doc.expects = rebuild_identifier_map(
        std::mem::take(&mut doc.expects),
        "expects",
        |resolved| {
            TestDocError::Yaml(format!(
                "duplicate expectation endpoint `{resolved}` after interpolation"
            ))
        },
        lookup,
    )?;
    // `sequence:` entries interpolate in place; duplicates remain allowed
    // per the arrival-sequence canon — NO collision guard.
    if let Some(sequence) = doc.sequence.as_mut() {
        for (index, entry) in sequence.iter_mut().enumerate() {
            *entry = interpolate_identifier(entry, &format!("sequence[{index}]"), lookup)?;
        }
    }
    // `inputs[].to` interpolates in place; `body`, `headers`, and
    // `expectReply` are assertion data and stay literal.
    for (index, input) in doc.inputs.iter_mut().enumerate() {
        input.to = interpolate_identifier(&input.to, &format!("inputs[{index}].to"), lookup)?;
    }
    Ok(())
}
