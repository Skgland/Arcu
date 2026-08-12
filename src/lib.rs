// #![cfg_attr(not(feature = "std"), no_std)]

#![deny(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

extern crate alloc;

pub mod epoch_counters;

use std::ops::Deref;

use alloc::sync::Arc;
use epoch_counters::EpochCounterPool;

use crate::epoch_counters::EpochCounter;
#[cfg(feature = "thread_local_counter")]
use crate::epoch_counters::GlobalEpochCounterPool;

pub mod rwlock;
pub mod strong_atomic;
pub mod weak_atomic;

pub mod mapped_arc;

mod doc_tests;

mod never;

/// Functionality shared by Rcu and WeakRcu
///
/// # Safety
/// - functions that take an epoch counter must ensure that that epoch counter is in an inactive state on return
pub unsafe trait RcuCore {
    /// The type contained in this Rcu
    type Item;

    /// The type for the pool of epoch counters used by this Rcu
    type Pool: EpochCounterPool;

    /// Create a new Rcu with the given initial value and epoch counter pool
    fn new(initial: impl Into<Arc<Self::Item>>, epoch_counter_pool: Self::Pool) -> Self;

    /// Replace the Rcu's content with a new value
    fn replace(&self, new_value: impl Into<Arc<Self::Item>>) -> Arc<Self::Item>;

    /// ## Safety
    /// - The epoch counter must not be used concurrently and must be in an inactive state
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
    unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<Self::Item>;

    // Read the value of the Rcu for the current epoch
    ///
    /// ## Blocking
    ///
    /// The initial read on each thread may block while registering the epoch counter.
    /// Further read on the same thread won't block even for different Rcu.
    ///
    /// ## Procedure
    ///
    /// 1. Register the Epoch Counter (only done once per thread, may block)
    /// 2. atomically increment the epoch counter (by one from even to odd)
    /// 3. atomically load the arc pointer
    /// 4. atomically increment the arc strong count
    /// 5. atomically increment the epoch counter (by one from odd back to even)
    #[cfg(feature = "thread_local_counter")]
    fn read(&self) -> Arc<Self::Item>
    where
        Self: RcuCore<Pool = GlobalEpochCounterPool>,
    {
        use crate::epoch_counters::GlobalEpochCounterPool;

        GlobalEpochCounterPool.with_thread_local_epoch_counter(|epoch_counter| {
            // Safety:
            // - callers of EpochCounter::enter_rcs must ensure this function isn't called while the epoch counter is active
            // - the thread local epoch counter is registered with the global epoch counter pool which is used by this rcu
            unsafe { self.raw_read(epoch_counter) }
        })
    }
}

/// Defines the operation available on the Rcu::UpdateGuard returned by Rcu::update_lock
pub trait UpdateGuard: Deref<Target = Self::Item> {
    /// The guarded item type
    type Item;

    /// Replace the current value by the new value
    fn replace(self, new: impl Into<Arc<Self::Item>>) -> Arc<Self::Item>;
}

/// An abstract Rcu to abstract over the atomic based [`atomic::Arcu`] and the RwLock based [`rwlock::Arcu`]
///
/// # Safety
/// - update and try_update must ensure that updates don't race i.e. are serialized
pub unsafe trait Rcu: RcuCore {
    /// The type returned by Rcu::update_lock
    type UpdateGuard<'a>: UpdateGuard<Item = Self::Item>
    where
        Self: 'a;

    /// Lock the Rcu to prevent it from being replaced concurrently.
    fn update_lock(&self) -> Self::UpdateGuard<'_>;

    /// Update the Rcu's contained value.
    /// Concurrent updates will be serialized
    fn update(&self, update: impl FnOnce(&Self::Item) -> Arc<Self::Item>) -> Arc<Self::Item> {
        let guard = self.update_lock();
        let new = update(&guard);
        guard.replace(new)
    }

    /// Try to update the Rcu's contained value.
    /// If the update function returns an error that error is returned and the contained value isn't updated
    /// Concurrent updates attempts will be serialized.
    fn try_update<Err>(
        &self,
        update: impl FnOnce(&Self::Item) -> Result<Arc<Self::Item>, Err>,
    ) -> Result<Arc<Self::Item>, Err> {
        let guard = self.update_lock();
        let new = update(&guard)?;
        Ok(guard.replace(new))
    }
}

/// # Safety
/// - functions that take an epoch counter must ensure that that epoch counter is in an inactive state on return
pub unsafe trait WeakRcu: RcuCore {
    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    ///
    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
    unsafe fn raw_weak_update(
        &self,
        mut update: impl for<'a> FnMut(&Self::Item) -> Arc<Self::Item>,
        epoch_counter: &EpochCounter,
    ) -> Arc<Self::Item> {
        // Safety: all obligations discharged to caller
        match unsafe {
            self.raw_weak_try_update(|item| Ok::<_, never::Never>(update(item)), epoch_counter)
        } {
            Ok(result) => result,
        }
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns Err(_)
    ///
    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
    unsafe fn raw_weak_try_update<Err>(
        &self,
        update: impl for<'a> FnMut(&Self::Item) -> Result<Arc<Self::Item>, Err>,
        epoch_counter: &EpochCounter,
    ) -> Result<Arc<Self::Item>, Err>;

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    #[cfg(feature = "thread_local_counter")]
    fn weak_update<F, R>(&self, mut update: F) -> Arc<Self::Item>
    where
        F: FnMut(&Self::Item) -> R,
        R: Into<Arc<Self::Item>>,
        Self: RcuCore<Pool = GlobalEpochCounterPool>,
    {
        match self.weak_try_update(|item| Ok::<_, never::Never>(update(item))) {
            Ok(result) => result,
        }
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns Err(_)
    #[cfg(feature = "thread_local_counter")]
    fn weak_try_update<F, R, Err>(&self, mut update: F) -> Result<Arc<Self::Item>, Err>
    where
        F: FnMut(&Self::Item) -> Result<R, Err>,
        R: Into<Arc<Self::Item>>,
        Self: RcuCore<Pool = GlobalEpochCounterPool>,
    {
        use crate::epoch_counters::GlobalEpochCounterPool;

        GlobalEpochCounterPool.with_thread_local_epoch_counter(|epoch_counter| {
            // Safety:
            // - callers of EpochCounter::enter_rcs must ensure this function isn't called while the epoch counter is active
            // - the thread local epoch counter is registered with the global epoch counter pool used by this rcu
            unsafe {
                self.raw_weak_try_update(move |old| update(old).map(Into::into), epoch_counter)
            }
        })
    }
}

// Safety: we don't use the epoch counter and the caller must provided it in the correct (inactive) state
unsafe impl<T: Rcu> WeakRcu for T {
    unsafe fn raw_weak_try_update<Err>(
        &self,
        update: impl for<'a> FnMut(&Self::Item) -> Result<Arc<Self::Item>, Err>,
        _epoch_counter: &EpochCounter,
    ) -> Result<Arc<Self::Item>, Err> {
        self.try_update(update)
    }
}
