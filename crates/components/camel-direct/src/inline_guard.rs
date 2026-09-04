//! Task-local guard stack for inline direct dispatch (change
//! `direct-inline-dispatch`). Inline dispatch runs the consumer pipeline on
//! the producer's task, so a re-entrant chain of `direct:` calls shares one
//! stack: cycle detection and the nesting-depth limit live here, while hop
//! counting stays on the dispatcher in camel-core.

use std::cell::RefCell;
use std::future::Future;

use camel_component_api::CamelError;

/// Error text prefix for a re-entrant `direct:` endpoint name.
pub(crate) const CYCLE_ERROR_PREFIX: &str = "direct cycle detected";

/// Error text for exceeding the inline dispatch nesting limit.
pub(crate) const DEPTH_ERROR: &str = "direct inline dispatch depth limit (64) exceeded";

/// Maximum number of simultaneously active inline dispatch hops.
const MAX_INLINE_DEPTH: usize = 64;

tokio::task_local! {
    static INLINE_STACK: RefCell<InlineStack>;
}

#[derive(Default)]
struct InlineStack {
    active: Vec<Box<str>>,
}

/// RAII marker for one hop on the inline dispatch stack.
///
/// Holds an owned copy of the endpoint name and pops it by name equality on
/// drop. It never holds the `RefCell` borrow across an await, so the
/// enclosing future stays `Send`; name-equality popping also keeps the stack
/// consistent under out-of-order drops.
pub(crate) struct InlineGuard {
    name: Box<str>,
}

impl Drop for InlineGuard {
    fn drop(&mut self) {
        // Never panic in Drop: an absent stack means the scope already ended.
        let _ = INLINE_STACK.try_with(|stack| {
            stack
                .borrow_mut()
                .active
                .retain(|active| *active != self.name);
        });
    }
}

/// Marks one inline dispatch hop for `name`.
///
/// Must be called inside a `with_inline_stack` scope; outside one, the
/// task-local is absent and this returns a processor error instead of
/// panicking. Rejects re-entering a name already on the stack (cycle) and
/// nesting deeper than [`MAX_INLINE_DEPTH`].
pub(crate) fn enter(name: &str) -> Result<InlineGuard, CamelError> {
    INLINE_STACK
        .try_with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.active.iter().any(|active| &**active == name) {
                return Err(CamelError::ProcessorError(format!(
                    "{CYCLE_ERROR_PREFIX} re-entering direct:{name}"
                )));
            }
            if stack.active.len() >= MAX_INLINE_DEPTH {
                return Err(CamelError::ProcessorError(DEPTH_ERROR.into()));
            }
            stack.active.push(name.into());
            Ok(InlineGuard { name: name.into() })
        })
        .unwrap_or_else(|_| {
            Err(CamelError::ProcessorError(
                "direct inline dispatch entered outside a with_inline_stack scope".into(),
            ))
        })
}

/// Runs `fut` with the inline dispatch stack available, creating one only at
/// the outermost call (mirrors the `CANCEL_TOKEN` try_with/scope pattern).
/// A nested chain — dispatcher → producer → dispatch — therefore shares one
/// stack, while sequential non-nested calls each start fresh; cycles and
/// depth only exist within a nested chain.
pub(crate) async fn with_inline_stack<R>(fut: impl Future<Output = R>) -> R {
    if INLINE_STACK.try_with(|_| ()).is_ok() {
        fut.await
    } else {
        INLINE_STACK
            .scope(RefCell::new(InlineStack::default()), fut)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guard_rejects_cycle_immediately() {
        with_inline_stack(async {
            let _guard = enter("a").unwrap();
            let res = enter("a");
            assert!(matches!(
                res,
                Err(CamelError::ProcessorError(ref msg))
                    if msg.starts_with(CYCLE_ERROR_PREFIX)
            ));
        })
        .await;
    }

    #[tokio::test]
    async fn guard_rejects_depth_65() {
        with_inline_stack(async {
            let guards: Vec<InlineGuard> =
                (0..64).map(|i| enter(&format!("n{i}")).unwrap()).collect();
            let res = enter("n65");
            assert!(matches!(
                res,
                Err(CamelError::ProcessorError(ref msg)) if msg == DEPTH_ERROR
            ));
            drop(guards);
        })
        .await;
    }

    #[tokio::test]
    async fn guard_allows_64_and_unwinds() {
        with_inline_stack(async {
            let guards: Vec<InlineGuard> =
                (0..64).map(|i| enter(&format!("n{i}")).unwrap()).collect();
            assert_eq!(guards.len(), 64);
            for guard in guards.into_iter().rev() {
                drop(guard);
            }
            assert!(enter("a").is_ok());
        })
        .await;
    }

    #[tokio::test]
    async fn nested_calls_share_stack() {
        with_inline_stack(async {
            let _outer = enter("outer").unwrap();
            with_inline_stack(async {
                let res = enter("outer");
                assert!(matches!(
                    res,
                    Err(CamelError::ProcessorError(ref msg))
                        if msg.starts_with(CYCLE_ERROR_PREFIX)
                ));
            })
            .await;
        })
        .await;
    }

    // Compile-time Send proof (Task 3.2's producer future must cross a spawn
    // boundary through `with_inline_stack`). NOTE: task-locals do NOT cross
    // `tokio::spawn`, so the sharing test above must stay same-task — Send is
    // pinned here at the type level instead.
    #[test]
    fn with_inline_stack_future_is_send() {
        fn assert_send<F: Future + Send>(_: &F) {}
        let fut = with_inline_stack(async {});
        assert_send(&fut);
    }
}
