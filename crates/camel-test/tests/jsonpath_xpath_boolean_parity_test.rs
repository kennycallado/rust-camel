//! Parity table: JSONPath `Matches` predicate mode vs XPath `boolean()`.
//!
//! Mission 399 (bd rc-lmfjx): `jsonpath` predicates could not express
//! "there is at least one match" while `xpath` exposes the core `boolean()`
//! function. With `predicate_mode: Matches` both language surfaces must
//! agree on match-existence semantics for equivalent fixtures, and a sole
//! boolean must keep its value on both sides (XPath `boolean(false())` is
//! false, not "non-empty node → true").

use camel_api::body::Body;
use camel_api::{Exchange, Message, Value};
use camel_language_api::Language;
use camel_language_jsonpath::{JsonPathConfig, JsonPathLanguage, JsonPathPredicateMode};
use camel_language_xpath::XPathLanguage;

const TWO_ITEMS_JSON: &str = r#"{"items":[{"price":20},{"price":5}]}"#;
const TWO_ITEMS_XML: &str = "<order><items><item><price>20</price></item>\
<item><price>5</price></item></items></order>";

const ONE_CHEAP_ITEM_JSON: &str = r#"{"items":[{"price":5}]}"#;
const ONE_CHEAP_ITEM_XML: &str = "<order><items><item><price>5</price></item></items></order>";

fn json_exchange(json: &str) -> Exchange {
    let value: Value = serde_json::from_str(json).unwrap();
    Exchange::new(Message::new(Body::Json(value)))
}

fn xml_exchange(xml: &str) -> Exchange {
    Exchange::new(Message::new(Body::Xml(xml.to_string())))
}

fn jsonpath_matches() -> JsonPathLanguage {
    JsonPathLanguage::with_config(JsonPathConfig {
        predicate_mode: JsonPathPredicateMode::Matches,
        ..JsonPathConfig::default()
    })
}

#[tokio::test]
async fn jsonpath_matches_mode_parity_with_xpath_boolean() {
    // (label, json body, xml body, jsonpath query, xpath query, expected)
    let table = [
        (
            "two item matches",
            TWO_ITEMS_JSON,
            TWO_ITEMS_XML,
            "$.items[?@.price>10]",
            "boolean(/order/items/item[price>10])",
            true,
        ),
        (
            "filter leaves zero matches",
            ONE_CHEAP_ITEM_JSON,
            ONE_CHEAP_ITEM_XML,
            "$.items[?@.price>10]",
            "boolean(/order/items/item[price>10])",
            false,
        ),
        (
            "missing path / absent node",
            r#"{"items":[]}"#,
            "<order><items /></order>",
            "$.nope",
            "boolean(/order/nope)",
            false,
        ),
        (
            "sole boolean true",
            r#"{"flag":true}"#,
            "<order><flag>true</flag></order>",
            "$.flag",
            "boolean(true())",
            true,
        ),
        (
            "sole boolean false stays false",
            r#"{"flag":false}"#,
            "<order><flag>false</flag></order>",
            "$.flag",
            "boolean(false())",
            false,
        ),
    ];

    let jp = jsonpath_matches();
    let xp = XPathLanguage::new();

    for (label, json, xml, jp_query, xp_query, expected) in table {
        let jp_pred = jp
            .create_predicate(jp_query)
            .unwrap_or_else(|e| panic!("jsonpath compile failed for {label}: {e:?}"));
        let jp_got = jp_pred
            .matches(&json_exchange(json))
            .await
            .unwrap_or_else(|e| panic!("jsonpath matches failed for {label}: {e:?}"));

        let xp_pred = xp
            .create_predicate(xp_query)
            .unwrap_or_else(|e| panic!("xpath compile failed for {label}: {e:?}"));
        let xp_got = xp_pred
            .matches(&xml_exchange(xml))
            .await
            .unwrap_or_else(|e| panic!("xpath matches failed for {label}: {e:?}"));

        assert_eq!(jp_got, expected, "jsonpath side of {label}");
        assert_eq!(xp_got, expected, "xpath side of {label}");
        assert_eq!(jp_got, xp_got, "parity broken for {label}");
    }
}
