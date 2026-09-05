//! Process-global staged-listener parking for `wasm:` source routes.
//!
//! A test helper binds the real socket and parks it here under its exact
//! `(host-string, port)` key; the source consumer consults the map at its
//! single bind site — after the operator/guest bind agreement and the
//! ADR-0061 exposure gate — and consumes the parked socket one-shot
//! instead of binding fresh. Host strings compare as-is: `0.0.0.0` and
//! `127.0.0.1` are different keys, and a same-port different-host take is
//! a deterministic conflict, never a silent fallback.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::LazyLock;
use std::sync::Mutex;

use camel_api::CamelError;
use tokio::net::TcpListener;

/// Process-global staged listeners, keyed by exact `(host-string, port)`.
///
/// std [`Mutex`], never held across an `.await` (mirroring the
/// camel-component-ws registry): every operation locks, mutates, and
/// releases synchronously. `LazyLock` only because `HashMap::new` is not
/// callable in a `static` initializer; access still goes through the
/// inner `Mutex` on every operation.
static STAGED: LazyLock<Mutex<StagedMap>> = LazyLock::new(|| Mutex::new(StagedMap::new()));

/// Staged sockets keyed by their exact `(host-string, port)` bind address.
type StagedMap = HashMap<(String, u16), TcpListener>;

/// Park a pre-bound listener under its exact `(host, port)` key.
///
/// The key comes from the listener's `local_addr()`. Staging over an
/// occupied key fails with `listener already staged for {h}:{p}` and
/// keeps the first listener in the slot — the socket is consumed once,
/// by the route whose config names it.
pub fn stage_listener(listener: TcpListener) -> Result<(), CamelError> {
    let addr = listener.local_addr().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("staged listener local_addr: {e}"))
    })?;
    let key = (addr.ip().to_string(), addr.port());
    let mut staged = staged_map()?;
    if staged.contains_key(&key) {
        return Err(CamelError::EndpointCreationFailed(format!(
            "listener already staged for {}:{}",
            key.0, key.1
        )));
    }
    staged.insert(key, listener);
    Ok(())
}

/// Take the staged listener for `bind_addr`, if any.
///
/// Exact-key hit → the listener, removed from the map (one-shot).
/// Same port staged under a different host string → deterministic
/// conflict error naming both hosts; slot and socket untouched.
/// Miss → `Ok(None)`; the caller binds fresh.
pub(crate) fn take(bind_addr: SocketAddr) -> Result<Option<TcpListener>, CamelError> {
    let mut staged = staged_map()?;
    let key = (bind_addr.ip().to_string(), bind_addr.port());
    if let Some(listener) = staged.remove(&key) {
        return Ok(Some(listener));
    }
    // No exact hit: any same-port entry under a different host string is a
    // conflict — the requested address is exactly the kind of mis-route the
    // staging map exists to catch. Slot and socket stay untouched.
    if let Some((staged_host, _)) = staged
        .keys()
        .find(|k| k.1 == bind_addr.port() && k.0 != key.0)
    {
        let staged_host = staged_host.clone();
        return Err(CamelError::EndpointCreationFailed(format!(
            "staged listener conflict on port {}: staged under host {staged_host}, requested {}",
            bind_addr.port(),
            bind_addr.ip()
        )));
    }
    Ok(None)
}

/// Lock the staged map, mapping a poisoned lock to
/// `EndpointCreationFailed` (all staged-listener failures surface as
/// endpoint-creation errors, never as panics).
fn staged_map() -> Result<std::sync::MutexGuard<'static, StagedMap>, CamelError> {
    STAGED.lock().map_err(|poisoned| {
        CamelError::EndpointCreationFailed(format!("staged listener map poisoned: {poisoned}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bind a fresh std listener on a loopback ephemeral port.
    fn loopback_std_listener() -> std::net::TcpListener {
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0 must succeed")
    }

    /// Wrap a std listener into a tokio listener (non-blocking required).
    fn tokio_from_std(std_listener: std::net::TcpListener) -> TcpListener {
        std_listener
            .set_nonblocking(true)
            .expect("set_nonblocking must succeed");
        TcpListener::from_std(std_listener).expect("from_std must succeed")
    }

    /// Staging then taking on the exact key round-trips the socket; the
    /// consumed key is gone (second take is `Ok(None)`).
    ///
    /// Async test only because `TcpListener::from_std` requires a reactor;
    /// `stage_listener`/`take` themselves are synchronous.
    #[tokio::test]
    async fn stage_take_exact_key_roundtrip() {
        let std_listener = loopback_std_listener();
        let addr = std_listener.local_addr().expect("local_addr");

        stage_listener(tokio_from_std(std_listener)).expect("staging a fresh key must succeed");

        let taken = take(addr)
            .expect("exact-key take must not error")
            .expect("exact-key take must return the staged listener");
        assert_eq!(
            taken.local_addr().expect("taken local_addr"),
            addr,
            "the taken socket must be the staged one"
        );

        let second = take(addr).expect("take on a consumed key must not error");
        assert!(
            second.is_none(),
            "second take on a consumed key must be Ok(None)"
        );
    }

    /// Duplicate staging of an occupied key fails naming the key, and the
    /// FIRST staged listener stays in the slot.
    ///
    /// A staged listener is moved into the map, so the same socket is
    /// re-referenced through a `try_clone` clone (the camel-component-ws
    /// CLONE-FIXTURE pattern, lib.rs:3027-3069).
    #[tokio::test]
    async fn duplicate_staging_rejected_first_stays() {
        let std_listener = loopback_std_listener();
        let addr = std_listener.local_addr().expect("local_addr");
        let clone = std_listener.try_clone().expect("try_clone");

        stage_listener(tokio_from_std(std_listener)).expect("first staging must succeed");

        let err = stage_listener(tokio_from_std(clone))
            .expect_err("duplicate staging of an occupied key must fail");
        assert!(
            err.to_string().contains(&format!(
                "listener already staged for 127.0.0.1:{}",
                addr.port()
            )),
            "error must name the occupied key: {err}"
        );

        let taken = take(addr)
            .expect("take after a rejected duplicate must not error")
            .expect("the FIRST staged listener must still own the slot");
        assert_eq!(
            taken.local_addr().expect("taken local_addr"),
            addr,
            "the surviving socket must be the first staged one"
        );
    }

    /// Taking with a same-port different-host address is a deterministic
    /// conflict, and the conflict leaves the slot untouched.
    #[tokio::test]
    async fn wrong_host_take_conflicts_and_preserves() {
        // Single stage — no clone needed: nothing else re-references the
        // socket while it sits in the slot.
        let std_listener = std::net::TcpListener::bind("0.0.0.0:0").expect("bind 0.0.0.0:0");
        let addr = std_listener.local_addr().expect("local_addr");

        stage_listener(tokio_from_std(std_listener)).expect("staging must succeed");

        let requested: SocketAddr = format!("127.0.0.1:{}", addr.port()).parse().expect("parse");
        let err = take(requested).expect_err("same-port different-host take must conflict");
        assert!(
            err.to_string().contains(&format!(
                "staged listener conflict on port {}: staged under host 0.0.0.0, requested 127.0.0.1",
                addr.port()
            )),
            "error must state the conflict exactly: {err}"
        );

        let taken = take(addr)
            .expect("exact-key take after a conflict must not error")
            .expect("the conflict must leave the slot untouched");
        assert_eq!(
            taken.local_addr().expect("taken local_addr"),
            addr,
            "the staged socket must survive the conflict"
        );
    }
}
