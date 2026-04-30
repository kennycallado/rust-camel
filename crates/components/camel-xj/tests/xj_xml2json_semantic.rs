use camel_xj::XML_TO_JSON_XSLT;

fn assert_template_present(xslt: &str, template_id: &str) {
    assert!(
        xslt.contains(template_id),
        "XSLT missing expected template/construct: {template_id}"
    );
}

#[test]
fn xslt_contains_for_each_group() {
    assert_template_present(XML_TO_JSON_XSLT, "xsl:for-each-group");
}

#[test]
fn xslt_contains_group_by_local_name() {
    assert_template_present(XML_TO_JSON_XSLT, "group-by=\"local-name()\"");
}

#[test]
fn xslt_contains_current_group() {
    assert_template_present(XML_TO_JSON_XSLT, "current-group()");
}

#[test]
fn xslt_contains_attribute_axis() {
    assert_template_present(XML_TO_JSON_XSLT, "@*");
}

#[test]
fn xslt_contains_at_prefix_for_attrs() {
    assert_template_present(XML_TO_JSON_XSLT, "concat('@', local-name())");
}

#[test]
fn xslt_contains_text_key() {
    assert_template_present(XML_TO_JSON_XSLT, "#text");
}

#[test]
fn xslt_contains_null_for_self_closing() {
    assert_template_present(XML_TO_JSON_XSLT, "not(node()) and not(@*)");
}

#[test]
fn xslt_escapes_carriage_return() {
    assert_template_present(XML_TO_JSON_XSLT, "&#13;");
}

#[test]
fn xslt_escapes_tab() {
    assert_template_present(XML_TO_JSON_XSLT, "&#9;");
}

#[test]
fn xslt_escapes_backspace() {
    assert_template_present(XML_TO_JSON_XSLT, "&#8;");
}

#[test]
fn xslt_escapes_form_feed() {
    assert_template_present(XML_TO_JSON_XSLT, "&#12;");
}

#[test]
fn xslt_contains_emit_object_template() {
    assert_template_present(XML_TO_JSON_XSLT, "name=\"emit-object\"");
}

#[test]
fn xslt_reverse_mode_preserved() {
    assert_template_present(XML_TO_JSON_XSLT, "mode=\"xj-reverse\"");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='object'");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='array'");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='string'");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='int'");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='boolean'");
    assert_template_present(XML_TO_JSON_XSLT, "@xj:type='null'");
}
