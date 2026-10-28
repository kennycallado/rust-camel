//! Pure `trim` shadow for the Rhai standard string surface (change
//! `mission359-languagep1wave`, task 3.2).
//!
//! Rhai's `StandardPackage` registers `trim(&mut ImmutableString)` as an
//! in-place method that returns unit `()` (see `rhai-1.26.1`
//! `src/packages/string_more.rs`). The mutating form makes a blank check
//! wrong: `let x = "   "; x.trim() != ""` is always true because unit never
//! equals a string, and the subject is silently rewritten in place.
//!
//! Registering `trim(ImmutableString) -> String` on the base engine replaces
//! that registration: rhai keys native functions by name plus the (mapped)
//! parameter `TypeId`s, and a `&mut ImmutableString` method and an owned
//! `ImmutableString` function collide on the same key. The pure registration
//! must therefore use `ImmutableString` (not `&str`/`String`, which would map
//! to a different key and leave the mutating method in place).
//!
//! Only `trim` is shadowed. No `_inplace` alias is registered. The other
//! in-place methods (`make_upper`, `make_lower`, `clear`, `truncate`,
//! `remove`, `crop`, `replace`, `pad`) keep their standard behavior and are
//! documented with their pure alternatives in `docs/src/languages/rhai.md`.

use rhai::{FuncRegistration, ImmutableString, Module};

/// Register the pure `trim` shadow into a module. The module must be installed
/// after `StandardPackage` so the shadow takes precedence over the mutating
/// registration.
pub(crate) fn shadow_into(module: &mut Module) {
    FuncRegistration::new("trim")
        .in_global_namespace()
        .set_into_module(module, |s: ImmutableString| -> String {
            s.trim().to_string()
        });
}
