//! RAII guard that cancels a [`CancellationToken`] on drop unless
//! [`complete`](InvokeCancelGuard::complete) was called first.

use tokio_util::sync::CancellationToken;

pub(crate) struct InvokeCancelGuard {
    token: CancellationToken,
    completed: bool,
}

impl InvokeCancelGuard {
    pub(crate) fn new(token: CancellationToken) -> Self {
        Self {
            token,
            completed: false,
        }
    }

    pub(crate) fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for InvokeCancelGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancels_on_drop_if_not_completed() {
        let token = CancellationToken::new();
        let _guard = InvokeCancelGuard::new(token.clone());
        drop(_guard);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn does_not_cancel_when_completed() {
        let token = CancellationToken::new();
        let mut guard = InvokeCancelGuard::new(token.clone());
        guard.complete();
        drop(guard);
        assert!(!token.is_cancelled());
    }
}
