//! Address-keyed wait queues. Blink's Machine/System design is the reference
//! for ownership; its vendored SysFutex supports WAIT/WAKE only. Requeue and
//! bitset handling here implement the Linux ABI rather than pretending that
//! broadcasting a condition variable moves its waiters.
//!
//! All compare/enqueue, wake, requeue and removal operations hold one mutex.
//! A woken waiter remains marked until it consumes the wake, so repeated wakes
//! cannot count it twice and a wake before parking cannot be lost.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Default)]
struct Table {
    next_id: u64,
    waiters: BTreeMap<u64, Entry>,
}
struct Entry {
    address: u64,
    bitset: u32,
    woken: bool,
    cv: Arc<Condvar>,
}
#[derive(Default)]
pub(super) struct FutexManager {
    table: Mutex<Table>,
}

pub(super) struct FutexWait {
    manager: Arc<FutexManager>,
    id: u64,
    cv: Arc<Condvar>,
}

impl FutexManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// `read` runs while holding the queue lock. It must read the current
    /// guest word, not a value cached before acquiring the lock. The caller
    /// must synchronize guest memory and release it before parking.
    pub fn prepare_wait(
        self: &Arc<Self>,
        address: u64,
        expected: u32,
        bitset: u32,
        read: impl FnOnce() -> Result<u32, i32>,
    ) -> Result<FutexWait, i32> {
        if address & 3 != 0 || bitset == 0 {
            return Err(-22);
        }
        let mut table = self.table.lock().unwrap();
        if read()? != expected {
            return Err(-11);
        }
        let id = table.next_id;
        table.next_id = id.checked_add(1).ok_or(-11)?;
        let cv = Arc::new(Condvar::new());
        table.waiters.insert(
            id,
            Entry {
                address,
                bitset,
                woken: false,
                cv: cv.clone(),
            },
        );
        Ok(FutexWait {
            manager: self.clone(),
            id,
            cv,
        })
    }

    pub fn wake(&self, address: u64, count: u32, bitset: u32) -> i32 {
        if address & 3 != 0 || bitset == 0 || count > i32::MAX as u32 {
            return -22;
        }
        let mut table = self.table.lock().unwrap();
        let mut woken = 0;
        for entry in table.waiters.values_mut() {
            if woken == count {
                break;
            }
            if entry.address == address && !entry.woken && entry.bitset & bitset != 0 {
                entry.woken = true;
                entry.cv.notify_one();
                woken += 1;
            }
        }
        woken as i32
    }

    /// Wake up to `wake_count`, move up to `move_count` others, and return
    /// the total affected. The optional comparison is atomic with the move.
    pub fn requeue(
        &self,
        from: u64,
        to: u64,
        wake_count: u32,
        move_count: u32,
        compare: impl FnOnce() -> Result<(), i32>,
    ) -> i32 {
        if from & 3 != 0
            || to & 3 != 0
            || wake_count > i32::MAX as u32
            || move_count > i32::MAX as u32
        {
            return -22;
        }
        let mut table = self.table.lock().unwrap();
        if let Err(error) = compare() {
            return error;
        }
        let mut woken = 0u32;
        let mut moved = 0u32;
        for entry in table.waiters.values_mut() {
            if entry.address != from || entry.woken {
                continue;
            }
            if woken < wake_count {
                entry.woken = true;
                entry.cv.notify_one();
                woken += 1;
            } else if moved < move_count {
                entry.address = to;
                moved += 1;
            }
        }
        (woken + moved) as i32
    }
}

impl FutexWait {
    /// Spurious host wakeups do not complete the guest wait. Deadline and
    /// wake are checked under the same lock, including the timeout race.
    pub fn park(self, deadline: Instant) -> i32 {
        let mut table = self.manager.table.lock().unwrap();
        loop {
            if table.waiters[&self.id].woken {
                return 0;
            }
            let now = Instant::now();
            if now >= deadline {
                return -110;
            }
            let (guard, _) = self.cv.wait_timeout(table, deadline - now).unwrap();
            table = guard;
        }
    }
}
impl Drop for FutexWait {
    fn drop(&mut self) {
        // Runs after park's guard has been dropped; also removes a registered
        // waiter if thread creation fails or a pending syscall is cancelled.
        self.manager.table.lock().unwrap().waiters.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn manager() -> Arc<FutexManager> {
        Arc::new(FutexManager::new())
    }
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }

    #[test]
    fn compare_enqueue_and_wake_before_park_are_atomic() {
        let manager = manager();
        let word = AtomicU32::new(1);
        assert!(matches!(
            manager.prepare_wait(4, 0, !0, || Ok(word.load(Ordering::SeqCst))),
            Err(-11)
        ));
        let wait = manager
            .prepare_wait(4, 1, !0, || Ok(word.load(Ordering::SeqCst)))
            .unwrap();
        word.store(0, Ordering::SeqCst);
        assert_eq!(manager.wake(4, 1, !0), 1);
        assert_eq!(manager.wake(4, 1, !0), 0);
        assert_eq!(wait.park(deadline()), 0);
        assert!(manager.table.lock().unwrap().waiters.is_empty());
    }

    #[test]
    fn wake_limit_and_bitsets_select_only_requested_waiters() {
        let manager = manager();
        let first = manager.prepare_wait(4, 1, 1, || Ok(1)).unwrap();
        let second = manager.prepare_wait(4, 1, 2, || Ok(1)).unwrap();
        let third = manager.prepare_wait(4, 1, 2, || Ok(1)).unwrap();
        assert_eq!(manager.wake(4, 1, 2), 1);
        assert_eq!(second.park(deadline()), 0);
        assert_eq!(manager.wake(4, 2, 2), 1);
        assert_eq!(third.park(deadline()), 0);
        assert_eq!(manager.wake(4, 2, 2), 0);
        assert_eq!(manager.wake(4, 1, 1), 1);
        assert_eq!(first.park(deadline()), 0);
    }

    #[test]
    fn requeue_moves_sleepers_and_obeys_both_counts() {
        let manager = manager();
        let waiters: Vec<_> = (0..4)
            .map(|_| manager.prepare_wait(4, 0, !0, || Ok(0)).unwrap())
            .collect();
        assert_eq!(manager.requeue(4, 8, 1, 2, || Ok(())), 3);
        assert_eq!(manager.wake(8, 8, !0), 2);
        assert_eq!(manager.wake(4, 8, !0), 1);
        for wait in waiters {
            assert_eq!(wait.park(deadline()), 0);
        }
    }

    #[test]
    fn compare_requeue_failure_does_not_wake_or_move_anyone() {
        let manager = manager();
        let wait = manager.prepare_wait(4, 0, !0, || Ok(0)).unwrap();
        assert_eq!(manager.requeue(4, 8, 1, 1, || Err(-11)), -11);
        assert_eq!(manager.wake(8, 1, !0), 0);
        assert_eq!(manager.wake(4, 1, !0), 1);
        assert_eq!(wait.park(deadline()), 0);
    }

    #[test]
    fn timeout_and_cancellation_remove_requeued_waiters() {
        let manager = manager();
        let wait = manager.prepare_wait(4, 0, !0, || Ok(0)).unwrap();
        assert_eq!(manager.requeue(4, 8, 0, 1, || Ok(())), 1);
        assert_eq!(wait.park(Instant::now()), -110);
        assert_eq!(manager.wake(8, 1, !0), 0);
        let wait = manager.prepare_wait(4, 0, !0, || Ok(0)).unwrap();
        drop(wait);
        assert!(manager.table.lock().unwrap().waiters.is_empty());
    }

    #[test]
    fn invalid_addresses_masks_and_counts_are_rejected() {
        let manager = manager();
        assert!(matches!(manager.prepare_wait(3, 0, !0, || Ok(0)), Err(-22)));
        assert!(matches!(manager.prepare_wait(4, 0, 0, || Ok(0)), Err(-22)));
        assert!(matches!(
            manager.prepare_wait(4, 0, !0, || Err(-14)),
            Err(-14)
        ));
        assert_eq!(manager.wake(4, u32::MAX, !0), -22);
        assert_eq!(manager.wake(4, 1, 0), -22);
        assert_eq!(manager.requeue(4, 7, 1, 1, || Ok(())), -22);
    }

    #[test]
    fn host_threads_can_sleep_requeue_and_wake() {
        let manager = manager();
        let waits: Vec<_> = (0..8)
            .map(|_| manager.prepare_wait(4, 0, !0, || Ok(0)).unwrap())
            .collect();
        let threads: Vec<_> = waits
            .into_iter()
            .map(|wait| std::thread::spawn(move || wait.park(deadline())))
            .collect();
        assert_eq!(manager.requeue(4, 8, 2, 6, || Ok(())), 8);
        assert_eq!(manager.wake(8, 6, !0), 6);
        for thread in threads {
            assert_eq!(thread.join().unwrap(), 0);
        }
        assert!(manager.table.lock().unwrap().waiters.is_empty());
    }
}
