#![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]

//! Arc based Rcu implementation based on my implementation in [mthom/scryer-prolog#1980](https://github.com/mthom/scryer-prolog/pull/1980)
//!
//! ```
//! Arc
//!  Rcu
//! Arcu
//! ```

extern crate alloc;

use core::{
    fmt::Debug,
    mem::ManuallyDrop,
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

mod epoch_counters;
pub use epoch_counters::EpochCounter;

pub struct Arcu<T> {
    active_value: AtomicPtr<T>,
}

#[cfg(feature = "std")]
impl<T: core::fmt::Display> core::fmt::Display for Arcu<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let data = self.read();
        core::fmt::Display::fmt(&data.deref(), f)
    }
}

#[cfg(feature = "std")]
impl<T: core::fmt::Debug> core::fmt::Debug for Arcu<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rcu")
            .field("active_value", &self.read())
            .finish()
    }
}

impl<T> Arcu<T> {
    pub fn new(initial_value: T) -> Self {
        Self::from_arc(Arc::new(initial_value))
    }

    #[inline]
    pub fn from_arc(initial: Arc<T>) -> Self {
        Arcu {
            active_value: AtomicPtr::new(Arc::into_raw(initial).cast_mut()),
        }
    }

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
    #[cfg(feature = "std")]
    pub fn read(&self) -> RcuRef<T, T> {
        let arc = epoch_counters::THREAD_EPOCH_COUNTER.with(|epoch_counter| {
            let epoch_counter = epoch_counter.get_or_init(|| {
                let epoch_counter = Arc::new(EpochCounter::new());

                // register the current threads epoch counter on init
                epoch_counters::EPOCH_COUNTERS.write().unwrap().push(Arc::downgrade(&epoch_counter));

                epoch_counter
            });

            // Safety:
            // - we just registered the epoch counter
            // - this is a thread local epoch counter that is only used here, so there can't be a concurrent use
            unsafe { self.raw_read(epoch_counter) }
        });

        RcuRef {
            data: arc.deref().into(),
            arc,
        }
    }

    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must be made available to write operations
    #[inline]
    pub unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<T> {
        epoch_counter.enter_rcs();

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

        epoch_counter.leave_rcs();

        arc
    }

    /// Create a new Arc containing the new value and replace the Rcu's current arc.
    ///
    /// Concurrent replace operations will behave as if serialized into some order.
    ///
    /// ## Blocking
    /// The replace operation will block until all epoch counters have been observed to be even or to have changed
    ///
    /// ## Returns
    /// The replaced Arc
    ///
    #[cfg(feature = "std")]
    pub fn replace(&self, new_value: T) -> Arc<T> {
        self.replace_arc(Arc::new(new_value))
    }

    /// Replace the Rcu's content with a new value
    ///
    /// This does not synchronize writes and the last to update the active_value pointer wins.
    ///
    /// all writes that do not win will be lost, though not leaked.
    /// This will block until the old value can be reclaimed,
    /// i.e. all threads witnessed to be in the read critical sections
    /// have been witnessed to have left the critical section at least once
    #[cfg(feature = "std")]
    #[inline]
    pub fn replace_arc(&self, new_value: Arc<T>) -> Arc<T> {
        unsafe { self.raw_replace_arc(new_value, || epoch_counters::EPOCH_COUNTERS.read().unwrap().clone()) }
    }

    /// ## Safety
    /// - `get_epoch_counters` must return a vector containing all epoch counters used with this Rcu that are odd at the time it is called
    /// - the vector may contain more epoch counters than required, i.e. epoch counters that are even and epoch counters in use with this Rcu
    #[inline]
    pub unsafe fn raw_replace_arc(
        &self,
        new_value: Arc<T>,
        get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>>,
    ) -> Arc<T> {
        let arc_ptr = self
            .active_value
            .swap(Arc::into_raw(new_value).cast_mut(), Ordering::AcqRel);

        // manually drop as we need to ensure not to drop the arc while
        // we have not witnessed all threads to be or have been outside the read critical section
        // i.e. even epoch counter or different odd epoch counter
        // Safety:
        // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
        // - the Rcu itself holds one strong count
        let arc = unsafe { ManuallyDrop::new(Arc::from_raw(arc_ptr)) };

        // Get the current state of the epoch counters,
        // we can only drop the old value once we have observed all to be even or to have changed
        let epochs = get_epoch_counters();

        let mut epochs = epochs
            .into_iter()
            .flat_map(|elem| {
                let arc = elem.upgrade()?;
                let init_val = arc.get_epoch();
                if init_val % 2 == 0 {
                    // already even can be ignored
                    return None;
                }
                // odd initial value thread is in the read critical section
                // we need to wait for the value to change before we can drop the arc
                Some((init_val, elem))
            })
            .collect::<Vec<_>>();

        while !epochs.is_empty() {
            epochs.retain(|elem| {
                let Some(arc) = elem.1.upgrade() else {
                    // as the thread is dead it can't have a pointer to the old arc
                    return false;
                };
                // the epoch counter has not changed so the thread is still in the same instance of the critical section
                // any different value is ok as
                // - even values indicate the thread is outside of the critical section
                // - a different odd value indicates the thread has left the critical section and can subsequently only read the new active_value
                arc.get_epoch() == elem.0
            })
        }

        // Safety:
        // - we have not dropped the arc another way
        // - we witnessed all threads either with an even epoch count or with a new odd count,
        //   as such they must have left the critical section at some point
        ManuallyDrop::into_inner(arc)
    }
}

pub struct RcuRef<T, M>
where
    T: ?Sized,
    M: ?Sized,
{
    arc: Arc<T>,
    data: NonNull<M>,
}

impl<T: ?Sized, M: ?Sized + Debug> Debug for RcuRef<T, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RcuRef")
            .field("data", &self.deref())
            .finish()
    }
}

// use associated functions rather than methods so that we don't overlap
// with functions of the Deref Target type
impl<T: ?Sized, M: ?Sized> RcuRef<T, M> {
    pub fn map<N: ?Sized, F: for<'a> FnOnce(&'a M) -> &'a N>(
        reference: Self,
        f: F,
    ) -> RcuRef<T, N> {
        RcuRef {
            arc: reference.arc,
            // Safety: See deref
            data: f(unsafe { reference.data.as_ref() }).into(),
        }
    }

    pub fn try_map<N: ?Sized, F: for<'a> FnOnce(&'a M) -> Option<&'a N>>(
        reference: Self,
        f: F,
    ) -> Option<RcuRef<T, N>> {
        // Safety: See deref
        let val = f(unsafe { reference.data.as_ref() })?;
        Some(RcuRef {
            arc: Arc::clone(&reference.arc),
            data: val.into(),
        })
    }

    pub fn same_epoch<M2>(this: &Self, other: &RcuRef<T, M2>) -> bool {
        Arc::ptr_eq(&this.arc, &other.arc)
    }

    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.data == other.data
    }

    #[allow(clippy::should_implement_trait)]
    pub fn clone(this: &Self) -> Self {
        Self {
            arc: Arc::clone(&this.arc),
            data: this.data,
        }
    }

    pub fn get_root(this: &Self) -> &T {
        &this.arc
    }
}

impl<T: ?Sized, M: ?Sized> Deref for RcuRef<T, M> {
    type Target = M;

    fn deref(&self) -> &Self::Target {
        // Safety: The pointer points into the arc we are holding
        // while we are alive so is the target
        // as the content is in an Rcu no mutable access is given out
        unsafe { self.data.as_ref() }
    }
}
