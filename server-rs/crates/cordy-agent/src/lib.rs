//! Provider-neutral execution contracts and launch safety for local agent
//! runtimes.
//!
//! This crate is the Rust counterpart of `server/pkg/agent`. Provider families
//! become constructible only when their real transport implementation lands;
//! metadata alone never manufactures a backend that cannot execute.

pub mod antigravity;
pub mod codebuddy;
pub mod codebuddy_discovery;
pub mod command;
pub mod contract;
pub mod deveco;
pub mod dsh;
pub mod mcp;
pub mod model;
pub mod opencode;
pub mod opencode_mcp;
pub mod pi;
pub mod process;
pub mod qwen;
pub mod registry;
pub mod stderr;
pub mod stream;
pub mod version;

pub use antigravity::{AntigravityBackend, AntigravityConfig};
pub use codebuddy::{CodebuddyBackend, CodebuddyConfig};
pub use command::{BlockedArgMode, FilteredArgs, RuntimeCommand};
pub use contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
pub use deveco::{build_deveco_args, DevecoBackend, DevecoConfig};
pub use dsh::{build_dsh_args, DshBackend, DshConfig};
pub use model::{
    Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelServiceTier, ModelThinking,
    ThinkingLevel,
};
pub use opencode::{build_opencode_args, OpencodeBackend, OpencodeConfig};
pub use pi::{build_pi_args, PiBackend, PiConfig};
pub use process::{OwnedProcessTree, ProcessTreeSignal};
pub use qwen::{QwenBackend, QwenConfig};
