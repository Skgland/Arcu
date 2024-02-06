#![deny(clippy::undocumented_unsafe_blocks)]

//! Arc based Rcu implementation based on my implementation in [mthom/scryer-prolog#1980](https://github.com/mthom/scryer-prolog/pull/1980)
//!
//! ```
//! Arc
//!  Rcu
//! Arcu
//! ```

extern crate alloc;

use core::{fmt::Debug, mem::ManuallyDrop, ops::Deref, ptr::NonNull, sync::atomic::{AtomicPtr, AtomicU8}, cell::OnceCell};

use alloc::sync::{Arc, Weak};

// TODO find a no_std RwLock
use std::sync::RwLock;

// the epoch counters of all threads that have ever accessed an Rcu
// threads that have finished will have a dangling Weak reference and can be cleaned up
// having this be shared between all Rcu's is a tradeof:
// - writes will be slower as more epoch counters need to be waited for
// - reads should be faster as a thread only needs to register itself once on the first read
static EPOCH_COUNTERS: RwLock<Vec<Weak<AtomicU8>>> = RwLock::new(Vec::new());

thread_local! {
    // odd value means the current thread is about to access the active_epoch of an Rcu
    // - threads observing this while leaving the write critical section will need to wait for this to change to a different (odd or even) value
    // a thread has a single epoch counter for all Rcu it accesses, as a thread can only access one Rcu at a time
    static THREAD_EPOCH_COUNTER: OnceCell<Arc<AtomicU8>> = const { OnceCell::new() };
}

pub struct Rcu<T> {
    active_value: AtomicPtr<T>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Rcu<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let active_epoch = self.read();
        f.debug_struct("Rcu")
            .field("active_value", &active_epoch)
            .finish()
    }
}

impl<T> Rcu<T> {

    pub fn new(initial_value: T) -> Self {
        Self::from_arc(Arc::new(initial_value))
    }

    #[inline]
    pub fn from_arc(initial: Arc<T>) -> Self {
        Rcu {
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
    pub fn read(&self) -> RcuRef<T, T> {
        THREAD_EPOCH_COUNTER.with(|epoch_counter| {
            let epoch_counter = epoch_counter.get_or_init(|| {
                let epoch_counter = Arc::new(AtomicU8::new(0));
                // register the current threads epoch counter on init
                EPOCH_COUNTERS.write().unwrap()
                    .push(Arc::downgrade(&epoch_counter));
                epoch_counter
            });

            let old = epoch_counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            assert!(old % 2 == 0, "Old Epoch counter value should be even!");
        });

        let arc_ptr = self.active_value.load(std::sync::atomic::Ordering::Acquire);

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

        THREAD_EPOCH_COUNTER.with(|epoch_counter| {
            let old = epoch_counter
                .get().expect("we initialized the OnceCell when we incremented the epoch counter the fist time")
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            assert!(old % 2 != 0, "Old Epoch counter value should be odd!");
        });

        RcuRef {
            data: arc.deref().into(),
            arc,
        }
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
    pub fn replace_arc(&self, new_value: Arc<T>) -> Arc<T> {
        let arc_ptr = self.active_value.swap(
            Arc::into_raw(new_value).cast_mut(),
            std::sync::atomic::Ordering::AcqRel,
        );

        // manually drop as we need to ensure not to drop the arc while
        // we have not witnessed all threads to be or have been outside the read critical section
        // i.e. even epoch counter or different odd epoch counter
        // Safety:
        // - the ptr was created in Rcu::new or Rcu::replace with Arc::into_raw
        // - the Rcu itself holds one strong count
        let arc = unsafe { ManuallyDrop::new(Arc::from_raw(arc_ptr)) };

        // Get the current state of the epoch counters,
        // we can only drop the old value once we have observed all to be even or to have changed
        let epochs = EPOCH_COUNTERS.read().unwrap().clone();
        let mut epochs = epochs
            .into_iter()
            .flat_map(|elem| {
                let arc = elem.upgrade()?;
                let init_val = arc.load(std::sync::atomic::Ordering::Acquire);
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
                arc.load(std::sync::atomic::Ordering::Acquire) == elem.0
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RcuRef")
            .field("data", &self.deref())
            .finish()
    }
}

// use associated functions rather than methods so that we don't overlap
// with functions of the Deref Target type
impl<T: ?Sized, M: ?Sized> RcuRef<T, M> {
    pub fn map<N: ?Sized, F: for<'a> FnOnce(&'a M) -> &'a N>(reference: Self, f: F) -> RcuRef<T, N> {
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
