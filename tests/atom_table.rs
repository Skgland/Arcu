#![cfg(feature = "thread_local_counter")]

use std::sync::Arc;

use arcu::{
    Rcu, epoch_counters::GlobalEpochCounterPool, rwlock::RwLockArcu,
    strong_atomic::StrongAtomicArcu,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Atom {
    index: usize,
}
struct AtomTable<Arcu> {
    table: Arcu,
}

impl<Arcu: Rcu<Item = Vec<&'static str>, Pool = GlobalEpochCounterPool>> AtomTable<Arcu> {
    fn build_with(&self, atom: &'static str) -> Atom {
        let table = self.table.read();

        if let Some(atom) = find_atom_in_table(&table, atom) {
            return atom;
        }

        let mut index = 0;

        if let Err(atom) = self.table.try_update(|table| {
            if let Some(atom) = find_atom_in_table(table, atom) {
                return Err(atom);
            }

            let mut new_table = Vec::with_capacity(table.len() + 1);
            index = table.len();
            new_table.extend(table);
            new_table.push(atom);
            Ok(Arc::new(new_table))
        }) {
            return atom;
        };

        Atom { index }
    }
}

fn find_atom_in_table(table: &[&str], atom: &str) -> Option<Atom> {
    if let Some(pos) = (*table).iter().position(|&item| item == atom) {
        return Some(Atom { index: pos });
    }
    None
}

#[test]
fn simulate_parallel_machine_atomic() {
    simulate_parallel_machine::<StrongAtomicArcu<_, _>>();
}

#[test]
fn simulate_parallel_machine_rwlock() {
    simulate_parallel_machine::<RwLockArcu<_, _>>();
}

fn simulate_parallel_machine<
    Arcu: Rcu<Item = Vec<&'static str>, Pool = GlobalEpochCounterPool> + Sync,
>() {
    const EXAMPLES: &[&str] = &[".", "|", "halt", "module", "library", ":", ""];

    let mut storage: [Option<Atom>; _] = [None; 100];
    let table = AtomTable {
        table: Arcu::new(vec![], GlobalEpochCounterPool),
    };

    std::thread::scope(|scope| {
        for (idx, store) in storage.each_mut().into_iter().enumerate() {
            let table = &table;
            scope.spawn(move || {
                let atom = EXAMPLES[idx % EXAMPLES.len()];
                let init_val = table.build_with(atom);
                *store = Some(init_val);

                for _ in 0..10 {
                    let val = table.build_with(atom);
                    assert_eq!(init_val, val);
                }
            });
        }
    });

    storage
        .iter()
        .zip(storage.iter().skip(EXAMPLES.len()))
        .for_each(|(a, b)| {
            assert!(a.is_some());
            assert_eq!(a, b)
        });
}
