use bindings::camel::plugin::types::{WasmError, WasmExchange};
use bindings::Guest;

mod bindings {
    wit_bindgen::generate!({
        world: "authorization-policy",
        path: "wit",
    });
}

struct InitCheck;

/// Expected config pairs that init() validates against.
/// Sorted by key for deterministic comparison.
const EXPECTED_CONFIG: &[( & str,  & str)] = &[
    ("ldap_url", "ldap://corp"),
    ("retry", "3"),
];

impl Guest for InitCheck {
    fn init(config: Vec<(String, String)>) -> Result<(), String> {
        let mut sorted_actual = config.clone();
        sorted_actual.sort_by(|a, b| a.0.cmp(&b.0));
        let expected: Vec<(String, String)> = EXPECTED_CONFIG
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if sorted_actual == expected {
            Ok(())
        } else {
            Err(format!("init() expected {:?}, got {:?}", expected, sorted_actual))
        }
    }

    fn evaluate(exchange: WasmExchange) -> Result<Option<String>, WasmError> {
        let _ = exchange;
        Ok(None)
    }
}

bindings::export!(InitCheck with_types_in bindings);
