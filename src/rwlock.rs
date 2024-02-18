extern crate alloc;


use std::sync::RwLock;

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

use crate::epoch_counters::EpochCounter;

use super::Rcu;

pub struct Arcu<T> {
    // Safety invariant
    // - the pointer has been created with Arc::into_raw
    // - Arcu "owns" one strong reference count
    // active_value: AtomicPtr<T>,
    active_value: RwLock<Arc<T>>,
}

#[cfg(feature = "thread_local_counters")]
impl<T: core::fmt::Display> core::fmt::Display for Arcu<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let data = self.read();
        core::fmt::Display::fmt(&data.deref(), f)
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Arcu<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        #[cfg(feature = "thread_local_counters")]
        {
            f.debug_struct("Rcu")
                .field("active_value", &self.read().deref())
                .finish();
        }
        #[cfg(not(feature = "thread_local_counters"))]
        {
            f.debug_struct("Rcu")
                .field("active_value", &"Opaque")
                .finish()
        }
    }
}

impl<T> Rcu for Arcu<T> {
    type Item = T;

    #[inline]
    fn new(initial: impl Into<Arc<T>>) -> Self {
        Arcu {
            // active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
            active_value: RwLock::new(initial.into()),
        }
    }

    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must be made available to write operations
    #[inline]
    unsafe fn raw_read(&self, _epoch_counter: &EpochCounter) -> Arc<T> {
        self.active_value.read().unwrap().clone()
    }

    /// ## Safety
    /// - `get_epoch_counters` must return a vector containing all epoch counters used with this Rcu that are odd at the time it is called
    /// - the vector may contain more epoch counters than required, i.e. epoch counters that are even and epoch counters in use with this Rcu
    #[inline]
    unsafe fn raw_replace(
        &self,
        new_value: Arc<T>,
        _get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>>,
    ) -> Arc<T> {
        std::mem::replace(&mut self.active_value.write().unwrap(), new_value)
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    ///
    /// ## Safety
    /// - `epoch_counter` must be valid for `raw_read`
    /// - `get_epoch_counters` must be valid for `raw_replace`
    #[inline]
    unsafe fn raw_try_update<'a>(
        &self,
        mut update: impl FnMut(&T) -> Option<Arc<T>>,
        _epoch_counter: &EpochCounter,
        _get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>> + 'a,
    ) -> Option<Arc<T>> {

        loop {
            let old = self.active_value.read().unwrap().clone();
            let new = update(&old)?;
            let mut cur = self.active_value.write().unwrap();
            if Arc::ptr_eq(&cur, &old) {
                return Some(std::mem::replace(&mut cur, new))
            } else {
                println!("Ptr neq, retry!")
            }
        }
    }
}

impl<T> Arcu<T> {


}
