//! Provider-neutral execution contracts and launch safety for local agent
//! runtimes.
//!
//! This crate is the Rust counterpart of the shared half of
//! `server/pkg/agent`. Provider protocol adapters are deliberately absent: a
//! provider becomes constructible only when its real transport implementation
//! lands, so the registry can never manufacture a backend that cannot execute.

pub mod command;
pub mod contract;
pub mod mcp;
pub mod model;
pub mod process;
pub mod registry;
pub mod stderr;
pub mod stream;
pub mod version;

pub use command::{BlockedArgMode, FilteredArgs, RuntimeCommand};
pub use contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
pub use model::{Catalog, Model, ModelServiceTier, ModelThinking, ThinkingLevel};
pub use process::{OwnedProcessTree, ProcessTreeSignal};
