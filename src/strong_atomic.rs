//! Thi module contains the atomic and Arc based Rcu

extern crate alloc;

#[cfg(feature = "thread_local_counter")]
use core::ops::Deref;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::{marker::PhantomData, sync::Mutex};

use alloc::sync::Arc;

use crate::Rcu;
use crate::{
    CreateRcu, RawWeakRcu,
    epoch_counters::{EpochCounter, EpochCounterPool},
};
#[cfg(feature = "thread_local_counter")]
use crate::{ThreadLocalRcuRead, epoch_counters::GlobalEpochCounterPool};

/// A Rcu based on an atomic pointer to an [`Arc`] and a [`EpochCounterPool`]
///
pub struct StrongAtomicArcu<T, P> {
    // Safety invariant
    // - the pointer has been created with Arc::into_raw
    // - Arcu "owns" one strong reference count
    active_value: AtomicPtr<T>,
    epoch_counter_pool: P,
    write: Mutex<()>,
    phantom: PhantomData<Arc<T>>,
}

#[cfg(feature = "thread_local_counter")]
impl<T: core::fmt::Display> core::fmt::Display for StrongAtomicArcu<T, GlobalEpochCounterPool> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let data = self.read();
        core::fmt::Display::fmt(&data.deref(), f)
    }
}

impl<T: core::fmt::Debug, P> core::fmt::Debug for StrongAtomicArcu<T, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rcu")
            .field("active_value", &"Opaque")
            .field("epoch_counter_pool", &"Opaque")
            .finish()
    }
}

impl<T, P: EpochCounterPool> CreateRcu for StrongAtomicArcu<T, P> {
    #[inline]
    fn new(initial: impl Into<Arc<T>>, epoch_counter_pool: P) -> Self {
        StrongAtomicArcu {
            active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
            epoch_counter_pool,
            write: Mutex::new(()),
            phantom: PhantomData,
        }
    }
}

// Safety:
//       - try_update serialized updated by taking the write mutex lock
//       - default update impl uses try_update
unsafe impl<T, P: EpochCounterPool> Rcu for StrongAtomicArcu<T, P> {
    fn try_update<Err>(
        &self,
        update: impl FnOnce(&Self::Item) -> Result<Arc<Self::Item>, Err>,
    ) -> Result<Arc<Self::Item>, Err> {
        let write_guard = self.write.lock();
        let arc_ptr = self.active_value.load(Ordering::Acquire);

        // Safety: See comments inside the block
        let old: Arc<T> = unsafe {
            // Safety:
            // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
            // - the Rcu is responsible for of the arc's strong references
            // - the Rcu is alive as this function takes a reference to the Rcu
            // - we have the write lock so there won't be a concurrent decrement
            Arc::increment_strong_count(arc_ptr);
            // Safety:
            // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
            // - we have just ensured an additional strong count by incrementing the count
            Arc::from_raw(arc_ptr)
        };

        let new = update(&old)?;

        // exchange old and new
        // the rcu is now responsible for freeing the last strong count of new
        // in turn we must release one strong count of old while ensuring that we
        // don't release the last strong count while readers are still in the critical section
        let old2 = self
            .active_value
            .swap(Arc::into_raw(new).cast_mut(), Ordering::Release);

        // verify that old and old2 point to the same arc
        // this should always hold as we have the write lock
        // so only check when debug assertions are enabled
        debug_assert_eq!(old2, arc_ptr);

        // Safety:
        //  - we got one strong count from swapping with new (in exchange for a )
        //  - the arc is still kept alive by old so we won't invalidate readers
        unsafe { Arc::decrement_strong_count(old2) };

        drop(write_guard);

        self.epoch_counter_pool.wait_for_epochs();

        Ok(old)
    }
}

// safety: each call of `enter_rcs` is paired with a call to `leave_rcs`
unsafe impl<T, P: EpochCounterPool> RawWeakRcu for StrongAtomicArcu<T, P> {
    type Item = T;
    type Pool = P;

    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must be made available to write operations
    #[inline]
    unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<T> {
        // safety: caller obligation
        let rcs_guard = unsafe { epoch_counter.enter_rcs() };

        let arc_ptr = self.active_value.load(Ordering::Acquire);

        // Safety: See comments inside the block
        let arc = unsafe {
            // Safety:
            // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
            // - the Rcu is responsible for of the arc's strong references
            // - the Rcu is alive as this function takes a reference to the Rcu
            // - replace will wait with decrementing the old values strong count until our epoch counter is even again
            Arc::increment_strong_count(arc_ptr);
            // Safety:
            // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
            // - we have just ensured an additional strong count by incrementing the count
            Arc::from_raw(arc_ptr)
        };

        drop(rcs_guard);

        arc
    }

    #[inline]
    fn replace(&self, new_value: impl Into<Arc<T>>) -> Arc<T> {
        self.update(move |_| new_value.into())
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    ///
    /// ## Safety
    /// - `epoch_counter` must be valid for `raw_read`
    /// - `get_epoch_counters` must be valid for `raw_replace`
    unsafe fn raw_weak_try_update<Err>(
        &self,
        update: impl for<'a> FnMut(&'a T) -> Result<Arc<T>, Err>,
        _epoch_counter: &EpochCounter,
    ) -> Result<Arc<T>, Err> {
        self.try_update(update)
    }
}

impl<T, P> Drop for StrongAtomicArcu<T, P> {
    fn drop(&mut self) {
        // Safety:
        // - The Pointer was created by Arc::into_raw
        // - The Arcu is responsible for one strong count, so the string count is at least 1
        unsafe { Arc::from_raw(self.active_value.load(Ordering::Relaxed)) };
    }
}
