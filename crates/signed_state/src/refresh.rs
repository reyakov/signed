/// Refresh coalescing shared by the event stores.
///
/// [`crate::RepoStore`], [`crate::RepoListStore`] and [`crate::CheckoutsStore`]
/// re-query their inputs on a debounce timer with the same policy:
/// a request arriving while a run is in flight is folded into a follow-up run,
/// a request arriving while the debounce timer is pending is dropped by it.
#[derive(Debug, Default)]
pub struct RefreshGate {
    /// A run is in flight.
    running: bool,
    /// A request arrived while a run was in flight.
    dirty: bool,
    /// The debounce timer is pending.
    debouncing: bool,
}

/// What a refresh request decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRequest {
    /// No run or timer covers the request, start the debounce timer.
    Schedule,
    /// A run or pending timer already covers the request.
    Fold,
}

impl RefreshGate {
    /// Whether a run is in flight.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Whether the debounce timer is pending.
    pub fn debouncing(&self) -> bool {
        self.debouncing
    }

    /// A new refresh request arrived.
    ///
    /// Folded into a follow-up run while one is in flight, dropped while the
    /// debounce timer is pending, otherwise starts the timer.
    pub fn request(&mut self) -> RefreshRequest {
        if self.running {
            self.dirty = true;
            RefreshRequest::Fold
        } else if self.debouncing {
            RefreshRequest::Fold
        } else {
            self.debouncing = true;
            RefreshRequest::Schedule
        }
    }

    /// The debounce timer fired and the run starts now.
    pub fn begin(&mut self) {
        self.debouncing = false;
        self.running = true;
    }

    /// The run ended. Whether a request arrived while it ran.
    pub fn finish(&mut self) -> bool {
        self.running = false;
        std::mem::take(&mut self.dirty)
    }

    /// The run was abandoned, e.g. on error. Pending follow-up requests survive.
    pub fn abort(&mut self) {
        self.running = false;
    }
}
