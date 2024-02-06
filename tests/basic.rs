use std::ops::Deref;

use arcu::EpochCounter;

extern crate alloc;

#[cfg(feature = "std")]
#[test]
fn replace() {
    let rcu = arcu::Rcu::new(11);
    assert_eq!(rcu.read().deref(), &11);
    rcu.replace(55);
    assert_eq!(rcu.read().deref(), &55);
}

#[test]
fn raw_replace() {
    use alloc::sync::Arc;

    let rcu = arcu::Rcu::new(22);

    let epoch_counters = [Arc::new(EpochCounter::new())];

    let val = unsafe { rcu.raw_read(&epoch_counters[0]) };
    assert_eq!(val.deref(), &22);

    unsafe {
        rcu.raw_replace_arc(Arc::new(66), || {
            epoch_counters.iter().map(Arc::downgrade).collect()
        })
    };

    let val = unsafe { rcu.raw_read(&epoch_counters[0]) };
    assert_eq!(val.deref(), &66);
}
