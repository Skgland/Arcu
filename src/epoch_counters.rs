use core::sync::atomic::{AtomicU8, Ordering};

#[cfg(feature = "std")]
mod std {
    use super::EpochCounter;
    use std::sync::RwLock;

    use alloc::sync::{Arc, Weak};


    // the epoch counters of all threads that have ever accessed an Rcu
    // threads that have finished will have a dangling Weak reference and can be cleaned up
    // having this be shared between all Rcu's is a tradeof:
    // - writes will be slower as more epoch counters need to be waited for
    // - reads should be faster as a thread only needs to register itself once on the first read
    pub(crate) static EPOCH_COUNTERS: RwLock<Vec<Weak<EpochCounter>>> = RwLock::new(Vec::new());

    thread_local! {
        // odd value means the current thread is about to access the active_epoch of an Rcu
        // - threads observing this while leaving the write critical section will need to wait for this to change to a different (odd or even) value
        // a thread has a single epoch counter for all Rcu it accesses, as a thread can only access one Rcu at a time
        pub(crate) static THREAD_EPOCH_COUNTER: std::cell::OnceCell<Arc<EpochCounter>> = const { std::cell::OnceCell::new() };
    }
}

#[cfg(feature = "std")]
pub(crate) use self::std::*;

#[repr(transparent)]
pub struct EpochCounter (
    core::sync::atomic::AtomicU8
);

impl EpochCounter {

    #[inline]
    pub const fn new() -> Self {
        Self (AtomicU8::new(0))
    }

    #[inline]
    pub(crate) fn enter_rcs(&self) {
        let old = self.0.fetch_add(1, Ordering::AcqRel);
        assert!(old % 2 == 0, "Old Epoch counter value should be even!");
    }

    #[inline]
    pub(crate) fn leave_rcs(&self) {

        let old = self.0.fetch_add(1, Ordering::AcqRel);
        assert!(old % 2 != 0, "Old Epoch counter value should be odd!");
    }

    pub(crate) fn get_epoch(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }
}
