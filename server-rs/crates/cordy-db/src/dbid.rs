//! Mints primary keys for rows the application inserts — port of
//! `server/pkg/dbid`.
//!
//! Every id is a UUIDv7: a 48-bit millisecond timestamp followed by random
//! bits, so consecutive inserts cluster in a narrow key range instead of
//! scattering across the primary-key B-tree.
//!
//! Rules of use (from the Go package doc):
//! - Only for identity columns of rows we insert and keep — NOT for lease /
//!   claim tokens, idempotency keys, or secrets (those stay on v4 /
//!   gen_random_uuid()).
//! - The DB-side `DEFAULT gen_random_uuid()` plus the queries'
//!   `COALESCE(sqlc.narg('id')::uuid, gen_random_uuid())` stay in place; a
//!   table holds a mix of v4/v7 ids forever.
//! - Suitable ONLY when the id is used solely as the inserted row's identity.
//!   If the id doubles as an object key / filename / correlation id, mint
//!   directly and handle errors instead.
//! - A v7 embeds creation time to millisecond precision only and is only
//!   approximately ordered across writers — never derive ordering guarantees
//!   from it.
//! - On INSERT ... ON CONFLICT the minted id wins only if the row is actually
//!   inserted; always read the id back from RETURNING.

use uuid::Uuid;

/// Returns a fresh UUIDv7 ready to assign to an insert parameter's ID field.
///
/// Go's variant deliberately has no error return: uuid.NewV7 fails only when
/// OS entropy does, and the zero pgtype.UUID plus the query's COALESCE
/// fallback let Postgres mint the id instead. Rust's [`Uuid::now_v7`] is
/// infallible, so the fallback path is unreachable here — the contract
/// (infallible call site, DB fallback preserved in SQL) carries over.
pub fn new_v7() -> Uuid {
    Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_version7_ids() {
        let id = new_v7();
        assert_eq!(id.get_version_num(), 7);
    }

    #[test]
    fn consecutive_ids_are_monotonically_ordered_within_a_writer() {
        let mut prev = new_v7();
        for _ in 0..100 {
            let next = new_v7();
            assert!(next.as_bytes() >= prev.as_bytes());
            prev = next;
        }
    }
}
