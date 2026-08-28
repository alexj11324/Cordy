//! Re-export of `patchbay-task-failure`,
//! extracted to its own crate so `patchbay-metrics` can depend on it without a
//! dependency cycle through `patchbay-service`.

pub use patchbay_task_failure::*;
