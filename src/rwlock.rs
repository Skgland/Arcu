//! This module contains the RwLock and Arc based Rcu
//!
//! This is primarily intended to sanity check the atomic based one in [`super::atomic`]

extern crate alloc;

use std::{
    marker::PhantomData,
    ops::Deref,
    sync::{RwLock, RwLockWriteGuard},
};

use alloc::sync::Arc;

use crate::{
    Rcu, RcuCore, UpdateGuard,
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

// Safety:
//  - callers must ensure the epoch counter are initially in an inactive state
//  - we don't change the state of the epoch counters (epoch counters are unused)
unsafe impl<T, P: EpochCounterPool> RcuCore for RwLockArcu<T, P> {
    type Item = T;
    type Pool = P;

    #[inline]
    fn new(initial: impl Into<Arc<T>>, _epoch_counter_pool: P) -> Self {
        RwLockArcu {
            // active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
            active_value: RwLock::new(initial.into()),
            epoch_counter_pool: PhantomData,
        }
    }

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
}

// Safety: the write lock ensures the serialization of the writes
unsafe impl<T, P: EpochCounterPool> Rcu for RwLockArcu<T, P> {
    type UpdateGuard<'a>
        = RwLockArcuUpdateGuard<'a, Self::Item>
    where
        Self: 'a;

    fn update_lock(&self) -> Self::UpdateGuard<'_> {
        RwLockArcuUpdateGuard(self.active_value.write().unwrap())
    }
}

/// UpdateGuard for RwLockArcu
pub struct RwLockArcuUpdateGuard<'a, T>(RwLockWriteGuard<'a, Arc<T>>);

impl<T> UpdateGuard for RwLockArcuUpdateGuard<'_, T> {
    type Item = T;

    fn replace(mut self, new: impl Into<Arc<Self::Item>>) -> Arc<Self::Item> {
        std::mem::replace(&mut *self.0, new.into())
    }
}

impl<T> Deref for RwLockArcuUpdateGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
