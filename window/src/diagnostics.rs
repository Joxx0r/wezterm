//! Counters used by the opt-in GUI diagnostics to tell whether the main
//! thread is still pumping OS messages or only draining the spawn queue.
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static SPAWN_TASKS_RUN: AtomicU64 = AtomicU64::new(0);
pub(crate) static MESSAGE_PUMP_ITERATIONS: AtomicU64 = AtomicU64::new(0);
pub(crate) static LONGEST_DRAIN_TASKS: AtomicU64 = AtomicU64::new(0);

pub fn spawn_tasks_run() -> u64 {
    SPAWN_TASKS_RUN.load(Ordering::Relaxed)
}

pub fn message_pump_iterations() -> u64 {
    MESSAGE_PUMP_ITERATIONS.load(Ordering::Relaxed)
}

/// Returns and resets the largest number of tasks run by a single
/// `SpawnQueue::run` call since the previous call to this function.
pub fn take_longest_drain_tasks() -> u64 {
    LONGEST_DRAIN_TASKS.swap(0, Ordering::Relaxed)
}

/// Returns the (high priority, low priority) spawn queue lengths.
pub fn spawn_queue_depth() -> (usize, usize) {
    crate::spawn::SPAWN_QUEUE.queue_depth()
}
