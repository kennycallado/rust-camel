//! Unit tests for the decision configuration types.

use std::time::Duration;

use super::*;

use camel_component_api::CamelError;

use crate::LlmComponent;
use crate::config::LlmGlobalConfig;
use crate::decision::QuestionKind;
use crate::decision::test_support::TRIAGE_TOML;

#[test]
fn decide_total_timeout_defaults_to_120s() {
    assert_eq!(
        decide_total_timeout_secs(None, None),
        Duration::from_secs(120),
        "the default total deadline must be 120s"
    );
    assert_eq!(
        decide_total_timeout_secs(Some(5), None),
        Duration::from_secs(5),
        "a provider timeout must win over the default"
    );
    assert_eq!(
        decide_total_timeout_secs(None, Some(30)),
        Duration::from_secs(30),
        "a global timeout must win over the default"
    );
    assert_eq!(
        decide_total_timeout_secs(Some(5), Some(60)),
        Duration::from_secs(5),
        "the provider value must win over the global one"
    );
}

#[test]
fn triage_config_parses() {
    let toml_str = format!(
        "[decision_providers.fake]\ntype = \"mock\"\n{}",
        TRIAGE_TOML
    );
    let cfg: LlmGlobalConfig = toml::from_str(&toml_str).expect("triage TOML must parse");

    let set = cfg
        .decision_question_sets
        .get("triage")
        .expect("triage question set must be present");
    let names: Vec<&str> = set.questions.iter().map(|q| q.name.as_str()).collect();
    assert_eq!(names, ["is_billing", "dept", "urgency"]);

    let questions = set.to_questions();
    match &questions[1].kind {
        QuestionKind::Choice { choices } => {
            let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
            assert_eq!(values, ["billing", "tech"]);
        }
        other => panic!("dept should be Choice, got {other:?}"),
    }

    assert_eq!(set.questions[0].threshold, Some(0.8));
}

#[test]
fn all_provider_types_parse() {
    let toml_str = r#"
[decision_providers.oai]
type = "openai"
api_key = "sk-a"
default_model = "gpt-x"

[decision_providers.ts]
type = "typesafe"
api_key = "ts-a"
default_model = "ts-x"

[decision_providers.oll]
type = "ollama"
base_url = "http://localhost:11434"
default_model = "llama3"

[decision_providers.mock]
type = "mock"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("all provider types must parse");

    assert_eq!(cfg.decision_providers["oai"].type_name(), "openai");
    assert_eq!(cfg.decision_providers["ts"].type_name(), "typesafe");
    assert_eq!(cfg.decision_providers["oll"].type_name(), "ollama");
    assert_eq!(cfg.decision_providers["mock"].type_name(), "mock");

    match &cfg.decision_providers["mock"] {
        DecisionProviderConfig::Mock(m) => assert_eq!(m.predicate_probability, 1.0),
        other => panic!("mock entry should be Mock, got {other:?}"),
    }
}

#[test]
fn ollama_api_key_field_rejected_at_parse() {
    let toml_str = r#"
[decision_providers.oll]
type = "ollama"
base_url = "http://localhost:11434"
default_model = "llama3"
api_key = "x"
"#;
    assert!(
        toml::from_str::<LlmGlobalConfig>(toml_str).is_err(),
        "ollama entries must reject api_key"
    );
}

#[test]
fn openai_debug_redacts_api_key() {
    let toml_str = r#"
[decision_providers.oai]
type = "openai"
api_key = "sk-secret-123"
default_model = "gpt-x"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("openai entry must parse");
    let provider = &cfg.decision_providers["oai"];
    let debug = format!("{:?}", provider);
    assert!(
        !debug.contains("sk-secret-123"),
        "Debug output must not leak api_key: {debug}"
    );
}

#[test]
fn accessors_return_policy_fields() {
    let toml_str = r#"
[decision_providers.oai]
type = "openai"
api_key = "k"
default_model = "m"
timeout_secs = 7
max_concurrency = 2

[decision_providers.mock]
type = "mock"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("policy entries must parse");

    let oai = &cfg.decision_providers["oai"];
    assert_eq!(oai.timeout_secs(), Some(7));
    assert_eq!(oai.max_concurrency(), Some(2));

    let mock = &cfg.decision_providers["mock"];
    assert_eq!(mock.timeout_secs(), None);
    assert_eq!(mock.max_concurrency(), None);
    assert!(mock.network_retry().is_none());
}

#[test]
fn ollama_debug_redacts_base_url_userinfo() {
    let toml_str = r#"
[decision_providers.local]
type = "ollama"
base_url = "http://user:s3cret-pw@127.0.0.1:11434"
default_model = "tev1:0.8b"
"#;
    let config: crate::config::LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
    let rendered = format!("{:?}", config.decision_providers);
    assert!(
        !rendered.contains("s3cret-pw"),
        "ollama base_url credentials leaked in Debug: {rendered}"
    );
}

/// Mock provider table used as the provider half of the valid base config.
const MOCK_PROVIDER: &str = "[decision_providers.fake]\ntype = \"mock\"\n";

/// The valid base config: one mock provider plus the triage question set.
fn valid_base_toml() -> String {
    format!("{MOCK_PROVIDER}{TRIAGE_TOML}")
}

/// Replaces the single occurrence of `from` in `src`. Panics if `from` is
/// missing, so a drifted fixture fails loudly instead of passing silently.
fn replace_once(src: &str, from: &str, to: &str) -> String {
    assert!(src.contains(from), "fixture no longer contains {from:?}");
    src.replacen(from, to, 1)
}

/// Parses and validates a config, returning the validation result.
fn validate_toml(toml_str: &str) -> Result<(), CamelError> {
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("test TOML must parse");
    cfg.validate()
}

/// Asserts that validation rejects the config with `CamelError::Config` and
/// returns the message.
fn rejection_message(toml_str: &str) -> String {
    match validate_toml(toml_str) {
        Ok(()) => panic!("expected validation to reject:\n{toml_str}"),
        Err(CamelError::Config(msg)) => msg,
        Err(other) => panic!("expected CamelError::Config, got {other:?}"),
    }
}

/// Valid base config with `triage` duplicated by a second question named `dept`.
fn duplicate_name_toml() -> String {
    format!(
        "{}[[decision_question_sets.triage.questions]]\nname = \"dept\"\ntype = \"predicate\"\ninstructions = \"Duplicate?\"\n",
        valid_base_toml()
    )
}

#[test]
fn valid_triage_config_passes() {
    validate_toml(&valid_base_toml()).expect("valid triage config must pass");
}

#[test]
fn duplicate_question_name_rejected() {
    let msg = rejection_message(&duplicate_name_toml());
    assert!(
        msg.contains("triage") && msg.contains("dept"),
        "message must name the set and question: {msg}"
    );
}

#[test]
fn choice_with_one_option_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        r#"choices = [{ value = "billing", description = "payments" }, { value = "tech" }]"#,
        r#"choices = [{ value = "billing" }]"#,
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("2..=255"), "{msg}");
}

#[test]
fn score_with_eleven_levels_rejected() {
    let levels: Vec<String> = (0..11).map(|i| format!("{{ label = \"l{i}\" }}")).collect();
    let toml_str = replace_once(
        &valid_base_toml(),
        r#"levels = [{ label = "low" }, { label = "medium" }, { label = "high", description = "outage" }]"#,
        &format!("levels = [{}]", levels.join(", ")),
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("2..=10"), "{msg}");
}

#[test]
fn threshold_on_choice_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "instructions = \"Which department?\"\n",
        "instructions = \"Which department?\"\nthreshold = 0.5\n",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("threshold"), "{msg}");
}

#[test]
fn min_confidence_on_predicate_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "threshold = 0.8\n",
        "threshold = 0.8\nmin_confidence = 0.5\n",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("min_confidence"), "{msg}");
}

#[test]
fn levels_on_choice_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "instructions = \"Which department?\"\n",
        "instructions = \"Which department?\"\nlevels = [{ label = \"low\" }, { label = \"high\" }]\n",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("levels"), "{msg}");
}

#[test]
fn threshold_out_of_range_rejected() {
    let toml_str = replace_once(&valid_base_toml(), "threshold = 0.8\n", "threshold = 1.5\n");
    assert!(
        validate_toml(&toml_str).is_err(),
        "threshold 1.5 must be rejected"
    );
}

#[test]
fn invalid_question_name_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "name = \"is_billing\"\n",
        "name = \"bad name\"\n",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("bad name"), "{msg}");
}

#[test]
fn empty_question_set_rejected() {
    let toml_str = format!(
        "{}[decision_question_sets.empty]\nquestions = []\n",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("1..=400"), "{msg}");
}

#[test]
fn blank_openai_api_key_rejected() {
    let toml_str = format!(
        "[decision_providers.oai]\ntype = \"openai\"\napi_key = \"  \"\ndefault_model = \"gpt-x\"\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("api_key"), "{msg}");
}

#[test]
fn zero_timeout_rejected() {
    let toml_str = format!(
        "[decision_providers.oai]\ntype = \"openai\"\napi_key = \"sk-a\"\ndefault_model = \"gpt-x\"\ntimeout_secs = 0\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("timeout_secs"), "{msg}");
}

#[test]
fn mock_probability_out_of_range_rejected() {
    let toml_str = format!(
        "[decision_providers.fake]\ntype = \"mock\"\npredicate_probability = 2.0\n{TRIAGE_TOML}"
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("predicate_probability"), "{msg}");
}

#[test]
fn mock_refuse_invalid_name_rejected() {
    let toml_str = format!(
        "[decision_providers.fake]\ntype = \"mock\"\nrefuse = [\"bad name\"]\n{TRIAGE_TOML}"
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("refuse"), "{msg}");
}

#[test]
fn unknown_default_decision_provider_rejected() {
    let toml_str = format!(
        "default_decision_provider = \"ghost\"\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("ghost"), "{msg}");
}

#[test]
fn blank_instructions_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "instructions = \"Which department?\"",
        "instructions = \" \"",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("blank instructions"), "{msg}");
}

#[test]
fn blank_choice_value_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "{ value = \"tech\" }",
        "{ value = \"\" }",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("blank choice value"), "{msg}");
}

#[test]
fn duplicate_choice_value_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "{ value = \"tech\" }",
        "{ value = \"billing\" }",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("duplicate choice value"), "{msg}");
}

#[test]
fn blank_score_label_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "{ label = \"low\" }",
        "{ label = \"\" }",
    );
    assert!(
        validate_toml(&toml_str).is_err(),
        "blank score label must be rejected"
    );
}

#[test]
fn choices_on_predicate_rejected() {
    let toml_str = replace_once(
        &valid_base_toml(),
        "threshold = 0.8\n",
        "threshold = 0.8\nchoices = [{ value = \"yes\" }, { value = \"no\" }]\n",
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("choices"), "{msg}");
}

#[test]
fn four_hundred_one_questions_rejected() {
    use std::fmt::Write as _;

    let mut toml_str = String::from(MOCK_PROVIDER);
    for i in 0..401 {
        writeln!(
            toml_str,
            "[[decision_question_sets.triage.questions]]\nname = \"q{i}\"\ntype = \"predicate\"\ninstructions = \"Question {i}?\""
        )
        .expect("writing to a String cannot fail");
    }
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("1..=400"), "{msg}");
}

#[test]
fn blank_base_url_rejected() {
    let toml_str = format!(
        "[decision_providers.oll]\ntype = \"ollama\"\nbase_url = \" \"\ndefault_model = \"llama3\"\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("base_url"), "{msg}");
}

#[test]
fn blank_default_model_rejected() {
    let toml_str = format!(
        "[decision_providers.oai]\ntype = \"openai\"\napi_key = \"sk-a\"\ndefault_model = \"\"\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("default_model"), "{msg}");
}

#[test]
fn zero_max_concurrency_rejected() {
    let toml_str = format!(
        "[decision_providers.oai]\ntype = \"openai\"\napi_key = \"sk-a\"\ndefault_model = \"gpt-x\"\nmax_concurrency = 0\n{}",
        valid_base_toml()
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("max_concurrency"), "{msg}");
}

#[test]
fn component_new_runs_validate() {
    let cfg: LlmGlobalConfig =
        toml::from_str(&duplicate_name_toml()).expect("test TOML must parse");
    assert!(
        LlmComponent::new(cfg).is_err(),
        "LlmComponent::new must run config validation"
    );
}

#[test]
fn threshold_nan_rejected() {
    let toml_str = replace_once(&valid_base_toml(), "threshold = 0.8", "threshold = nan");
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("threshold"), "{msg}");
}

#[test]
fn mock_probability_nan_rejected() {
    let toml_str = format!(
        "[decision_providers.fake]\ntype = \"mock\"\npredicate_probability = nan\n{TRIAGE_TOML}"
    );
    let msg = rejection_message(&toml_str);
    assert!(msg.contains("predicate_probability"), "{msg}");
}

#[test]
fn question_name_length_boundary() {
    let ok_name = "a".repeat(64);
    let ok = replace_once(
        &valid_base_toml(),
        "name = \"dept\"",
        &format!("name = \"{ok_name}\""),
    );
    assert!(validate_toml(&ok).is_ok(), "64-char name must be accepted");

    let long_name = "a".repeat(65);
    let bad = replace_once(
        &valid_base_toml(),
        "name = \"dept\"",
        &format!("name = \"{long_name}\""),
    );
    let msg = rejection_message(&bad);
    assert!(msg.contains("invalid question name"), "{msg}");
}
