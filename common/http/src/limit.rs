use std::mem::take;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use metrics::Metrics;
use serde::{Serialize, Serializer};
use std::cell::Cell;
use std::thread::sleep;
use std::time::Duration;

pub struct RateLimiter {
    pub slots: Arc<AtomicUsize>,
    pub max: usize,
}

impl RateLimiter {
    pub fn new(max: usize) -> Self {
        RateLimiter {
            slots: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    pub fn get_slot<T>(&self, item: T) -> Slot<T> {
        let backoff = Backoff::new();
        loop {
            let current = self.slots.load(Ordering::SeqCst);

            if current >= self.max {
                Metrics::http().increment_limit_hits();
                backoff.snooze();
                continue;
            }

            match self.slots.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(res) if current == res => {
                    return Slot {
                        inner: Arc::new(InnerSlot {
                            inner: item,
                            slot: current + 1,
                            slots: self.slots.clone(),
                        }),
                    }
                }
                _ => {
                    Metrics::http().increment_limit_hits();
                    backoff.snooze();
                    continue;
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Slot<T> {
    inner: Arc<InnerSlot<T>>,
}

impl<T> Slot<T>
where
    T: Clone + Default,
{
    pub fn into_inner(self) -> T {
        match Arc::try_unwrap(self.inner) {
            Ok(mut inner) => take(&mut inner.inner),
            Err(new_inner) => new_inner.inner.clone(),
        }
    }
}

impl<T> Deref for Slot<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner.inner
    }
}

impl<T: Serialize> Serialize for Slot<T> {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        self.inner.inner.serialize(serializer)
    }
}

impl<T> AsRef<T> for Slot<T> {
    fn as_ref(&self) -> &T {
        &self.inner.inner
    }
}

#[derive(Debug)]
struct InnerSlot<T> {
    inner: T,
    slot: usize,
    slots: Arc<AtomicUsize>,
}

impl<T> Drop for InnerSlot<T> {
    fn drop(&mut self) {
        self.slots.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Backoff {
    step: Cell<u32>,
    base: u64,
    multipler: u64,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            step: Cell::new(0),
            base: 2,
            multipler: 10,
        }
    }

    pub fn snooze(&self) {
        let step = self.step.get();
        sleep(Duration::from_millis(self.base.pow(step) * self.multipler));
        self.step.set(step + 1);
    }
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::thread::spawn;
    use std::time::Duration;

    use rand::Rng;

    use super::*;

    #[test]
    fn simple_max_slots() {
        let limiter = RateLimiter::new(3);
        let slot1 = limiter.get_slot(());
        let slot2 = limiter.get_slot(());
        let slot3 = limiter.get_slot(());
        assert_eq!(slot1.inner.slot, 1);
        assert_eq!(slot2.inner.slot, 2);
        assert_eq!(slot3.inner.slot, 3);
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 3);
        drop(slot1);
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 2);
        drop(slot2);
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 1);
        drop(slot3);
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn single_thread_loop() {
        let limiter = RateLimiter::new(3);
        for _ in 0..1_000_000 {
            assert_ne!(limiter.get_slot(()).inner.slot, 0);
        }
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn multi_thread_loop() {
        let mut joins = Vec::new();
        let limiter = Arc::new(RateLimiter::new(3));
        for _ in 0..num_cpus::get().max(1) {
            let tmp = limiter.clone();
            let join_handle = spawn(move || {
                for _ in 0..3_000_000 {
                    let slot = tmp.get_slot(());
                    assert_ne!(slot.inner.slot, 0);
                    assert!(slot.inner.slot <= 3);
                }
            });
            joins.push(join_handle);
        }
        for join_handle in joins {
            join_handle.join().unwrap();
        }
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 0);
    }

    #[test]
    #[cfg(unix)]
    fn multi_thread_random() {
        let mut joins = Vec::new();
        let limiter = Arc::new(RateLimiter::new(2));
        for _ in 0..num_cpus::get().max(1) {
            let tmp = limiter.clone();
            let join_handle = spawn(move || {
                let mut rng = rand::thread_rng();
                for _ in 0..10000 {
                    let slot = tmp.get_slot(());
                    assert_ne!(slot.inner.slot, 0);
                    assert!(slot.inner.slot <= 2);
                    sleep(Duration::from_micros(rng.gen_range(1..100)))
                }
            });
            joins.push(join_handle);
        }
        for join_handle in joins {
            join_handle.join().unwrap();
        }
        assert_eq!(limiter.slots.load(Ordering::SeqCst), 0);
    }
}
