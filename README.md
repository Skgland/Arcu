# Arcu

An Arc based Rcu implementation originally implementated in [mthom/scryer-prolog#1980](https://github.com/mthom/scryer-prolog/pull/1980)

| A | r | c | u |
| - | - | - | - |
| A | r | c |   |
|   | R | c | u |

The atomics based version performs lock-free[^1] reads.
By using Arc we keep the **r**ead-**c**ritical-**s**ection short; free of user defined code; and
automatically perform cleanup when no reference remains.

To coordinate reads and writes [EpochCounter]s from an [EpochCounterPool] are used.
Each read uses an `EpochCounter` from the `EpochCounterPool` of the `Arcu`, incrementing it once before entering the RCS and once more on leaving the RCS.
Each write checks against all `EpochCounter`s in the pool, blocking until it is safe to decrement the strong count of the `Arc` that was replaced by the write.

[^1]: when using thread local epoch counter with the global epoch counter pool, the initial read may block while adding the threads epoch counter to the pool