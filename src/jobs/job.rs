use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{Mutex, Notify};
use crate::types::PromptRequest;

pub struct Job {
    pub cancel: Notify,
    pub stopped: AtomicBool,
    pub queue: Mutex<VecDeque<PromptRequest>>,
}

impl Job {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: Notify::new(),
            stopped: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
        })
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    pub fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
