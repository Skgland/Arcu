//! This module contains the RwLock and Arc based Rcu
//!
//! This is primarily intended to sanity check the atomic based one in [`super::atomic`]

extern crate alloc;

use std::{marker::PhantomData, sync::RwLock};

use alloc::sync::Arc;

use crate::{
    CreateRcu, RawWeakRcu, Rcu,
    epoch_counters::{EpochCounter, EpochCounterPool},
};

/// An Rcu based on an RwLock containing an Arc.
///
/// You probably want the Atomics basec one [`super::atomic::Arcu`].
///
/// This Rcu uses a RwLocks for synchronization instead of the EpochCounterPool.
/// The EpochCounterPool is kept to keep the API compatible with the atomics based one.
pub struct RwLockArcu<T, P> {
    active_value: RwLock<Arc<T>>,
    epoch_counter_pool: PhantomData<P>,
}

impl<T: core::fmt::Display, P> core::fmt::Display for RwLockArcu<T, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        T::fmt(&self.active_value.read().unwrap(), f)
    }
}

impl<T: core::fmt::Debug, P> core::fmt::Debug for RwLockArcu<T, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rcu")
            .field("active_value", &self.active_value.read().unwrap())
            .field("epoch_counter_pool", &"Opaque")
            .finish()
    }
}

impl<T, P: EpochCounterPool> CreateRcu for RwLockArcu<T, P> {
    #[inline]
    fn new(initial: impl Into<Arc<T>>, _epoch_counter_pool: P) -> Self {
        RwLockArcu {
            // active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
            active_value: RwLock::new(initial.into()),
            epoch_counter_pool: PhantomData,
        }
    }
}

// Safety:
//  - callers must ensure the epoch counter are initally in an inactive state
//  - we don't change the state of the epoch counters (epoch counters are unused)
unsafe impl<T, P: EpochCounterPool> RawWeakRcu for RwLockArcu<T, P> {
    type Item = T;
    type Pool = P;

    /// ## Safety
    /// - this impl is actually safe
    #[inline]
    unsafe fn raw_read(&self, _epoch_counter: &EpochCounter) -> Arc<T> {
        self.active_value.read().unwrap().clone()
    }

    #[inline]
    fn replace(&self, new_value: impl Into<Arc<T>>) -> Arc<T> {
        std::mem::replace(&mut self.active_value.write().unwrap(), new_value.into())
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    ///
    /// ## Safety:
    /// - this impl is actually safe
    #[inline]
    unsafe fn raw_weak_try_update<Err>(
        &self,
        mut update: impl for<'a> FnMut(&'a T) -> Result<Arc<T>, Err>,
        _epoch_counter: &EpochCounter,
    ) -> Result<Arc<T>, Err> {
        loop {
            let old = self.active_value.read().unwrap().clone();
            let new = update(&old)?;
            let mut cur = self.active_value.write().unwrap();
            if Arc::ptr_eq(&cur, &old) {
                return Ok(std::mem::replace(&mut cur, new));
            } else {
                println!("Ptr neq, retry!")
            }
        }
    }
}

// Safety: the write lock ensures the serialization of the writes
unsafe impl<T, P: EpochCounterPool> Rcu for RwLockArcu<T, P> {
    fn try_update<Err>(
        &self,
        update: impl FnOnce(&Self::Item) -> Result<Arc<Self::Item>, Err>,
    ) -> Result<Arc<Self::Item>, Err> {
        let mut guard = self.active_value.write().unwrap();
        let new = update(&*guard)?;
        Ok(std::mem::replace(&mut *guard, new))
    }
}
