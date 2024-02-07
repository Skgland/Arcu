use core::sync::atomic::{AtomicU8, Ordering};

// the epoch counters of all threads that have ever accessed an Rcu
// threads that have finished will have a dangling Weak reference and can be cleaned up
// having this be shared between all Rcu's is a tradeof:
// - writes will be slower as more epoch counters need to be waited for
// - reads should be faster as a thread only needs to register itself once on the first read
#[cfg(feature = "global_counters")]
static GLOBAL_EPOCH_COUNTERS: std::sync::RwLock<Vec<alloc::sync::Weak<EpochCounter>>> =
    std::sync::RwLock::new(Vec::new());

#[cfg(feature = "global_counters")]
pub fn register_epoch_counter(epoch_counter: alloc::sync::Weak<EpochCounter>) {
    GLOBAL_EPOCH_COUNTERS.write().unwrap().push(epoch_counter)
}

#[cfg(feature = "global_counters")]
pub fn global_counters() -> Vec<::alloc::sync::Weak<EpochCounter>>{
    GLOBAL_EPOCH_COUNTERS.read().unwrap().clone()
}

#[cfg(feature = "thread_local_counter")]
thread_local! {
    // odd value means the current thread is about to access the active_epoch of an Rcu
    // - threads observing this while leaving the write critical section will need to wait for this to change to a different (odd or even) value
    // a thread has a single epoch counter for all Rcu it accesses, as a thread can only access one Rcu at a time
    static THREAD_EPOCH_COUNTER: std::cell::OnceCell<std::sync::Arc<EpochCounter>> = const { std::cell::OnceCell::new() };
}

/// Calls the provided function with the thread local epoch counter
///
/// Per Thread: On first use registers the epoch counter
#[cfg(feature = "thread_local_counter")]
pub fn with_thread_local_epoch_counter<T>(fun: impl FnOnce(&EpochCounter) -> T) -> T {
    use alloc::sync::Arc;

    THREAD_EPOCH_COUNTER.with(|epoch_counter| {
        let epoch_counter = epoch_counter.get_or_init(|| {
            let epoch_counter = Arc::new(EpochCounter::new());

            // register the current threads epoch counter on init
            register_epoch_counter(Arc::downgrade(&epoch_counter));

            epoch_counter
        });

        fun(&epoch_counter)
    })
}

#[repr(transparent)]
pub struct EpochCounter(core::sync::atomic::AtomicU8);

impl EpochCounter {
    #[inline]
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
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
