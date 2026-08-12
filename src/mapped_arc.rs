//! This module contains the [`RcuRef`] type which is a smart pointer to the content of an [`super::Rcu`]

// FIXME use ArcRef/MappedArc once stable https://github.com/rust-lang/libs-team/issues/700

use alloc::sync::Arc;
use core::{fmt::Debug, ops::Deref, ptr::NonNull};

use crate::never;

/// A smard pointer for a reference to the content of an [`super::Rcu`]
pub struct MappedArc<T>
where
    T: ?Sized,
{
    // we keep the arc to ensure its still alive, but we only access its data through data
    _arc: Arc<dyn Send + Sync>,
    data: NonNull<T>,
}

impl<T: ?Sized + Debug> Debug for MappedArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RcuRef")
            .field("data", &self.deref())
            .finish()
    }
}

impl<T: ?Sized> MappedArc<T>
where
    Arc<T>: DynArc,
{
    /// Create a new `RcuRef` from an `Arc`
    pub fn new(arc: Arc<T>) -> Self {
        let data = arc.as_ref().into();
        let arc: Arc<dyn Send + Sync + 'static> = arc.to_dyn_arc();
        Self { _arc: arc, data }
    }
}

mod seal {
    use super::*;

    pub(crate) trait Seal {}

    impl<T: Send + Sync + 'static> Seal for Arc<T> {}
    impl Seal for Arc<dyn Send + Sync + 'static> {}
}

/// A trait for abstracting over Arcs that are either Arc<dyn Send + Sync + 'static> or can be coearced to it
#[allow(private_bounds)]
pub trait DynArc: seal::Seal {
    /// Convert an Arc to Arc<dyn Send + Sync + 'static>
    fn to_dyn_arc(self) -> Arc<dyn Sync + Send + 'static>;
}

impl<T: Send + Sync + 'static> DynArc for Arc<T> {
    fn to_dyn_arc(self) -> Arc<dyn Sync + Send + 'static> {
        self
    }
}

impl DynArc for Arc<dyn Send + Sync + 'static> {
    fn to_dyn_arc(self) -> Arc<dyn Sync + Send + 'static> {
        self
    }
}

// use associated functions rather than methods so that we don't overlap
// with functions of the Deref Target type
impl<T: ?Sized> MappedArc<T> {
    /// apply the mapping function to the reference in this RcuRef
    pub fn map<N: ?Sized, F: for<'a> FnOnce(&'a T) -> &'a N>(
        reference: Self,
        f: F,
    ) -> MappedArc<N> {
        match MappedArc::try_map(reference, |data| Ok::<&N, never::Never>(f(data))) {
            Ok(result) => result,
        }
    }

    /// try to apply the failable mapping function to the reference in this RcuRef
    pub fn try_map<N: ?Sized, F: for<'a> FnOnce(&'a T) -> Result<&'a N, Err>, Err>(
        reference: Self,
        f: F,
    ) -> Result<MappedArc<N>, Err> {
        Ok(MappedArc {
            _arc: reference._arc,
            // Safety:
            // - data points into arc keeping the pointer valid
            data: f(unsafe { reference.data.as_ref() })?.into(),
        })
    }

    /// Check whether the two RcuRefs reference values in the same epoch
    pub fn same_epoch(this: &Self, other: &Self) -> bool {
        Arc::ptr_eq(&this._arc, &other._arc)
    }

    /// Compares the RcuRefs references via [`core::ptr::eq`]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        core::ptr::eq(this.data.as_ptr(), other.data.as_ptr())
    }

    /// Compares the RcuRefs references via [`core::ptr::addr_eq`]
    pub fn ptr_addr_eq(this: &Self, other: &Self) -> bool {
        std::ptr::addr_eq(this.data.as_ptr(), other.data.as_ptr())
    }

    /// Clones the RcuRef
    ///
    /// Not implementing clone to not shadow the inner types clone impl
    #[allow(clippy::should_implement_trait)]
    pub fn clone(this: &Self) -> Self {
        Self {
            _arc: Arc::clone(&this._arc),
            data: this.data,
        }
    }

    /// Get a reference to root of the RcuRef
    ///
    /// i.e. the value that was stored in the Rcu
    /// before applying any mappings
    pub fn get_root(this: &Self) -> &Arc<dyn Send + Sync + 'static> {
        &this._arc
    }
}

impl<M: ?Sized> Deref for MappedArc<M> {
    type Target = M;

    fn deref(&self) -> &Self::Target {
        // Safety: The pointer points into the arc we are holding
        // while we are alive so is the target
        // as the content is in an Rcu no mutable access is given out
        unsafe { self.data.as_ref() }
    }
}
