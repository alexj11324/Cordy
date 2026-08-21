//! In-process pub/sub event bus — port of `server/internal/events`.

pub mod bus;

pub use bus::{Bus, Event, Handler};
