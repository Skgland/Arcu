pub(crate) type Never = <fn() -> ! as FnOutput>::Output;

pub(crate) trait FnOutput {
    type Output;
}

impl<R> FnOutput for fn() -> R {
    type Output = R;
}
