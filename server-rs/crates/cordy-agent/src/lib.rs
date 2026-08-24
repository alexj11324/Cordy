//! Provider-neutral execution contracts and launch safety for local agent
//! runtimes.
//!
//! This crate is the Rust counterpart of `server/pkg/agent`. Provider families
//! become constructible only when their real transport implementation lands;
//! metadata alone never manufactures a backend that cannot execute.

pub mod command;
pub mod contract;
pub mod mcp;
pub mod model;
pub mod process;
pub mod qwen;
pub mod registry;
pub mod stderr;
pub mod stream;
pub mod version;

pub use command::{BlockedArgMode, FilteredArgs, RuntimeCommand};
pub use contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
pub use model::{
    Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelServiceTier, ModelThinking,
    ThinkingLevel,
};
pub use process::{OwnedProcessTree, ProcessTreeSignal};
pub use qwen::{QwenBackend, QwenConfig};
