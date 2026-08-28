//! Shared agent concurrency validation.
//!
//! The API handler and CLI must agree on this contract. Keeping the bounds in
//! the configuration crate prevents one entry point from silently accepting a
//! value that another entry point rejects.

use std::fmt;

pub const DEFAULT_MAX_CONCURRENT_TASKS: i32 = 6;
pub const MIN_MAX_CONCURRENT_TASKS: i32 = 1;
pub const MAX_MAX_CONCURRENT_TASKS: i32 = 50;

pub const fn is_valid_max_concurrent_tasks(value: i32) -> bool {
    value >= MIN_MAX_CONCURRENT_TASKS && value <= MAX_MAX_CONCURRENT_TASKS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMaxConcurrentTasks;

impl fmt::Display for InvalidMaxConcurrentTasks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "must be between {MIN_MAX_CONCURRENT_TASKS} and {MAX_MAX_CONCURRENT_TASKS}"
        )
    }
}

impl std::error::Error for InvalidMaxConcurrentTasks {}

pub const fn validate_max_concurrent_tasks(value: i32) -> Result<(), InvalidMaxConcurrentTasks> {
    if is_valid_max_concurrent_tasks(value) {
        Ok(())
    } else {
        Err(InvalidMaxConcurrentTasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_inclusive_contract_bounds() {
        assert!(is_valid_max_concurrent_tasks(MIN_MAX_CONCURRENT_TASKS));
        assert!(is_valid_max_concurrent_tasks(MAX_MAX_CONCURRENT_TASKS));
        assert!(validate_max_concurrent_tasks(DEFAULT_MAX_CONCURRENT_TASKS).is_ok());
    }

    #[test]
    fn rejects_values_outside_the_contract() {
        for value in [MIN_MAX_CONCURRENT_TASKS - 1, MAX_MAX_CONCURRENT_TASKS + 1] {
            let error = validate_max_concurrent_tasks(value).unwrap_err();
            assert_eq!(error.to_string(), "must be between 1 and 50");
        }
    }
}
