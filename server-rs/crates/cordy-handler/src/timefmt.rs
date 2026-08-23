//! RFC3339 formatting helpers matching the Go handler wire contract
//! (timestampToString → RFC3339 seconds; timestampToNanoPtr → RFC3339Nano).

/// Go `timestampToString` / `timestampToPtr`: UTC, second precision.
pub fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Go time.RFC3339Nano: fractional seconds without trailing zeros. chrono's
/// AutoSi rounds to 3/6/9 digits rather than trimming fully — invisible to
/// ISO-8601 parsers on both clients (same note as cordy-service).
pub fn rfc3339_nano(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_uses_second_precision_utc() {
        let t = chrono::DateTime::parse_from_rfc3339("2026-08-22T12:34:56.789Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(rfc3339(t), "2026-08-22T12:34:56Z");
    }
}
