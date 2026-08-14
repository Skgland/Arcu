//! This module contains the atomic and Arc based Rcu

extern crate alloc;

use core::ops::Deref;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::sync::MutexGuard;
use std::{marker::PhantomData, sync::Mutex};

use alloc::sync::Arc;

#[cfg(feature = "thread_local_counter")]
use crate::epoch_counters::GlobalEpochCounterPool;

use crate::epoch_counters::{EpochCounter, EpochCounterPool};
use crate::{Rcu, RcuCore, UpdateGuard};

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

// Safety:
// - try_update serialized updated by taking the write mutex lock
// - default update impl uses try_update
unsafe impl<T, P: EpochCounterPool> Rcu for StrongAtomicArcu<T, P> {
    type UpdateGuard<'a>
        = StrongAtomicArcuUpdateGuard<'a, T, P>
    where
        Self: 'a;

    fn update_lock(&self) -> Self::UpdateGuard<'_> {
        StrongAtomicArcuUpdateGuard {
            guard: self.write.lock().unwrap(),
            arcu: self,
        }
    }
}

/// UpdateGuard for StrongAtomicArcu
pub struct StrongAtomicArcuUpdateGuard<'a, T, P> {
    arcu: &'a StrongAtomicArcu<T, P>,
    guard: MutexGuard<'a, ()>,
}

impl<T, P: EpochCounterPool> UpdateGuard for StrongAtomicArcuUpdateGuard<'_, T, P> {
    type Item = T;

    fn replace(self, new: impl Into<Arc<Self::Item>>) -> Arc<Self::Item> {
        // exchange old and new
        // the rcu is now responsible for freeing the last strong count of new
        // in turn we must release one strong count of old while ensuring that we
        // don't release the last strong count while readers are still in the critical section
        let old = self
            .arcu
            .active_value
            .swap(Arc::into_raw(new.into()).cast_mut(), Ordering::Release);

        drop(self.guard);

        self.arcu.epoch_counter_pool.wait_for_epochs();

        // Safety:
        //  - we got one strong count from swapping with new (in exchange for a strong count of old)
        unsafe { Arc::from_raw(old) }
    }
}

impl<T, P> Deref for StrongAtomicArcuUpdateGuard<'_, T, P> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // safety:
        // - while this guard is alive only this guard may change this value
        // - for this guard to change the value it needs to be borrowed mutable
        // - while this shared borrow is alive self can't be borrowed mutable
        unsafe { &*self.arcu.active_value.load(Ordering::Acquire) }
    }
}

// safety: each call of `enter_rcs` is paired with a call to `leave_rcs`
unsafe impl<T, P: EpochCounterPool> RcuCore for StrongAtomicArcu<T, P> {
    type Item = T;
    type Pool = P;

    #[inline]
    fn new(initial: impl Into<Arc<T>>, epoch_counter_pool: P) -> Self {
        StrongAtomicArcu {
            active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
            epoch_counter_pool,
            write: Mutex::new(()),
            phantom: PhantomData,
        }
    }

    /// ## Safety
    /// - The epoch counter must not be used concurrently and must be in an inactive state
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
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
}

impl<T, P> Drop for StrongAtomicArcu<T, P> {
    fn drop(&mut self) {
        // Safety:
        // - The Pointer was created by Arc::into_raw
        // - The Arcu is responsible for one strong count, so the string count is at least 1
        unsafe { Arc::from_raw(self.active_value.load(Ordering::Relaxed)) };
    }
}
