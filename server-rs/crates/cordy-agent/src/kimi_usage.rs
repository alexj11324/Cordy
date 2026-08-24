//! Kimi wire-log token fallback for ACP builds that omit billing counters.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::contract::TokenUsage;
use crate::stream::MAX_LINE_BYTES;

const USAGE_RECORD: &str = "usage.record";

pub(crate) struct KimiUsageScan<'a> {
    pub started_at: SystemTime,
    pub configured_home: Option<&'a str>,
    pub session_id: &'a str,
    pub resumed: bool,
    pub fallback_model: &'a str,
}

#[derive(Deserialize)]
struct WireRecord {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    time: i64,
    #[serde(default)]
    model: String,
    usage: WireUsage,
}

#[derive(Default, Deserialize)]
struct WireUsage {
    #[serde(default, rename = "inputOther")]
    input_other: i64,
    #[serde(default)]
    output: i64,
    #[serde(default, rename = "inputCacheRead")]
    cache_read: i64,
    #[serde(default, rename = "inputCacheCreation")]
    cache_write: i64,
}

pub(crate) fn scan_kimi_session_usage(scan: KimiUsageScan<'_>) -> BTreeMap<String, TokenUsage> {
    let Some(root) = session_root(scan.configured_home) else {
        return BTreeMap::new();
    };
    let mut usage = BTreeMap::new();
    let cutoff = scan
        .started_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    for path in session_wire_logs(&root, scan.session_id) {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .unwrap_or_default();
        if modified < truncate_seconds(cutoff) {
            continue;
        }
        accumulate(&mut usage, &path, &scan);
    }
    usage
}

fn truncate_seconds(duration: Duration) -> Duration {
    Duration::from_secs(duration.as_secs())
}

fn session_root(configured_home: Option<&str>) -> Option<PathBuf> {
    let root = if let Some(configured) = configured_home
        .map(str::trim)
        .filter(|home| !home.is_empty())
    {
        PathBuf::from(configured).join("sessions")
    } else if let Some(ambient) = std::env::var_os("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
    {
        ambient.join("sessions")
    } else {
        user_home()?.join(".kimi-code").join("sessions")
    };
    root.is_dir().then_some(root)
}

fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    let value = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = std::env::var_os("HOME");
    value.map(PathBuf::from)
}

fn session_wire_logs(root: &Path, session_id: &str) -> Vec<PathBuf> {
    let session_id = session_id.trim();
    if session_id.is_empty()
        || matches!(session_id, "." | "..")
        || session_id.contains('/')
        || session_id.contains('\\')
    {
        return Vec::new();
    }
    let Ok(workspaces) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for workspace in workspaces.flatten().filter(|entry| entry.path().is_dir()) {
        let agents = workspace.path().join(session_id).join("agents");
        let Ok(agent_entries) = fs::read_dir(agents) else {
            continue;
        };
        paths.extend(
            agent_entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.path().join("wire.jsonl")),
        );
    }
    paths
}

fn accumulate(usage: &mut BTreeMap<String, TokenUsage>, path: &Path, scan: &KimiUsageScan<'_>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(bytes) = read_bounded_line(&mut reader, &mut line) else {
            return;
        };
        if bytes == 0 {
            return;
        }
        if line.len() > MAX_LINE_BYTES
            || !line
                .windows(USAGE_RECORD.len())
                .any(|w| w == USAGE_RECORD.as_bytes())
        {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<WireRecord>(&line) else {
            continue;
        };
        if record.kind != USAGE_RECORD || !record_in_turn(record.time, scan) {
            continue;
        }
        let model = if record.model.is_empty() {
            scan.fallback_model
        } else {
            &record.model
        };
        let total = usage.entry(model.to_string()).or_default();
        total.input_tokens = total
            .input_tokens
            .saturating_add(record.usage.input_other.max(0));
        total.output_tokens = total
            .output_tokens
            .saturating_add(record.usage.output.max(0));
        total.cache_read_tokens = total
            .cache_read_tokens
            .saturating_add(record.usage.cache_read.max(0));
        total.cache_write_tokens = total
            .cache_write_tokens
            .saturating_add(record.usage.cache_write.max(0));
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<usize> {
    let limit = u64::try_from(MAX_LINE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let bytes = {
        let mut bounded = std::io::Read::take(reader, limit);
        bounded.read_until(b'\n', line)?
    };
    if line.len() > MAX_LINE_BYTES && !line.ends_with(b"\n") {
        discard_line_remainder(reader)?;
    }
    Ok(bytes)
}

fn discard_line_remainder<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(newline + 1);
            return Ok(());
        }
        let consumed = buffer.len();
        reader.consume(consumed);
    }
}

fn record_in_turn(record_millis: i64, scan: &KimiUsageScan<'_>) -> bool {
    if record_millis <= 0 {
        return !scan.resumed;
    }
    let started_millis = scan
        .started_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u128::try_from(record_millis).is_ok_and(|record| record >= started_millis)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn scan_is_session_scoped_model_aware_and_resume_safe() {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let started_at = SystemTime::now();
        let current = started_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        write_log(
            home.path(),
            "mine",
            "main",
            &[
                format!(
                    r#"{{"type":"usage.record","time":{},"model":"kimi-k3","usage":{{"inputOther":10,"output":2,"inputCacheRead":30,"inputCacheCreation":4}}}}"#,
                    current + 1
                ),
                format!(
                    r#"{{"type":"step.end","time":{},"model":"kimi-k3","usage":{{"inputOther":999}}}}"#,
                    current + 2
                ),
            ],
        );
        write_log(
            home.path(),
            "mine",
            "subagent",
            &[format!(
                r#"{{"type":"usage.record","time":{},"model":"other","usage":{{"inputOther":5,"output":1}}}}"#,
                current + 3
            )],
        );
        write_log(
            home.path(),
            "theirs",
            "main",
            &[format!(
                r#"{{"type":"usage.record","time":{},"usage":{{"inputOther":9999}}}}"#,
                current + 4
            )],
        );
        let usage = scan_kimi_session_usage(KimiUsageScan {
            started_at,
            configured_home: home.path().to_str(),
            session_id: "mine",
            resumed: true,
            fallback_model: "fallback",
        });
        assert_eq!(usage["kimi-k3"].input_tokens, 10);
        assert_eq!(usage["kimi-k3"].cache_read_tokens, 30);
        assert_eq!(usage["other"].output_tokens, 1);
        assert!(!usage.contains_key("fallback"));
    }

    #[test]
    fn traversal_session_id_is_rejected() {
        assert!(session_wire_logs(Path::new("/tmp"), "../other").is_empty());
        assert!(session_wire_logs(Path::new("/tmp"), "a/b").is_empty());
        assert!(session_wire_logs(Path::new("/tmp"), "a\\b").is_empty());
    }

    #[test]
    fn oversized_wire_record_is_bounded_and_following_record_survives() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 5_000];
        input.push(b'\n');
        input.extend_from_slice(b"next\n");
        let mut reader = BufReader::new(input.as_slice());
        let mut line = Vec::new();
        let bytes = read_bounded_line(&mut reader, &mut line)
            .unwrap_or_else(|error| panic!("bounded oversized line: {error}"));
        assert_eq!(bytes, MAX_LINE_BYTES + 1);
        assert_eq!(line.len(), MAX_LINE_BYTES + 1);
        line.clear();
        read_bounded_line(&mut reader, &mut line)
            .unwrap_or_else(|error| panic!("line after oversized record: {error}"));
        assert_eq!(line, b"next\n");
    }

    fn write_log(root: &Path, session: &str, agent: &str, lines: &[String]) {
        let directory = root
            .join("sessions")
            .join("workspace")
            .join(session)
            .join("agents")
            .join(agent);
        fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("create wire directory: {error}"));
        let mut file = File::create(directory.join("wire.jsonl"))
            .unwrap_or_else(|error| panic!("create wire log: {error}"));
        for line in lines {
            writeln!(file, "{line}").unwrap_or_else(|error| panic!("write wire log: {error}"));
        }
    }
}
