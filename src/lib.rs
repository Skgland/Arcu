#![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]

//! Arc based Rcu implementation based on my implementation in [mthom/scryer-prolog#1980](https://github.com/mthom/scryer-prolog/pull/1980)
//!
//! ```text
//! Arc
//!  Rcu
//! Arcu
//! ```

pub mod epoch_counters;
pub mod arcu;

pub use arcu::Arcu;
