// #![cfg_attr(not(feature = "std"), no_std)]

#![deny(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

extern crate alloc;

pub mod epoch_counters;

use alloc::sync::Arc;
use epoch_counters::EpochCounterPool;

#[cfg(feature = "thread_local_counter")]
use crate::epoch_counters::GlobalEpochCounterPool;
use crate::epoch_counters::EpochCounter;

pub mod rwlock;
pub mod strong_atomic;
pub mod weak_atomic;

pub mod rcu_ref;

mod doc_tests;

mod never;

/// An abstract Rcu to abstract over the atomic based [`atomic::Arcu`] and the RwLock based [`rwlock::Arcu`]
///
/// # Safety
/// - update and try_update must ensure that updates are serialized
pub unsafe trait Rcu: RawWeakRcu {
    /// Update the Rcu's contained value.
    /// Concurrent updates will be serialized
    fn update(&self, update: impl FnOnce(&Self::Item) -> Arc<Self::Item>) -> Arc<Self::Item> {
        match self.try_update(|item| Ok::<_, never::Never>(update(item))) {
            Ok(result) => result,
        }
    }

    /// Try to update the Rcu's contained value.
    /// If the update function returns an error that error is returned and the contained value isn't updated
    /// Concurrent updates attempts will be serialized.
    fn try_update<Err>(
        &self,
        update: impl FnOnce(&Self::Item) -> Result<Arc<Self::Item>, Err>,
    ) -> Result<Arc<Self::Item>, Err>;
}

/// Provides a read operation on the Rcu using the thread local epoch counter
#[cfg(feature = "thread_local_counter")]
pub trait ThreadLocalRcuRead: RawWeakRcu<Pool = GlobalEpochCounterPool> {
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
    fn read(&self) -> rcu_ref::RcuRef<Self::Item, Self::Item> {
        use crate::epoch_counters::GlobalEpochCounterPool;

        let arc = GlobalEpochCounterPool.with_thread_local_epoch_counter(|epoch_counter| {
            // Safety:
            // - callers of EpochCounter::enter_rcs must ensure this function isn't called while the epoch counter is active
            // - the thread local epoch counter will be registered with the global epoch counter pool
            unsafe { self.raw_read(epoch_counter) }
        });

        rcu_ref::RcuRef::<Self::Item, Self::Item>::new(arc)
    }
}

#[cfg(feature = "thread_local_counter")]
impl<T> ThreadLocalRcuRead for T where T: RawWeakRcu<Pool = epoch_counters::GlobalEpochCounterPool> {}

/// Provides weak update operations on the Rcu using the thread local epoch counter.
/// Weak update operations may fail resulting in them being retried until success or error.
#[cfg(feature = "thread_local_counter")]
pub trait ThreadLocalRcuWeakUpdate:
    RawWeakRcu<Pool = epoch_counters::GlobalEpochCounterPool>
{
    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    fn weak_update<F, R>(&self, mut update: F) -> Arc<Self::Item>
    where
        F: FnMut(&Self::Item) -> R,
        R: Into<Arc<Self::Item>>,
    {
        match self.weak_try_update(|item| Ok::<_, never::Never>(update(item))) {
            Ok(result) => result,
        }
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns Err(_)
    fn weak_try_update<F, R, Err>(&self, mut update: F) -> Result<Arc<Self::Item>, Err>
    where
        F: FnMut(&Self::Item) -> Result<R, Err>,
        R: Into<Arc<Self::Item>>,
    {
        use crate::epoch_counters::GlobalEpochCounterPool;

        GlobalEpochCounterPool.with_thread_local_epoch_counter(|epoch_counter| {
            // Safety:
            // - callers of EpochCounter::enter_rcs must ensure this function isn't called while the epoch counter is active
            // - the thread local epoch counter will be registered with the global epoch counter pool
            unsafe {
                self.raw_weak_try_update(move |old| update(old).map(Into::into), epoch_counter)
            }
        })
    }
}

#[cfg(feature = "thread_local_counter")]
impl<T> ThreadLocalRcuWeakUpdate for T where
    T: RawWeakRcu<Pool = epoch_counters::GlobalEpochCounterPool>
{
}

/// # Safety
/// - functions that take an epoch counter must ensure that that epoch counter is in an inactive state on return
pub unsafe trait RawWeakRcu {
    /// The type contained in this Rcu
    type Item;

    /// The type for the pool of epoch counters used by this Rcu
    type Pool: EpochCounterPool;

    /// ## Safety
    /// - The epoch counter must not be used concurrently and must be in an inactive state
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
    unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<Self::Item>;

    /// Replace the Rcu's content with a new value
    ///
    /// This does not synchronize writes and for racing writes it is undefined which write wins.
    ///
    /// All writes that do not win will be lost, though not leaked.
    /// This will block until the old value can be reclaimed,
    /// i.e. all threads witnessed to be in the read critical sections
    /// have been witnessed to have left the critical section at least once
    fn replace(&self, new_value: impl Into<Arc<Self::Item>>) -> Arc<Self::Item>;

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    ///
    /// ## Safety
    /// - same requirements as raw_weak_try_update
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
    /// Aborts when the update function returns None
    ///
    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must belong to the EpochCounterPool of this Rcu
    unsafe fn raw_weak_try_update<Err>(
        &self,
        update: impl for<'a> FnMut(&Self::Item) -> Result<Arc<Self::Item>, Err>,
        epoch_counter: &EpochCounter,
    ) -> Result<Arc<Self::Item>, Err>;
}

/// Allows creating any Rcu that can be created from just the initial value and epoch counter pool
pub trait CreateRcu: RawWeakRcu {
    /// Create a new Rcu with the given initial value and epoch counter pool
    fn new(initial: impl Into<Arc<Self::Item>>, epoch_counter_pool: Self::Pool) -> Self;
}
