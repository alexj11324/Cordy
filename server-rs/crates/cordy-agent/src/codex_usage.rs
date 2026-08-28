//! Bounded Codex session-rollout usage fallback.
//!
//! Codex app-server versions do not all return usage in the turn response.
//! This reader is deliberately limited to rollout files owned by one thread;
//! it never scans arbitrary JSONL or assigns a concurrent thread's usage to the
//! current task.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::contract::TokenUsage;

const MAX_METADATA_LINES: usize = 64;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_DIRECTORY_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexSessionUsage {
    pub(crate) usage: TokenUsage,
    pub(crate) model: String,
}

#[derive(Debug, Clone)]
struct RolloutCandidate {
    path: PathBuf,
    modified: SystemTime,
}

#[derive(Debug, Clone, Copy, Default)]
struct RawUsage {
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    reasoning_output_tokens: i64,
}

/// Scans the newest rollout for `thread_id` after `started_at`.
pub(crate) fn scan_codex_session_usage(
    configured_home: Option<&str>,
    thread_id: &str,
    started_at: SystemTime,
    resumed: bool,
) -> Option<CodexSessionUsage> {
    let root = codex_session_root(configured_home)?;
    let candidates = find_rollouts(&root, thread_id);
    let candidate = candidates
        .into_iter()
        .filter(|candidate| candidate.modified >= started_at)
        .max_by(|left, right| {
            left.modified
                .cmp(&right.modified)
                .then_with(|| left.path.cmp(&right.path))
        })?;
    parse_rollout(&candidate.path, started_at, resumed)
}

fn codex_session_root(configured_home: Option<&str>) -> Option<PathBuf> {
    let configured_home = configured_home
        .map(str::trim)
        .filter(|home| !home.is_empty());
    if let Some(home) = configured_home {
        let root = Path::new(home).join("sessions");
        if root.is_dir() {
            return Some(root);
        }
    } else if let Ok(home) = std::env::var("CODEX_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            let root = Path::new(home).join("sessions");
            if root.is_dir() {
                return Some(root);
            }
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| home.join(".codex").join("sessions"))
        .filter(|root| root.is_dir())
}

fn find_rollouts(root: &Path, thread_id: &str) -> Vec<RolloutCandidate> {
    if thread_id.trim().is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    collect_rollouts(root, 0, &mut candidates);

    let suffix = format!("-{thread_id}.jsonl");
    let named: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
        .filter(|candidate| rollout_matches_thread(&candidate.path, thread_id, true))
        .cloned()
        .collect();
    if !named.is_empty() {
        return named;
    }
    candidates
        .into_iter()
        .filter(|candidate| rollout_matches_thread(&candidate.path, thread_id, false))
        .collect()
}

fn collect_rollouts(root: &Path, depth: usize, output: &mut Vec<RolloutCandidate>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            output.push(RolloutCandidate {
                path,
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            });
        } else if metadata.is_dir() && depth < MAX_DIRECTORY_DEPTH {
            collect_rollouts(&path, depth + 1, output);
        }
    }
}

fn rollout_matches_thread(path: &Path, thread_id: &str, filename_is_evidence: bool) -> bool {
    match read_rollout_thread_id(path) {
        Ok(Some(owner)) => owner == thread_id,
        Ok(None) => filename_is_evidence,
        Err(_) => false,
    }
}

fn read_rollout_thread_id(path: &Path) -> io::Result<Option<String>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    for _ in 0..MAX_METADATA_LINES {
        let Some(valid) = read_bounded_line(&mut reader, &mut line)? else {
            break;
        };
        if !valid {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        if let Some(owner) = value
            .get("payload")
            .and_then(|payload| payload.get("id"))
            .and_then(Value::as_str)
            .filter(|owner| !owner.is_empty())
        {
            return Ok(Some(owner.to_string()));
        }
    }
    Ok(None)
}

fn parse_rollout(path: &Path, started_at: SystemTime, resumed: bool) -> Option<CodexSessionUsage> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let started_at = system_time_as_utc(started_at);
    let mut line = Vec::new();
    let mut previous_total = None;
    let mut accumulated = RawUsage::default();
    let mut final_usage = None;
    let mut model = String::new();
    let mut after_start_boundary = false;

    loop {
        let valid = match read_bounded_line(&mut reader, &mut line) {
            Ok(Some(valid)) => valid,
            Ok(None) | Err(_) => break,
        };
        if !valid {
            continue;
        }
        if !line
            .windows(b"token_count".len())
            .any(|window| window == &b"token_count"[..])
            && !line
                .windows(b"turn_context".len())
                .any(|window| window == &b"turn_context"[..])
        {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let timestamp_after_start = event_after_start(&value, started_at);
        if timestamp_after_start {
            after_start_boundary = true;
        }

        if value.get("type").and_then(Value::as_str) == Some("turn_context") {
            if let Some(value) = value
                .get("payload")
                .and_then(|payload| payload.get("model"))
                .and_then(Value::as_str)
            {
                model = value.to_string();
            }
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("response_item")
            && value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                != Some("token_count")
        {
            continue;
        }
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info").and_then(Value::as_object) else {
            continue;
        };
        if let Some(value) = info.get("model").and_then(Value::as_str) {
            model = value.to_string();
        }
        let after_start = started_at.is_none()
            || timestamp_after_start
            || (value.get("timestamp").is_none() && (!resumed || after_start_boundary));
        if let Some(total) = info.get("total_token_usage").and_then(parse_raw_usage) {
            if after_start {
                let delta = previous_total.map_or(total, |previous| subtract(total, previous));
                accumulated = add(accumulated, delta);
                final_usage = Some(accumulated);
            }
            previous_total = Some(total);
        } else if after_start {
            final_usage = info.get("last_token_usage").and_then(parse_raw_usage);
        }
    }

    let raw = final_usage?;
    let usage = TokenUsage {
        input_tokens: raw
            .input_tokens
            .saturating_sub(raw.cached_input_tokens)
            .max(0),
        output_tokens: raw
            .output_tokens
            .saturating_add(raw.reasoning_output_tokens),
        cache_read_tokens: raw.cached_input_tokens,
        ..TokenUsage::default()
    };
    (usage != TokenUsage::default()).then_some(CodexSessionUsage { usage, model })
}

fn event_after_start(value: &Value, started_at: Option<DateTime<Utc>>) -> bool {
    let (Some(started_at), Some(timestamp)) =
        (started_at, value.get("timestamp").and_then(Value::as_str))
    else {
        return false;
    };
    DateTime::parse_from_rfc3339(timestamp)
        .map(|value| value.with_timezone(&Utc) > started_at)
        .unwrap_or(false)
}

fn system_time_as_utc(value: SystemTime) -> Option<DateTime<Utc>> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    DateTime::<Utc>::from_timestamp(duration.as_secs().try_into().ok()?, duration.subsec_nanos())
}

fn parse_raw_usage(value: &Value) -> Option<RawUsage> {
    let object = value.as_object()?;
    let cached_input_tokens = number(object, &["cached_input_tokens"]);
    Some(RawUsage {
        input_tokens: number(object, &["input_tokens"]),
        output_tokens: number(object, &["output_tokens"]),
        cached_input_tokens: if cached_input_tokens == 0 {
            number(object, &["cache_read_input_tokens"])
        } else {
            cached_input_tokens
        },
        reasoning_output_tokens: number(object, &["reasoning_output_tokens"]),
    })
}

fn number(object: &serde_json::Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| {
            let value = object.get(*key)?;
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| value.try_into().ok()))
                .or_else(|| {
                    value
                        .as_f64()
                        .filter(|value| value.is_finite())
                        .map(|value| value.max(0.0) as i64)
                })
        })
        .unwrap_or_default()
}

fn subtract(total: RawUsage, baseline: RawUsage) -> RawUsage {
    RawUsage {
        input_tokens: non_negative_delta(total.input_tokens, baseline.input_tokens),
        output_tokens: non_negative_delta(total.output_tokens, baseline.output_tokens),
        cached_input_tokens: non_negative_delta(
            total.cached_input_tokens,
            baseline.cached_input_tokens,
        ),
        reasoning_output_tokens: non_negative_delta(
            total.reasoning_output_tokens,
            baseline.reasoning_output_tokens,
        ),
    }
}

fn non_negative_delta(total: i64, baseline: i64) -> i64 {
    if total < baseline {
        total
    } else {
        total.saturating_sub(baseline)
    }
}

fn add(left: RawUsage, right: RawUsage) -> RawUsage {
    RawUsage {
        input_tokens: left.input_tokens.saturating_add(right.input_tokens),
        output_tokens: left.output_tokens.saturating_add(right.output_tokens),
        cached_input_tokens: left
            .cached_input_tokens
            .saturating_add(right.cached_input_tokens),
        reasoning_output_tokens: left
            .reasoning_output_tokens
            .saturating_add(right.reasoning_output_tokens),
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<Option<bool>> {
    line.clear();
    let mut oversized = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if line.is_empty() && !oversized {
                Ok(None)
            } else {
                Ok(Some(!oversized))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(buffer.len(), |index| index + 1);
        if !oversized {
            if line.len().saturating_add(count) > MAX_LINE_BYTES {
                oversized = true;
            } else {
                line.extend_from_slice(&buffer[..count]);
            }
        }
        reader.consume(count);
        if newline.is_some() {
            return Ok(Some(!oversized));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn scans_only_owned_thread_and_subtracts_resume_baseline() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create session directory: {error}"));
        let sessions = directory.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap_or_else(|error| panic!("create sessions: {error}"));
        let path = sessions.join("rollout-2026-08-27-thread-1.jsonl");
        fs::write(
            &path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n"
                .to_string()
                + "{\"type\":\"token_count\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":20},\"model\":\"gpt-test\"}}}\n"
                + "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"type\":\"token_count\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":130,\"output_tokens\":16,\"cached_input_tokens\":30}}}}\n",
        )
        .unwrap_or_else(|error| panic!("write rollout: {error}"));
        let started = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(UNIX_EPOCH);
        let result =
            parse_rollout(&path, started, true).unwrap_or_else(|| panic!("expected rollout usage"));
        assert_eq!(result.model, "gpt-test");
        assert_eq!(result.usage.input_tokens, 30);
        assert_eq!(result.usage.output_tokens, 6);
        assert_eq!(result.usage.cache_read_tokens, 10);
    }

    #[test]
    fn malformed_or_unowned_rollouts_are_ignored() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create session directory: {error}"));
        let path = directory.path().join("rollout-other.jsonl");
        fs::write(
            &path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"other\"}}\n",
        )
        .unwrap_or_else(|error| panic!("write rollout: {error}"));
        assert!(!rollout_matches_thread(&path, "thread-1", true));
        assert!(parse_rollout(&path, UNIX_EPOCH, false).is_none());
    }

    #[test]
    fn preserves_resume_delta_and_counter_reset_contract() {
        let started = UNIX_EPOCH + Duration::from_secs(10);
        let cases = [
            (
                "last usage wins",
                "{\"timestamp\":\"1970-01-01T00:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10}}}}\n{\"timestamp\":\"1970-01-01T00:00:11Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":160,\"output_tokens\":20}}}}\n{\"timestamp\":\"1970-01-01T00:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":7,\"output_tokens\":3}}}}",
                TokenUsage {
                    input_tokens: 7,
                    output_tokens: 3,
                    ..TokenUsage::default()
                },
            ),
            (
                "cache alias",
                "{\"timestamp\":\"1970-01-01T00:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cache_read_input_tokens\":700}}}}\n{\"timestamp\":\"1970-01-01T00:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1800,\"cached_input_tokens\":1400}}}}",
                TokenUsage {
                    input_tokens: 100,
                    cache_read_tokens: 700,
                    ..TokenUsage::default()
                },
            ),
            (
                "counter reset",
                "{\"timestamp\":\"1970-01-01T00:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":50}}}}\n{\"timestamp\":\"1970-01-01T00:00:11Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":120,\"output_tokens\":60}}}}\n{\"timestamp\":\"1970-01-01T00:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"output_tokens\":5}}}}\n{\"timestamp\":\"1970-01-01T00:00:13Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":150,\"output_tokens\":70}}}}",
                TokenUsage {
                    input_tokens: 170,
                    output_tokens: 80,
                    ..TokenUsage::default()
                },
            ),
            (
                "missing timestamp establishes boundary",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10}}}}\n{\"timestamp\":\"1970-01-01T00:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":160,\"output_tokens\":25}}}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"output_tokens\":30}}}}",
                TokenUsage {
                    input_tokens: 80,
                    output_tokens: 20,
                    ..TokenUsage::default()
                },
            ),
        ];

        for (name, content, want) in cases {
            let directory = tempfile::tempdir()
                .unwrap_or_else(|error| panic!("create rollout directory for {name}: {error}"));
            let path = directory.path().join("rollout-test.jsonl");
            fs::write(&path, content)
                .unwrap_or_else(|error| panic!("write rollout fixture for {name}: {error}"));
            let got = parse_rollout(&path, started, true)
                .unwrap_or_else(|| panic!("expected usage for {name}"));
            assert_eq!(got.usage, want, "case: {name}");
        }
    }

    #[test]
    fn scans_explicit_home_and_requires_metadata_ownership() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create Codex home: {error}"));
        let date_directory = directory.path().join("sessions/2026/08/27");
        fs::create_dir_all(&date_directory)
            .unwrap_or_else(|error| panic!("create date directory: {error}"));
        let started = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(UNIX_EPOCH);
        fs::write(
            date_directory.join("rollout-future-name.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":60}}}}\n",
        )
        .unwrap_or_else(|error| panic!("write owned rollout: {error}"));
        fs::write(
            date_directory.join("rollout-2026-08-27-thread-1.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"other-thread\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":900}}}}\n",
        )
        .unwrap_or_else(|error| panic!("write foreign rollout: {error}"));

        let home = directory
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("temporary home is not UTF-8"));
        let result = scan_codex_session_usage(Some(home), "thread-1", started, false)
            .unwrap_or_else(|| panic!("expected owned rollout usage"));
        assert_eq!(result.usage.input_tokens, 60);
    }
}
