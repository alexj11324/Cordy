//! Provider-neutral execution contracts and launch safety for local agent
//! runtimes.
//!
//! Provider families become constructible only when their real transport
//! implementation exists;
//! metadata alone never manufactures a backend that cannot execute.

pub mod acp;
pub mod acp_mcp;
pub mod antigravity;
pub mod claude;
pub mod codebuddy;
pub mod codebuddy_discovery;
pub mod codex;
pub(crate) mod codex_usage;
pub mod command;
pub mod contract;
pub mod copilot;
pub mod cursor;
pub mod deveco;
pub mod dsh;
mod env;
mod kimi_usage;
pub mod mcp;
pub mod model;
pub mod openclaw;
pub mod opencode;
pub mod opencode_mcp;
pub mod pi;
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
pub use claude::{build_claude_args, ClaudeBackend, ClaudeConfig};
pub use codebuddy::{CodebuddyBackend, CodebuddyConfig};
pub use codex::{build_codex_args, CodexBackend, CodexConfig};
pub use command::{BlockedArgMode, FilteredArgs, RuntimeCommand};
pub use contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
pub use copilot::{build_copilot_args, CopilotBackend, CopilotConfig};
pub use cursor::{build_cursor_args, CursorBackend, CursorConfig};
pub use deveco::{build_deveco_args, DevecoBackend, DevecoConfig};
pub use dsh::{build_dsh_args, DshBackend, DshConfig};
pub use model::{
    parse_acp_session_models, Catalog, CatalogCache, Model, ModelDiscoveryCacheKey,
    ModelServiceTier, ModelThinking, ThinkingLevel,
};
pub use openclaw::{build_openclaw_args, OpenclawBackend, OpenclawConfig};
pub use opencode::{build_opencode_args, OpencodeBackend, OpencodeConfig};
pub use pi::{build_pi_args, PiBackend, PiConfig};
pub use process::{OwnedProcessTree, ProcessTreeSignal};
pub use qoder::{
    DimBackend, DimConfig, GrokBackend, GrokConfig, HermesBackend, HermesConfig, KimiBackend,
    KimiConfig, KiroBackend, KiroConfig, McodeBackend, McodeConfig, QoderBackend, QoderConfig,
    QwenpawBackend, QwenpawConfig, ReasonixBackend, ReasonixConfig, TraecliBackend, TraecliConfig,
};
pub use qwen::{QwenBackend, QwenConfig};
pub use registry::{build_backend, BackendConfig};
pub use registry::{builtin_runtime, filter_launch_prefix_for_provider, protocol_family, provider};
pub use version::{check_provider_minimum, extract_version_line};
