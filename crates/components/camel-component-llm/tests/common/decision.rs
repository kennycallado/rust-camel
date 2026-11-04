#![allow(dead_code)]

//! Shared decision-question fixtures for integration tests.
//! Copied from the canonical "Shared test fixtures" block in
//! `openspec/changes/llm-decisions/tasks.md`.

/// TOML form of the triage question set. Provider tables are prepended by tests.
pub const TRIAGE_TOML: &str = r#"
[[decision_question_sets.triage.questions]]
name = "is_billing"
type = "predicate"
instructions = "Is this a billing issue?"
threshold = 0.8

[[decision_question_sets.triage.questions]]
name = "dept"
type = "choice"
instructions = "Which department?"
choices = [{ value = "billing", description = "payments" }, { value = "tech" }]

[[decision_question_sets.triage.questions]]
name = "urgency"
type = "score"
instructions = "How urgent?"
levels = [{ label = "low" }, { label = "medium" }, { label = "high", description = "outage" }]
"#;

/// Canned OpenAI-style response for the triage set.
pub const OPENAI_TRIAGE_RESPONSE: &str = r#"{"model":"gpt-6-luna","answers":[
 {"type":"predicate","name":"is_billing","probability":0.97},
 {"type":"choice","name":"dept","choice":"billing","confidence":0.99,
  "probabilities":[{"value":"billing","probability":0.99},{"value":"tech","probability":0.01}]},
 {"type":"score","name":"urgency","score":0.96,"confidence":0.17,
  "probabilities":[{"label":"low","value":0,"probability":0.29},{"label":"medium","value":1,"probability":0.45},{"label":"high","value":2,"probability":0.26}]}],
 "usage":{"input_tokens":2000,"input_tokens_details":{"cached_tokens":1500,"cache_write_tokens":0},"output_tokens":0,"total_tokens":2000}}"#;

/// Canned System One-style response for the triage set.
pub const SYSTEM_ONE_TRIAGE_RESPONSE: &str = r#"{"model":"tev1:0.8b","answers":{
 "is_billing":{"type":"noul","noul":0.97},
 "dept":{"type":"choice","choice":"billing","confidence":0.99,"probabilities":{"billing":0.99,"tech":0.01}},
 "urgency":{"type":"score","score":0.96,"confidence":0.17,"legend":{"0":"low","1":"medium","2":"high"},
            "probabilities":{"0":0.29,"1":0.45,"2":0.26}}},
 "usage":{"input_tokens":375,"output_tokens":0}}"#;
