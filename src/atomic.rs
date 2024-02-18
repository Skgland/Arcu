extern crate alloc;

use core::sync::atomic::{AtomicPtr, Ordering};
use std::mem::ManuallyDrop;

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
    active_value: AtomicPtr<T>,
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

/// ##Safety
/// - When mixing safe and unsafe functions care needs to be taken that write operations see all Epochs used by concurrent read operations
/// - The safe read operations assume that the writer will observe `epoch_counters::THREAD_EPOCH_COUNTER`, see `epoch_counters::with_thread_local_epoch_counter`.
/// - The safe writers assume that the readers will use one of the epoch counters in `epoch_counters::GLOBAL_EPOCH_COUNTERS`, see `epoch_counters::register_epoch_counter`.
impl<T> Rcu for Arcu<T> {
    type Item = T;

    #[inline]
    fn new(initial: impl Into<Arc<T>>) -> Self {
        Arcu {
            active_value: AtomicPtr::new(Arc::into_raw(initial.into()).cast_mut()),
        }
    }

    /// ## Safety
    /// - The epoch counter must not be used concurrently
    /// - The epoch counter must be made available to write operations
    #[inline]
    unsafe fn raw_read(&self, epoch_counter: &EpochCounter) -> Arc<T> {
        epoch_counter.enter_rcs();

        let arc_ptr = self.active_value.load(Ordering::Acquire);

        core::sync::atomic::fence(Ordering::SeqCst);

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

        core::sync::atomic::fence(Ordering::SeqCst);

        epoch_counter.leave_rcs();

        arc
    }

    /// ## Safety
    /// - `get_epoch_counters` must return a vector containing all epoch counters used with this Rcu that are odd at the time it is called
    /// - the vector may contain more epoch counters than required, i.e. epoch counters that are even and epoch counters in use with this Rcu
    #[inline]
    unsafe fn raw_replace(
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

        wait_for_epochs(get_epoch_counters);

        // Safety:
        // - we have not dropped the arc another way
        // - we witnessed all threads either with an even epoch count or with a new odd count,
        //   as such they must have left the critical section at some point
        ManuallyDrop::into_inner(arc)
    }

    /// Update the Rcu using the provided update function
    /// Retries when the Rcu has been updated/replaced between reading the old value and writing the new value
    /// Aborts when the update function returns None
    ///
    /// ## Safety
    /// - `epoch_counter` must be valid for `raw_read`
    /// - `get_epoch_counters` must be valid for `raw_replace`
    unsafe fn raw_try_update<'a>(
        &self,
        mut update: impl FnMut(&T) -> Option<Arc<T>>,
        epoch_counter: &EpochCounter,
        get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>> + 'a,
    ) -> Option<Arc<T>> {

        loop {
            let old = self.raw_read(epoch_counter);

            let new = Arc::into_raw(update(&old)?);

            // we now exchange the ownership of rcu(old) for rcu(new)
            // if rcu(?) is rcu(old)
            let result = self.active_value.compare_exchange(
                Arc::as_ptr(&old).cast_mut(),
                new.cast_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            match result {
                Ok(old2) => {
                    // rcu(?) was rcu(old) so we exchanged an old strong count ownership for a new strong count ownership
                   assert_eq!(Arc::as_ptr(&old), old2);

                    // Compare Exchange Succeeded, ensure the old Arc gets dropped after waiting for all readers to leave the read critical section
                    //
                    // manually drop as we need to ensure not to drop the arc while
                    // we have not witnessed all threads to be or have been outside the read critical section
                    // i.e. even epoch counter or different odd epoch counter
                    // Safety:
                    // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
                    // - the Rcu itself holds one strong count

                    // we exchanged the old/new arc pointer
                    // we are now responsible for the rcu strong count of old
                    // in exchange for giving the rcu the responsibility of the strong count of new

                    // reclaim the old arc from the pointer we exchanged
                    let arc = unsafe { ManuallyDrop::new(Arc::from_raw(old2)) };

                    wait_for_epochs(get_epoch_counters);

                    // Note that we have one strong count more than at the beginning of the loop iteration
                    // and it is now safe to drop arc so we can return it to the caller

                    // Safety:
                    // - we have not dropped the arc another way
                    // - we witnessed all threads either with an even epoch count or with a new odd count,
                    //   as such they must have left the critical section at some point
                    return Some(ManuallyDrop::into_inner(arc));
                }
                Err(_new_old) => {
                    // Compare Exchange failed, reclaim the new arc we leaked with Arc::into_raw above

                    // Safety:
                    // - the ptr was just created using Arc::into_raw
                    // - there still one strong count left

                    // we haven't exchanged the references so we are still responsible to clean up one strong count of new
                    Arc::decrement_strong_count(new);

                    continue;
                }
            }
        }
    }
}

// Safety:
// This function must not return normally until all epoch counters have been witnessed to be even or to have changed
fn wait_for_epochs(get_epoch_counters: impl FnOnce() -> Vec<Weak<EpochCounter>>) {
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
}
