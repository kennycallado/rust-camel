#[test]
fn content_type_can_be_matched_exhaustively_downstream() {
    fn match_ct(ct: camel_api::ContentType) -> u8 {
        match ct {
            camel_api::ContentType::Bytes => 0,
            camel_api::ContentType::Text => 1,
            camel_api::ContentType::Json => 2,
            camel_api::ContentType::Xml => 3,
        }
    }
    assert_eq!(match_ct(camel_api::ContentType::Bytes), 0);
    assert_eq!(match_ct(camel_api::ContentType::Json), 2);
}
