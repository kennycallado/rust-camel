//! # Body::Xml Example
//!
//! This example demonstrates the `Body::Xml` variant as a first-class body type:
//!
//! - Constructing `Body::Xml` directly
//! - Accessing XML content with `as_xml()`
//! - Converting between `Body::Xml` and `Body::Text`
//! - Converting between `Body::Json` and `Body::Xml` (via body_converter)
//! - XML validation during conversion
//! - Error handling for invalid XML

use camel_api::body::Body;

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              Body::Xml Example - XML Body Type               ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // =========================================================================
    // 1. Constructing Body::Xml directly
    // =========================================================================
    println!("1. Constructing Body::Xml directly");
    println!("   ─────────────────────────────────");
    let xml_body = Body::Xml("<order><id>42</id></order>".to_string());
    println!("   Created: Body::Xml(\"<order><id>42</id></order>\")");
    println!();

    // =========================================================================
    // 2. Accessing XML content with as_xml()
    // =========================================================================
    println!("2. Accessing XML content with as_xml()");
    println!("   ─────────────────────────────────────");
    if let Some(xml_str) = xml_body.as_xml() {
        println!("   as_xml() returned: {:?}", xml_str);
    }
    println!();

    // =========================================================================
    // 3. Converting Body::Xml → Body::Text
    // =========================================================================
    println!("3. Converting Body::Xml → Body::Text");
    println!("   ───────────────────────────────────");
    let xml_body = Body::Xml("<order><id>42</id></order>".to_string());
    let text_body = xml_body.try_into_text().unwrap();
    if let Some(text) = text_body.as_text() {
        println!("   Converted to Text: {:?}", text);
    }
    println!();

    // =========================================================================
    // 4. Converting Body::Text (valid XML) → Body::Xml
    // =========================================================================
    println!("4. Converting Body::Text (valid XML) → Body::Xml");
    println!("   ──────────────────────────────────────────────");
    let text_with_xml = Body::Text("<product><name>Widget</name></product>".to_string());
    let xml_body = text_with_xml.try_into_xml().unwrap();
    if let Some(xml) = xml_body.as_xml() {
        println!("   Converted Text to Xml: {:?}", xml);
    }
    println!();

    // =========================================================================
    // 5. Invalid XML is rejected
    // =========================================================================
    println!("5. Invalid XML is rejected");
    println!("   ────────────────────────────");
    let invalid_text = Body::Text("not xml".to_string());
    match invalid_text.try_into_xml() {
        Ok(_) => println!("   ERROR: Should have failed!"),
        Err(e) => println!("   Expected error: {}", e),
    }
    println!();

    // =========================================================================
    // 6. JSON → XML conversion (via body_converter)
    // =========================================================================
    println!("6. Converting Body::Json → Body::Xml");
    println!("   ──────────────────────────────────");
    let json_body = Body::Json(serde_json::json!({"key": "value"}));
    match json_body.try_into_xml() {
        Ok(xml) => {
            if let Some(s) = xml.as_xml() {
                println!("   Converted Json to Xml: {:?}", s);
            }
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                    Example Complete                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
}
