//! Macro expansion baselines for the syn 2 -> 3 migration (openspec
//! change `syn3`, Task 1.1).
//!
//! These tests lock the PRE-pin-flip behavior of `camel-bean-macros`:
//! exact `TokenStream::to_string()` bytes for the success path and exact
//! `syn::Error` Display strings for the reject paths are compared
//! byte-for-byte against committed goldens under `src/baselines/`.
//! After the `syn = "3"` pin flip the same tests must pass
//! byte-identical — that is the behavior-neutrality proof for this
//! crate.
//!
//! Regenerate: `UPDATE_GOLDENS=1 cargo test -p camel-bean-macros --lib`.

use super::bean_impl_gen;
use crate::handler::parse_handler_method;
use syn::{ImplItemFn, ItemImpl, parse_quote};

/// Regeneration switch: when `UPDATE_GOLDENS=1` is set, write the
/// baseline files instead of comparing them.
fn update_goldens() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}

fn baseline_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/baselines")
        .join(name)
}

/// Byte-for-byte baseline lock for one expansion artifact (generated
/// token stream or error diagnostic).
fn check(name: &str, produced: String) {
    let path = baseline_path(name);
    if update_goldens() {
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("baseline {name} has no parent dir"));
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create baselines dir: {e}"));
        std::fs::write(&path, &produced).unwrap_or_else(|e| panic!("write baseline {name}: {e}"));
    } else {
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read baseline {name}: {e} (capture: UPDATE_GOLDENS=1)"));
        assert_eq!(expected, produced, "baseline {name} drifted");
    }
}

/// Representative ok case: two `#[handler]` methods (one body handler
/// with `Result<String, String>` return, one unit-return handler) fed
/// to `bean_impl_gen`; the full expansion bytes are locked.
#[test]
fn bean_impl_expansion_baseline_ok() {
    let item: ItemImpl = parse_quote! {
        impl OrderService {
            #[handler]
            pub async fn process(&self, body: String) -> Result<String, String> {
                Ok(body)
            }
            #[handler]
            pub async fn notify(&self) {}
        }
    };
    match bean_impl_gen(item) {
        Ok(tokens) => check("bean_impl_ok.txt", tokens.to_string()),
        Err(e) => panic!("bean_impl_gen should succeed for valid input, got: {e}"),
    }
}

/// Generic impl blocks are rejected with the BEAN-MACROS-003 diagnostic.
#[test]
fn bean_impl_baseline_generic_reject() {
    let item: ItemImpl = parse_quote! {
        impl<T> GenericService<T> {
            #[handler]
            pub async fn process(&self, body: T) -> Result<T, String> {
                Ok(body)
            }
        }
    };
    match bean_impl_gen(item) {
        Ok(_) => panic!("bean_impl_gen should reject generic impl blocks"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("bean_impl does not support generic types"),
                "unexpected diagnostic: {msg}"
            );
            check("bean_impl_generic_reject.txt", msg);
        }
    }
}

/// Impl blocks without any `#[handler]` method are rejected.
#[test]
fn bean_impl_baseline_no_handler_reject() {
    let item: ItemImpl = parse_quote! {
        impl MyService {
            pub async fn process(&self) {}
        }
    };
    match bean_impl_gen(item) {
        Ok(_) => panic!("bean_impl_gen should reject impl blocks without handlers"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("No #[handler] methods found in impl block"),
                "unexpected diagnostic: {msg}"
            );
            check("bean_impl_no_handler_reject.txt", msg);
        }
    }
}

/// By-value `self` receiver is rejected before the asyncness check.
#[test]
fn handler_baseline_self_reject() {
    let method: ImplItemFn = parse_quote! {
        #[handler]
        pub fn handle(self) {}
    };
    match parse_handler_method(&method) {
        Ok(_) => panic!("parse_handler_method should reject by-value self receivers"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("Handler methods must use &self, not self"),
                "unexpected diagnostic: {msg}"
            );
            check("handler_self_reject.txt", msg);
        }
    }
}

/// Sync (non-async) handler methods are rejected.
#[test]
fn handler_baseline_nonasync_reject() {
    let method: ImplItemFn = parse_quote! {
        #[handler]
        pub fn handle(&self) {}
    };
    match parse_handler_method(&method) {
        Ok(_) => panic!("parse_handler_method should reject non-async handlers"),
        Err(e) => check("handler_nonasync_reject.txt", e.to_string()),
    }
}

/// Duplicate parameter names are rejected.
#[test]
fn handler_baseline_dup_param_reject() {
    let method: ImplItemFn = parse_quote! {
        #[handler]
        pub async fn h(&self, a: String, a: String) {}
    };
    match parse_handler_method(&method) {
        Ok(_) => panic!("parse_handler_method should reject duplicate parameter names"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("Duplicate parameter name"),
                "unexpected diagnostic: {msg}"
            );
            check("handler_dup_param_reject.txt", msg);
        }
    }
}
