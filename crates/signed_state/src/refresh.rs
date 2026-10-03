#[derive(Debug, Default)]
pub struct RefreshGate {
    running: bool,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRequest {
    Schedule,
    Fold,
}

impl RefreshGate {
    pub fn running(&self) -> bool {
        self.running
    }

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

    pub fn finish(&mut self) -> bool {
        self.running = false;
        std::mem::take(&mut self.dirty)
    }

    // Pending follow-up requests survive an abandoned run.
    pub fn abort(&mut self) {
        self.running = false;
    }
}
