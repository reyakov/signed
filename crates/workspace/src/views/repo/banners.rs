use std::collections::HashSet;
use std::path::PathBuf;

use signed_state::{CheckoutStatus, CheckoutSyncStatus};

#[derive(Default)]
pub(super) struct Banners {
    dismissed: HashSet<(PathBuf, String)>,
    sync_dismissed: HashSet<PathBuf>,
    ready_requested: bool,
    ready_head: Option<String>,
    ready_statuses: Vec<CheckoutStatus>,
    push_statuses: Vec<CheckoutStatus>,
}

impl Banners {
    pub(super) fn dismissal(&self, status: &CheckoutStatus) -> bool {
        self.dismissed
            .contains(&(status.path.clone(), status.branch.clone()))
    }

    pub(super) fn dismiss(&mut self, status: &CheckoutStatus) {
        self.dismissed
            .insert((status.path.clone(), status.branch.clone()));
    }

    pub(super) fn sync_dismissal(&self, checkout: &CheckoutSyncStatus) -> bool {
        self.sync_dismissed.contains(&checkout.path)
    }

    pub(super) fn sync_dismiss(&mut self, checkout: &CheckoutSyncStatus) {
        self.sync_dismissed.insert(checkout.path.clone());
    }

    pub(super) fn ready_requested_at(&self) -> (bool, &Option<String>) {
        (self.ready_requested, &self.ready_head)
    }

    pub(super) fn mark_ready_requested(&mut self, head: Option<String>) {
        self.ready_requested = true;
        self.ready_head = head;
    }

    pub(super) fn set_statuses(
        &mut self,
        ready: Vec<CheckoutStatus>,
        push: Vec<CheckoutStatus>,
    ) -> bool {
        let changed = ready != self.ready_statuses || push != self.push_statuses;
        self.ready_statuses = ready;
        self.push_statuses = push;
        changed
    }
}
