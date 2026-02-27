// Utils - Async Priority
// Async task priority management

use tokio::sync::mpsc;

/// Priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
}

/// Priority sender
pub type PrioritySender<T> = mpsc::Sender<(Priority, T)>;

/// Priority receiver
pub type PriorityReceiver<T> = mpsc::Receiver<(Priority, T)>;

/// Create a new priority channel
pub fn priority_channel<T>(capacity: usize) -> (PrioritySender<T>, PriorityReceiver<T>) {
    mpsc::channel(capacity)
}
