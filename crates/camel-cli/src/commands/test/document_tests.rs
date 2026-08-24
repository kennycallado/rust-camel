use super::document::*;
use std::time::Duration;

fn err_of(yaml: &str) -> TestDocError {
    match parse_test_document(yaml) {
        Ok(doc) => panic!("document should fail to parse, got: {doc:?}"),
        Err(e) => e,
    }
}

mod beans;
mod intercepts;
mod parsing;
mod reply;
mod repositories;
