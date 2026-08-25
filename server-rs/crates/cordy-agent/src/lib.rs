//! Provider-neutral execution contracts and launch safety for local agent
//! runtimes.
//!
//! This crate is the Rust counterpart of `server/pkg/agent`. Provider families
//! become constructible only when their real transport implementation lands;
//! metadata alone never manufactures a backend that cannot execute.

pub mod acp;
pub mod acp_mcp;
pub mod antigravity;
pub mod codebuddy;
pub mod codebuddy_discovery;
pub mod command;
pub mod contract;
mod kimi_usage;
pub mod mcp;
pub mod model;
pub mod process;
pub mod qoder;
pub mod qwen;
pub mod registry;
pub mod stderr;
pub mod stream;
pub mod version;

pub use acp::{AcpClient, AcpError, AcpNotification, AcpPermissionDecision};
pub use acp_mcp::{
    build_acp_mcp_servers, filter_acp_mcp_servers, parse_acp_mcp_capabilities, AcpMcpCapabilities,
    AcpMcpCapabilityDeclaration, AcpMcpServer,
};
pub use antigravity::{AntigravityBackend, AntigravityConfig};
pub use codebuddy::{CodebuddyBackend, CodebuddyConfig};
pub use command::{BlockedArgMode, FilteredArgs, RuntimeCommand};
pub use contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
pub use model::{
    parse_acp_session_models, Catalog, CatalogCache, Model, ModelDiscoveryCacheKey,
    ModelServiceTier, ModelThinking, ThinkingLevel,
};
pub use process::{OwnedProcessTree, ProcessTreeSignal};
pub use qoder::{
    DimBackend, DimConfig, GrokBackend, GrokConfig, HermesBackend, HermesConfig, KimiBackend,
    KimiConfig, KiroBackend, KiroConfig, McodeBackend, McodeConfig, QoderBackend, QoderConfig,
    QwenpawBackend, QwenpawConfig, ReasonixBackend, ReasonixConfig, TraecliBackend, TraecliConfig,
};
pub use qwen::{QwenBackend, QwenConfig};
pub use registry::{build_backend, BackendConfig};
pub use registry::{builtin_runtime, protocol_family, provider};
pub use version::{check_provider_minimum, extract_version_line};
