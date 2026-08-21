//! Re-export of `cordy-task-failure` — port of `server/pkg/taskfailure`,
//! extracted to its own crate so `cordy-metrics` can depend on it without a
//! dependency cycle through `cordy-service`.

pub use cordy_task_failure::*;
