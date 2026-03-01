use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use cokra_protocol::ThreadId;

/// Max allowed depth for spawned threads in phase 1 parity.
pub const MAX_THREAD_SPAWN_DEPTH: usize = 1;

/// Returns true when the requested depth is above the supported limit.
pub fn exceeds_thread_spawn_depth_limit(depth: usize) -> bool {
  depth > MAX_THREAD_SPAWN_DEPTH
}

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
  #[error("agent thread limit reached (max_threads={max_threads})")]
  AgentLimitReached { max_threads: usize },
}

/// Per-session guard state shared across all agent controls.
#[derive(Default)]
pub struct Guards {
  threads_set: Mutex<HashSet<ThreadId>>,
  total_count: AtomicUsize,
}

impl Guards {
  pub fn reserve_spawn_slot(
    self: &Arc<Self>,
    max_threads: Option<usize>,
  ) -> Result<SpawnReservation, GuardError> {
    if let Some(max_threads) = max_threads {
      if !self.try_increment_spawned(max_threads) {
        return Err(GuardError::AgentLimitReached { max_threads });
      }
    } else {
      self.total_count.fetch_add(1, Ordering::AcqRel);
    }

    Ok(SpawnReservation {
      state: Arc::clone(self),
      active: true,
    })
  }

  pub fn release_spawned_thread(&self, thread_id: ThreadId) {
    let removed = {
      let mut threads = self
        .threads_set
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      threads.remove(&thread_id)
    };
    if removed {
      self.total_count.fetch_sub(1, Ordering::AcqRel);
    }
  }

  pub fn spawned_count(&self) -> usize {
    self.total_count.load(Ordering::Acquire)
  }

  fn register_spawned_thread(&self, thread_id: ThreadId) {
    let mut threads = self
      .threads_set
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    threads.insert(thread_id);
  }

  fn try_increment_spawned(&self, max_threads: usize) -> bool {
    let mut current = self.total_count.load(Ordering::Acquire);
    loop {
      if current >= max_threads {
        return false;
      }
      match self.total_count.compare_exchange_weak(
        current,
        current + 1,
        Ordering::AcqRel,
        Ordering::Acquire,
      ) {
        Ok(_) => return true,
        Err(updated) => current = updated,
      }
    }
  }
}

pub struct SpawnReservation {
  state: Arc<Guards>,
  active: bool,
}

impl SpawnReservation {
  pub fn commit(mut self, thread_id: ThreadId) {
    self.state.register_spawned_thread(thread_id);
    self.active = false;
  }
}

impl Drop for SpawnReservation {
  fn drop(&mut self) {
    if self.active {
      self.state.total_count.fetch_sub(1, Ordering::AcqRel);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reservation_drop_releases_slot() {
    let guards = Arc::new(Guards::default());
    let reservation = guards.reserve_spawn_slot(Some(1)).expect("reserve slot");
    drop(reservation);

    let reservation = guards.reserve_spawn_slot(Some(1)).expect("slot released");
    drop(reservation);
  }

  #[test]
  fn commit_holds_slot_until_release() {
    let guards = Arc::new(Guards::default());
    let reservation = guards.reserve_spawn_slot(Some(1)).expect("reserve slot");
    let thread_id = ThreadId::new();
    reservation.commit(thread_id.clone());

    assert!(matches!(
      guards.reserve_spawn_slot(Some(1)),
      Err(GuardError::AgentLimitReached { max_threads: 1 })
    ));

    guards.release_spawned_thread(thread_id);
    let reservation = guards
      .reserve_spawn_slot(Some(1))
      .expect("slot released after thread removal");
    drop(reservation);
  }

  #[test]
  fn depth_limit_is_enforced() {
    assert!(!exceeds_thread_spawn_depth_limit(1));
    assert!(exceeds_thread_spawn_depth_limit(2));
  }
}
