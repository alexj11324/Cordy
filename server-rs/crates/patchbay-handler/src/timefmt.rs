//! RFC3339 formatting helper matching the Go handler seconds wire contract.
//! RFC3339Nano output is shared through `patchbay_util::rfc3339_nano`.

/// Go `timestampToString` / `timestampToPtr`: UTC, second precision.
pub fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
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
