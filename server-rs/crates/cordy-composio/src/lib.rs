//! Composio integration (port of server/internal/integrations/composio +
//! the server/pkg/composio SDK): signed-state connect handshake, local
//! connection mirror, idempotent disconnect, MCP session helper, and the
//! per-task MCP overlay builder.

pub mod dispatch;
pub mod sdk;
pub mod service;
pub mod state;

pub use dispatch::{
    build_task_overlay, lower_trim, normalise_allowlist_to_set, pin_connected_accounts,
    ComposioMcpServer, ConnectedApp, McpOverlayPayload, OverlayResult, SessionSpawner,
    MCP_OVERLAY_SERVER_NAME,
};
pub use sdk::{
    parse_api_error, verify_webhook, ApiError, AuthConfig, AuthConfigRef, Client, ClientBuilder,
    ConnectedAccount, CreateLinkRequest, CreateLinkResponse, CreateSessionRequest,
    CreateSessionResponse, Error as SdkError, ExecuteToolRequest, ExecuteToolResponse,
    ListAuthConfigsRequest, ListAuthConfigsResponse, ListConnectedAccountsRequest,
    ListConnectedAccountsResponse, ListToolkitsRequest, ListToolkitsResponse, ManageConnections,
    McpDescriptor, SessionWarning, Toolkit, DEFAULT_BASE_URL, DEFAULT_WEBHOOK_TOLERANCE,
};
pub use service::{
    better_auth_config, Sdk, Service, ServiceConfig, ServiceError, Store, ToolkitView,
    UpsertConnectionParams, CALLBACK_PATH,
};
pub use state::{sign_payload, sign_state, verify_state, StateClaims, StateError};
