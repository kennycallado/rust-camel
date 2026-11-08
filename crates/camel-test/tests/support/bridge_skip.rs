#![allow(dead_code)] // not every test binary including `support` uses it

//! Suite-level skip gates for the native bridge integration suites.
//!
//! Each gate runs pre-main via `#[ctor::ctor(unsafe)]` in the suite's own test
//! binary, before libtest discovers any case. When the suite's env
//! selectors are absent AND the worktree-native bridge binary is absent,
//! the gate prints exactly one `SKIP <suite>: ...` line to stderr and the
//! caller exits 0, so an unprovisioned workspace reports an explicit,
//! actionable skip instead of a wall of panics. Presence of a selector —
//! even set to an invalid value — always suppresses the skip, so a
//! misconfiguration stays a hard failure inside the suite's own require
//! helpers and is never converted into a skip.

/// Decides whether an xml-bridge suite may run, printing the one-line
/// SKIP diagnostic and returning `false` when it must skip instead.
///
/// Skips only when `CAMEL_XML_BRIDGE_BINARY_PATH` is absent AND the
/// worktree-native binary (`bridges/xml/build/native/xml-bridge`, as
/// built by `cargo xtask build-xml-bridge`) is absent. A set selector —
/// even set to an invalid value — returns `true` so the suite's own
/// require helpers fail hard.
#[cfg(feature = "integration-tests")]
pub fn xml_bridge_suite_may_run(suite: &str) -> bool {
    if std::env::var_os(super::xml_bridge::ENV_CAMEL).is_some() {
        return true;
    }
    if super::xml_bridge::worktree_native_binary().is_some_and(|path| path.is_file()) {
        return true;
    }
    eprintln!(
        "SKIP {suite}: {} is not set and the worktree xml-bridge binary is absent. \
         Provision with `cargo xtask build-xml-bridge`, then \
         `export {}=\"$PWD/bridges/xml/build/native/xml-bridge\"` (unset it to skip).",
        super::xml_bridge::ENV_CAMEL,
        super::xml_bridge::ENV_CAMEL,
    );
    false
}

/// Decides whether the cxf bridge suite may run, printing the one-line
/// SKIP diagnostic and returning `false` when it must skip instead.
///
/// Skips only when `CAMEL_CXF_BRIDGE_BINARY_PATH` and `CXF_BRIDGE_PATH`
/// are both absent AND the worktree-native binary
/// (`bridges/cxf/build/native/cxf-bridge`, as built by
/// `cargo xtask build-cxf-bridge`) is absent. A set selector — even set
/// to an invalid value — returns `true` so the suite's own require
/// helpers fail hard.
#[cfg(any(feature = "integration-tests", feature = "integration-tests-cxf"))]
pub fn cxf_bridge_suite_may_run(suite: &str) -> bool {
    if std::env::var_os(super::cxf::ENV_CAMEL).is_some()
        || std::env::var_os(super::cxf::ENV_ALIAS).is_some()
    {
        return true;
    }
    if super::cxf::worktree_native_binary().is_some_and(|path| path.is_file()) {
        return true;
    }
    eprintln!(
        "SKIP {suite}: {} and {} are not set and the worktree cxf-bridge binary is absent. \
         Provision with `cargo xtask build-cxf-bridge`, then \
         `export {}=\"$PWD/bridges/cxf/build/native/cxf-bridge\"` (unset it to skip).",
        super::cxf::ENV_CAMEL,
        super::cxf::ENV_ALIAS,
        super::cxf::ENV_CAMEL,
    );
    false
}
