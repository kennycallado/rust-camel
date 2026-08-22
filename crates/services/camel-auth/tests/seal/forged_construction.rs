// Attempts to construct an `AuthenticatedPrincipal` from outside camel-auth.
// All fields are private and there is no public constructor, so this must fail
// to compile with E0451 (fields ... are private).
#![allow(unreachable_code)]

fn main() {
    let _ = camel_auth::AuthenticatedPrincipal {
        principal: todo!(),
        provider_id: todo!(),
    };
}
