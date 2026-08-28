//! In-process pub/sub event bus.

pub mod bus;

pub use bus::{Bus, Event, Handler};
