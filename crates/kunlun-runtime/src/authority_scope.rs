use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// Revocation is permanent and wakes every operation using this authority.
#[derive(Debug)]
pub(crate) struct AuthorityScope {
    active: AtomicBool,
    notify: Notify,
}

impl AuthorityScope {
    pub(crate) fn new() -> Self {
        Self {
            active: AtomicBool::new(true),
            notify: Notify::new(),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn revoke(&self) {
        self.active.store(false, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register before inspecting the flag, including on the first poll.
            notified.as_mut().enable();
            if !self.is_active() {
                return;
            }
            notified.await;
        }
    }
}
