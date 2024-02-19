use core::{fmt::Debug, ops::Deref, ptr::NonNull};

use alloc::sync::Arc;

pub struct RcuRef<T, M>
where
    T: ?Sized,
    M: ?Sized,
{
    // we keep the arc to ensure its still alive, but we only access its data through data
    #[allow(dead_code)]
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

impl<T: ?Sized> RcuRef<T, T> {
    pub fn new(arc: Arc<T>) -> Self {
        Self {
            data: arc.as_ref().into(),
            arc,
        }
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
