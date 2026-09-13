/// Refresh coalescing shared by the event stores.
#[derive(Debug, Default)]
pub struct RefreshGate {
    running: bool,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRequest {
    /// No run covers the request, start one now.
    Schedule,
    /// A run is in flight and covers the request, fold it into a follow-up.
    Fold,
}

impl RefreshGate {
    pub fn running(&self) -> bool {
        self.running
    }

    /// A new refresh request arrived.
    ///
    /// Folded into a follow-up run while one is in flight, otherwise the
    /// caller starts the run itself.
    pub fn request(&mut self) -> RefreshRequest {
        if self.running {
            self.dirty = true;
            RefreshRequest::Fold
        } else {
            RefreshRequest::Schedule
        }
    }

    pub fn begin(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_while_running_folds_into_a_follow_up() {
        let mut gate = RefreshGate::default();
        gate.begin();

        assert_eq!(gate.request(), RefreshRequest::Fold);
        assert!(gate.finish());
    }

    #[test]
    fn a_request_without_a_run_schedules() {
        let mut gate = RefreshGate::default();

        assert_eq!(gate.request(), RefreshRequest::Schedule);
        assert!(!gate.running());
    }

    #[test]
    fn a_request_after_a_run_schedules_again() {
        let mut gate = RefreshGate::default();
        gate.begin();
        assert_eq!(gate.request(), RefreshRequest::Fold);
        assert!(gate.finish());

        assert_eq!(gate.request(), RefreshRequest::Schedule);
    }

    #[test]
    fn abort_keeps_the_pending_request() {
        let mut gate = RefreshGate::default();
        gate.begin();
        assert_eq!(gate.request(), RefreshRequest::Fold);

        gate.abort();
        assert!(!gate.running());

        gate.begin();
        assert!(gate.finish());
    }
}
