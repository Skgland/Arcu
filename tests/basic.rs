use std::{
    fmt::Debug, ops::Deref, sync::{Arc, RwLock}
};

use arcu::epoch_counters::EpochCounter;

extern crate alloc;


struct Loud<T: Debug>(T);

impl<T: Debug> Drop for Loud<T> {
    fn drop(&mut self) {
        println!("Dropping: {:?}", self.0);
    }
}

#[cfg(all(feature = "global_counters", feature = "thread_local_counter"))]
#[test]
fn std_replace() {
    let rcu = arcu::Arcu::new(Loud(11));
    assert_eq!(rcu.read().0, 11);
    rcu.replace(Loud(55));
    assert_eq!(rcu.read().0, 55);
}

#[cfg(all(feature = "global_counters", feature = "thread_local_counter"))]
#[test]
fn std_update() {
    let rcu = arcu::Arcu::new(Loud((0,0)));
    let rcu_ref = &rcu;
    assert_eq!(rcu.read().0, (0,0));

    std::thread::scope(|scope| {
        for idx in 0..100 {
            scope.spawn(move || rcu_ref.try_update(|old| Some(Arc::new(Loud((idx,old.0.1 + 1))))));
        }
    });

    assert_eq!(rcu.read().0.1, 100);
}

#[test]
fn raw_replace() {
    use alloc::sync::Arc;

    let rcu = arcu::Arcu::new(201);

    let epoch_counters: [_; 1] = std::array::from_fn(|_| Arc::new(EpochCounter::new()));

    let val = unsafe { rcu.raw_read(&epoch_counters[0]) };
    assert_eq!(val.deref(), &201);

    let epoch_counters: &_ = &epoch_counters;

    std::thread::scope(|scope| {
        for idx in 0..100 {
            let new = Arc::new(idx);
            scope.spawn(|| {
                let to_drop = unsafe {
                    rcu.raw_replace(new, || epoch_counters.iter().map(Arc::downgrade).collect())
                };
                println!("Dropping: {to_drop}");
            });
        }
    });

    let val = unsafe { rcu.raw_read(&epoch_counters[0]) };
    assert!((0..100).contains(val.deref()));
}

#[test]
fn raw_update1() {
    let rcu = arcu::Arcu::new(RwLock::new(0));

    let epoch_counters: [_; 100] =
        std::array::from_fn(|idx| (Arc::new(EpochCounter::new()), Arc::new(RwLock::new(idx))));
    let epoch_counters_ref: &_ = &epoch_counters;

    std::thread::scope(|scope| {
        for (epoch_counter, arc) in epoch_counters_ref {
            scope.spawn(|| {
                let to_drop = unsafe {
                    rcu.raw_try_update(
                        |old| {
                            let old = *old.read().unwrap();
                            println!("Old: {old}");
                            *arc.write().unwrap() = old + 1;
                            Some(arc.clone())
                        },
                        epoch_counter.deref(),
                        || {
                            epoch_counters_ref
                                .iter()
                                .map(|(counter, _)| Arc::downgrade(counter))
                                .collect()
                        },
                    )
                };
                if let Some(to_drop) = to_drop {
                    let to_drop = *to_drop.read().unwrap();
                    println!("Dropping: {to_drop}");
                }
            });
        }
    });

    let final_val = unsafe { rcu.raw_read(&epoch_counters_ref[0].0) };

    assert_eq!(final_val.read().unwrap().deref(), &epoch_counters_ref.len());

    drop(epoch_counters);
}

#[test]
fn raw_update2() {
    let rcu = arcu::Arcu::new(Arc::new(0));

    let epoch_counters: [_; 100] = std::array::from_fn(|_idx| Arc::new(EpochCounter::new()));
    let epoch_counters_ref: &_ = &epoch_counters;

    std::thread::scope(|scope| {
        for epoch_counter in epoch_counters_ref {
            scope.spawn(|| {
                let to_drop = unsafe {
                    rcu.raw_try_update(
                        |old: &usize| {
                            println!("Old: {old}");
                            Some(Arc::new(old + 1))
                        },
                        epoch_counter.deref(),
                        || epoch_counters_ref.iter().map(Arc::downgrade).collect(),
                    )
                };
                if let Some(to_drop) = to_drop {
                    println!("Dropping: {to_drop}");
                }
            });
        }
    });

    let final_val = unsafe { rcu.raw_read(&epoch_counters_ref[0]) };

    assert_eq!(final_val.deref(), &epoch_counters_ref.len());

    drop(epoch_counters);
}
