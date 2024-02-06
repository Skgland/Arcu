#[cfg(feature = "std")]
#[repr(transparent)]
struct RwLock<T>(std::sync::RwLock<T>);

#[cfg(feature = "std")]
impl<T> RwLock<T> {
    fn read(&self) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_,T>> {
        self.0.read()
    }
}


#[cfg(not(feature = "std"))]
pub use spin::RwLock;
