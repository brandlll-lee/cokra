// Agent Guards
// Spawn depth limiting and resource guards

use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::collections::HashSet;

use cokra_protocol::ThreadId;

use super::MAX_THREAD_SPAWN_DEPTH;

/// Guards for agent spawning limits
pub struct Guards {
    /// Set of active thread IDs
    threads_set: Mutex<HashSet<String>>,
    /// Total thread count
    total_count: AtomicUsize,
}

impl Guards {
    /// Create new guards
    pub fn new() -> Self {
        Self {
            threads_set: Mutex::new(HashSet::new()),
            total_count: AtomicUsize::new(0),
        }
    }

    /// Check if spawn depth exceeds limit
    pub fn exceeds_spawn_depth_limit(&self, depth: i32) -> bool {
        depth >= MAX_THREAD_SPAWN_DEPTH
    }

    /// Reserve a spawn slot
    pub fn reserve_spawn(&self) -> anyhow::Result<SpawnReservation> {
        let mut threads = self.threads_set.lock().unwrap();

        // Check if we can spawn more
        // (In production, this would check against config limits)

        Ok(SpawnReservation {
            state: Arc::new(self.clone()),
            active: true,
        })
    }

    /// Commit a spawn (internal)
    fn commit_spawn(&self, thread_id: ThreadId) {
        let mut threads = self.threads_set.lock().unwrap();
        threads.insert(thread_id.generate());
        self.total_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Release a spawn slot (when thread ends)
    fn release_spawn(&self, thread_id: &str) {
        let mut threads = self.threads_set.lock().unwrap();
        if threads.remove(thread_id) {
            self.total_count.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Get total thread count
    pub fn total_count(&self) -> usize {
        self.total_count.load(Ordering::SeqCst)
    }
}

impl Clone for Guards {
    fn clone(&self) -> Self {
        Self {
            threads_set: Mutex::new(self.threads_set.lock().unwrap().clone()),
            total_count: AtomicUsize::new(self.total_count.load(Ordering::SeqCst)),
        }
    }
}

impl Default for Guards {
    fn default() -> Self {
        Self::new()
    }
}

/// Reservation for a spawn slot
pub struct SpawnReservation {
    /// Guards reference
    state: Arc<Guards>,
    /// Whether reservation is active
    active: bool,
}

impl SpawnReservation {
    /// Commit the reservation with a thread ID
    pub fn commit(mut self, thread_id: ThreadId) {
        if self.active {
            self.state.commit_spawn(thread_id);
            self.active = false;
        }
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        // If not committed, nothing to clean up
        self.active = false;
    }
}

/// Calculate session depth from session source
pub fn session_depth(session_source: &super::control::SessionSource) -> i32 {
    session_source.depth()
}

/// Calculate next thread spawn depth
pub fn next_thread_spawn_depth(session_source: &super::control::SessionSource) -> i32 {
    session_source.depth() + 1
}
