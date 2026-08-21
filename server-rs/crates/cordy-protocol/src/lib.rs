//! WebSocket wire vocabulary shared by server, web clients, and the daemon —
//! port of `server/pkg/protocol`.
//!
//! Wire stability is a hard constraint: these strings and JSON field names
//! are consumed by web/desktop/mobile clients and installed daemons that
//! upgrade on their own cadence. Renaming a value is a breaking change.

pub mod events;
pub mod messages;

pub use events::*;
pub use messages::*;
