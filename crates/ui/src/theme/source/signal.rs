//! Wakes the main-thread resolve pipeline from watcher threads, which hold
//! no gpui context (the pinned gpui's `AsyncApp` is not `Send`).

use gpui::{AsyncApp, Task};

pub(crate) struct ReapplySignal {
    sender: async_channel::Sender<()>,
    _task: Task<()>,
}

impl ReapplySignal {
    pub(crate) fn install(async_cx: &AsyncApp) -> Self {
        let executor = async_cx.foreground_executor().clone();
        let async_cx = async_cx.clone();

        let (sender, events) = async_channel::unbounded::<()>();

        let task = executor.spawn(async move {
            while events.recv().await.is_ok() {
                async_cx.update(crate::theme::reapply);
            }
        });

        Self {
            sender,
            _task: task,
        }
    }

    /// A clone for a watcher thread to poke the pipeline with.
    pub(crate) fn sender(&self) -> async_channel::Sender<()> {
        self.sender.clone()
    }

    /// Pokes the pipeline from a thread holding a [`Self::sender`] clone.
    pub(crate) fn poke(sender: &async_channel::Sender<()>) {
        if let Err(error) = sender.try_send(()) {
            log::warn!("theme: reapply signal failed: {error}");
        }
    }
}
