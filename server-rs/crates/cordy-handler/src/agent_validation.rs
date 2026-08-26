//! Agent configuration validation shared by create and update handlers.
//!
//! Port of `server/internal/agentconfig/concurrency.go` and
//! `server/internal/handler/agent_validation.go`. Keeping the bounds in one
//! handler-owned contract prevents create and update from drifting apart.

pub(crate) const DEFAULT_MAX_CONCURRENT_TASKS: i32 = 6;
pub(crate) const MIN_MAX_CONCURRENT_TASKS: i32 = 1;
pub(crate) const MAX_MAX_CONCURRENT_TASKS: i32 = 50;

pub(crate) fn is_valid_max_concurrent_tasks(value: i32) -> bool {
    (MIN_MAX_CONCURRENT_TASKS..=MAX_MAX_CONCURRENT_TASKS).contains(&value)
}

pub(crate) fn validate_max_concurrent_tasks(value: i32) -> Result<(), String> {
    if is_valid_max_concurrent_tasks(value) {
        Ok(())
    } else {
        Err(format!(
            "max_concurrent_tasks must be between {MIN_MAX_CONCURRENT_TASKS} and {MAX_MAX_CONCURRENT_TASKS}"
        ))
    }
}

pub(crate) fn default_and_validate_max_concurrent_tasks(value: Option<i32>) -> Result<i32, String> {
    let value = value.unwrap_or(DEFAULT_MAX_CONCURRENT_TASKS);
    validate_max_concurrent_tasks(value).map(|()| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_shared_inclusive_bounds() {
        assert!(is_valid_max_concurrent_tasks(MIN_MAX_CONCURRENT_TASKS));
        assert!(is_valid_max_concurrent_tasks(MAX_MAX_CONCURRENT_TASKS));
        assert!(!is_valid_max_concurrent_tasks(MIN_MAX_CONCURRENT_TASKS - 1));
        assert!(!is_valid_max_concurrent_tasks(MAX_MAX_CONCURRENT_TASKS + 1));
    }

    #[test]
    fn create_defaults_missing_or_null_to_six() {
        assert_eq!(
            default_and_validate_max_concurrent_tasks(None).unwrap(),
            DEFAULT_MAX_CONCURRENT_TASKS
        );
    }

    #[test]
    fn invalid_values_keep_the_handler_error_shape() {
        assert_eq!(
            validate_max_concurrent_tasks(0).unwrap_err(),
            "max_concurrent_tasks must be between 1 and 50"
        );
    }
}
