//! Depth-bounded property tests for XPath number projection and text roundtrip.
//!
//! All generators are deterministic and bounded by construction. No external
//! dependencies, I/O, threads, or sleeps are involved.

use super::XPathLanguage;
use super::number_to_json;
use camel_api::ExpressionErrorClass;
use camel_language_api::Body;
use camel_language_api::Exchange;
use camel_language_api::Language;
use camel_language_api::LanguageError;
use camel_language_api::Message;
use serde_json::Value as JsonValue;

/// Fixed iteration count per property.
const ITERATIONS: usize = 256;

/// Fixed nonzero seed for [`Xorshift64`]. Any nonzero value is valid; this one
/// is the 64-bit golden-ratio constant used only to make runs reproducible.
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// Maximum number of children generated for `<root>`.
const ROOT_MAX_CHILDREN: u64 = 3;

/// Maximum number of children generated for a non-root element.
const ELEMENT_MAX_CHILDREN: u64 = 3;

/// Maximum nesting depth. `<root>` sits at depth 0; generated elements reach
/// depth 4, so the document tree is at most five levels including `<root>`.
const MAX_DEPTH: usize = 4;

/// Maximum text length (in characters) attached to a single element.
const MAX_TEXT_LEN: u64 = 8;

/// Fixed element-name alphabet.
const ELEMENT_NAMES: [&str; 3] = ["a", "b", "c"];

/// Fixed text alphabet. It excludes `<`, `&`, and `>` so generated text is
/// always safe both as XML character data and as reconstructed XML input.
const TEXT_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// IEEE-754 bit pattern for `+Inf`.
const POSITIVE_INFINITY_BITS: u64 = 0x7ff0_0000_0000_0000;

/// IEEE-754 bit pattern for `-Inf`.
const NEGATIVE_INFINITY_BITS: u64 = 0xfff0_0000_0000_0000;

/// IEEE-754 bit pattern for a quiet `NaN`.
const NAN_BITS: u64 = 0x7ff8_0000_0000_0000;

/// Minimal xorshift64* PRNG.
///
/// The recurrence is the standard xorshift64* step: three xorshift rounds
/// followed by multiplication by the 64-bit scrambler. It is deterministic for
/// a fixed nonzero seed and needs no external crate.
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        assert!(seed != 0, "xorshift64 seed must be nonzero");
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

/// Generate a bounded XML document, returning it together with the expected
/// string-value of `/root` (all descendant text concatenated in document
/// order).
fn gen_xml_with_text(rng: &mut Xorshift64) -> (String, String) {
    let mut xml = String::from("<root>");
    let mut text = String::new();
    let child_count = 1 + (rng.next_u64() % ROOT_MAX_CHILDREN);
    for _ in 0..child_count {
        gen_element(rng, 1, &mut xml, &mut text);
    }
    xml.push_str("</root>");
    (xml, text)
}

/// Generate a bounded XML document; see [`gen_xml_with_text`].
fn gen_xml(rng: &mut Xorshift64) -> String {
    gen_xml_with_text(rng).0
}

fn gen_element(rng: &mut Xorshift64, depth: usize, xml: &mut String, text: &mut String) {
    let name = ELEMENT_NAMES[(rng.next_u64() % ELEMENT_NAMES.len() as u64) as usize];
    xml.push('<');
    xml.push_str(name);
    xml.push('>');

    let text_len = rng.next_u64() % (MAX_TEXT_LEN + 1);
    for _ in 0..text_len {
        let byte = TEXT_ALPHABET[(rng.next_u64() % TEXT_ALPHABET.len() as u64) as usize];
        xml.push(byte as char);
        text.push(byte as char);
    }

    if depth < MAX_DEPTH {
        let child_count = rng.next_u64() % (ELEMENT_MAX_CHILDREN + 1);
        for _ in 0..child_count {
            gen_element(rng, depth + 1, xml, text);
        }
    }

    xml.push_str("</");
    xml.push_str(name);
    xml.push('>');
}

/// Generate a number case, mixing every projection class selected per
/// iteration: finite fractional, finite integral in the i64 interval, finite
/// integral in the u64 interval, finite integral beyond `2^64` in both signs,
/// `+Inf`, `-Inf`, and `NaN`.
fn gen_case(rng: &mut Xorshift64) -> f64 {
    match rng.next_u64() % 8 {
        0 => {
            // Fractional: `n + 0.125` is exact and always has a nonzero
            // fractional part, since `n` is an integer below `2^20`.
            let n = rng.next_u64() % 1_000_000;
            n as f64 + 0.125
        }
        1 => {
            // Integral in [-2^63, 2^63): magnitude below 2^62.
            let magnitude = (rng.next_u64() % (1u64 << 62)) as f64;
            if rng.next_u64() & 1 == 0 {
                magnitude
            } else {
                -magnitude
            }
        }
        2 => {
            // Integral in [2^63, 2^64): every representable value in this
            // interval is `2^63 + m * 2048` with `m < 2^51`.
            let offset = (rng.next_u64() % (1u64 << 51)) as f64 * 2048.0;
            9_223_372_036_854_775_808.0 + offset
        }
        3 => 2f64.powi(65 + (rng.next_u64() % 36) as i32),
        4 => -2f64.powi(65 + (rng.next_u64() % 36) as i32),
        5 => f64::from_bits(POSITIVE_INFINITY_BITS),
        6 => f64::from_bits(NEGATIVE_INFINITY_BITS),
        _ => f64::from_bits(NAN_BITS),
    }
}

fn non_finite_label(value: f64) -> &'static str {
    if value.is_nan() {
        "NaN"
    } else if value.is_sign_negative() {
        "-Inf"
    } else {
        "+Inf"
    }
}

#[test]
fn property_conversion_total_and_typed() {
    let mut rng = Xorshift64::new(SEED);
    for i in 0..ITERATIONS {
        let n = gen_case(&mut rng);
        let result = number_to_json(n);
        if n.is_finite() {
            match result {
                Ok(JsonValue::Number(_)) => {}
                other => panic!(
                    "iteration {i}: finite {n:?} must project to a JSON number, got {other:?}"
                ),
            }
        } else {
            let expected_label = non_finite_label(n);
            let expected_detail = format!("xpath number result is not finite: {expected_label}");
            match result {
                Err(LanguageError::EvalFailure { class, detail, .. }) => {
                    assert_eq!(
                        class,
                        ExpressionErrorClass::Arithmetic,
                        "iteration {i}: non-finite {n:?} must be an Arithmetic failure"
                    );
                    assert_eq!(
                        detail.as_deref(),
                        Some(expected_detail.as_str()),
                        "iteration {i}: detail must label {expected_label}"
                    );
                }
                other => panic!("iteration {i}: non-finite {n:?} must be refused, got {other:?}"),
            }
        }
    }
}

#[test]
fn property_integral_fidelity_roundtrip() {
    let mut rng = Xorshift64::new(SEED);
    for i in 0..ITERATIONS {
        let n = gen_case(&mut rng);
        if let Ok(value) = number_to_json(n) {
            assert_eq!(
                value.as_f64(),
                Some(n),
                "iteration {i}: projection of {n:?} must preserve the mathematical value"
            );
        }
    }
}

#[tokio::test]
async fn property_xml_text_roundtrip() {
    let mut rng = Xorshift64::new(SEED);
    let lang = XPathLanguage::new();
    let root_expr = lang
        .create_expression("/root")
        .expect("valid XPath expression");
    let roundtrip_expr = lang
        .create_expression("/roundtrip")
        .expect("valid XPath expression");

    for i in 0..ITERATIONS {
        let (xml, expected_text) = gen_xml_with_text(&mut rng);
        let exchange = Exchange::new(Message::new(Body::Xml(xml)));
        let result = root_expr
            .evaluate(&exchange)
            .await
            .expect("generated XML must evaluate");
        let text = match result {
            JsonValue::String(text) => text,
            other => panic!("iteration {i}: /root must be a string, got {other:?}"),
        };
        assert_eq!(
            text, expected_text,
            "iteration {i}: /root string-value must concatenate generated text in document order"
        );
        assert!(
            !text.contains('<') && !text.contains('&') && !text.contains('>'),
            "iteration {i}: generated text must be XML-safe, got {text:?}"
        );

        let roundtrip_xml = format!("<roundtrip>{text}</roundtrip>");
        let roundtrip_exchange = Exchange::new(Message::new(Body::Xml(roundtrip_xml)));
        let roundtrip_result = roundtrip_expr
            .evaluate(&roundtrip_exchange)
            .await
            .expect("reconstructed XML must evaluate");
        assert_eq!(
            roundtrip_result,
            JsonValue::String(text),
            "iteration {i}: string-value must survive as input to a second document"
        );
    }
}

#[test]
fn property_generation_is_deterministic() {
    let mut first = Xorshift64::new(SEED);
    let mut second = Xorshift64::new(SEED);
    for i in 0..ITERATIONS {
        let first_xml = gen_xml(&mut first);
        let second_xml = gen_xml(&mut second);
        assert_eq!(
            first_xml, second_xml,
            "iteration {i}: XML must be deterministic"
        );

        let first_number = gen_case(&mut first);
        let second_number = gen_case(&mut second);
        assert_eq!(
            first_number.to_bits(),
            second_number.to_bits(),
            "iteration {i}: number sequence must be deterministic"
        );
    }
}
