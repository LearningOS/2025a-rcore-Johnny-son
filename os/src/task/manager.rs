//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A large constant used in stride scheduling.
///
/// Choose a value large enough to reduce division error, but small enough to avoid overflow.
pub const BIG_STRIDE: u64 = 10_000;

/// A simple stride scheduler (linear scan).
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty() {
            return None;
        }

        // Find the runnable task with minimal stride.
        let mut min_idx = 0usize;
        let mut min_stride = {
            let inner = self.ready_queue[0].inner_exclusive_access();
            inner.stride
        };
        for (i, t) in self.ready_queue.iter().enumerate().skip(1) {
            let inner = t.inner_exclusive_access();
            let s = inner.stride;
            if s < min_stride {
                min_stride = s;
                min_idx = i;
            }
        }

        let task = self.ready_queue.remove(min_idx).unwrap();
        // After selecting it to run, add its stride by pass.
        {
            let mut inner = task.inner_exclusive_access();
            inner.stride = inner.stride.wrapping_add(inner.pass);
        }
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
