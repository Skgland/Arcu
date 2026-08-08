//! Workaround ! being unstable
//! only used internally to guide type inference

pub(crate) type Never = <fn() -> ! as FnOutput>::Output;

pub(crate) trait FnOutput {
    type Output;
}

impl<R> FnOutput for fn() -> R {
    type Output = R;
}
