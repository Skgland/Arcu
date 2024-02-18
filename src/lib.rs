// #![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]

//! Arc based Rcu implementation based on my implementation in [mthom/scryer-prolog#1980](https://github.com/mthom/scryer-prolog/pull/1980)
//!
//! ```text
//! Arc
//!  Rcu
//! Arcu
//! ```

extern crate alloc;

pub mod epoch_counters;

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

use crate::epoch_counters::EpochCounter;

pub mod atomic;
pub mod rwlock;

pub mod rcu_ref;

/// ##Safety
/// - When mixing safe and unsafe functions care needs to be taken that write operations see all Epochs used by concurrent read operations
/// - The safe read operations assume that the writer will observe `epoch_counters::THREAD_EPOCH_COUNTER`, see `epoch_counters::with_thread_local_epoch_counter`.
/// - The safe writers assume that the readers will use one of the epoch counters in `epoch_counters::GLOBAL_EPOCH_COUNTERS`, see `epoch_counters::register_epoch_counter`.
pub trait Rcu {
    type Item;


    /// Read the value of the Rcu for the current epoch
    ///
    /// ## Blocking
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
    pub fn read(&self) -> RcuRef<T, T> {
        let arc = crate::epoch_counters::with_thread_local_epoch_counter(|epoch_counter| {
            // Safety:
            // - we just registered the epoch counter
            // - this is a thread local epoch counter that is only used here, so there can't be a concurrent use
            unsafe { self.raw_read(epoch_counter) }
        });

        RcuRef::new(arc)
    }

    /// Replace the Rcu's content with a new value
    ///
    /// This does not synchronize writes and the last to update the active_value pointer wins.
    ///
    /// all writes that do not win will be lost, though not leaked.
    /// This will block until the old value can be reclaimed,
    /// i.e. all threads witnessed to be in the read critical sections
    /// have been witnessed to have left the critical section at least once
    #[inline]
    #[cfg(feature = "global_counters")]
    pub fn replace(&self, new_value: impl Into<Arc<T>>) -> Arc<T> {
        // Safety:
        // - we are using global counters
        unsafe { self.raw_replace(new_value.into(), crate::epoch_counters::global_counters) }
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    #[cfg(feature = "thread_local_counter")]
    pub fn try_update<F, R>(&self, mut update: F) -> Option<Arc<T>>
    where
    F: FnMut(&T) -> Option<R>,
    R: Into<Arc<T>>{
        // Safety:
        // epoch_counter is thread local and as such can't be in use concurrently
        // get_epoch_counters returns the list of all registered epoch counters
        crate::epoch_counters::with_thread_local_epoch_counter(|epoch_counter| unsafe {
            self.raw_try_update(move |old| update(old).map(Into::into), epoch_counter, crate::epoch_counters::global_counters)
        })
    }

    fn new(initial: impl Into<Arc<Self::Item>>) -> Self;

    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must be made available to write operations
    unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<Self::Item>;


    /// ## Safety
    /// - `get_epoch_counters` must return a vector containing all epoch counters used with this Rcu that are odd at the time it is called
    /// - the vector may contain more epoch counters than required, i.e. epoch counters that are even and epoch counters in use with this Rcu
    unsafe fn raw_replace(
        &self,
        new_value: Arc<Self::Item>,
        get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>>,
    ) -> Arc<Self::Item>;


    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    ///
    /// ## Safety
    /// - `epoch_counter` must be valid for `raw_read`
    /// - `get_epoch_counters` must be valid for `raw_replace`
    unsafe fn raw_try_update<'a>(
        &self,
        update: impl FnMut(&Self::Item) -> Option<Arc<Self::Item>>,
        epoch_counter: &EpochCounter,
        get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>> + 'a,
    ) -> Option<Arc<Self::Item>> ;
}
