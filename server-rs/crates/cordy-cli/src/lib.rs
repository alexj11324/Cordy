//! Cordy CLI — incremental Rust replacement for `server/cmd/cordy`.
//!
//! The S10 migration deliberately registers only fully functional commands.
//! Shared configuration, API, error, and safe text-input behavior is ported
//! with each vertical slice rather than exposing placeholder command trees.
mod agent_commands;
mod agent_helpers;
mod api;
mod cli_command_schema;
#[cfg(test)]
mod root_command_tests;
#[cfg(test)]
mod daemon_command_tests;
#[cfg(test)]
mod disk_usage_command_tests;
#[cfg(test)]
mod setup_command_tests;
#[cfg(test)]
mod login_command_tests;
#[cfg(test)]
mod runtime_command_tests;
#[cfg(test)]
mod agent_command_tests;
#[cfg(test)]
mod skill_command_tests;
#[cfg(test)]
mod autopilot_command_tests;
#[cfg(test)]
mod workspace_command_tests;
#[cfg(test)]
mod squad_command_tests;
#[cfg(test)]
mod property_command_tests;
#[cfg(test)]
mod issue_search_command_tests;
#[cfg(test)]
mod issue_subscriber_command_tests;
#[cfg(test)]
mod issue_label_command_tests;
#[cfg(test)]
mod issue_metadata_command_tests;
#[cfg(test)]
mod issue_timeline_command_tests;
#[cfg(test)]
mod chat_command_tests;
#[cfg(test)]
mod repo_command_tests;
#[cfg(test)]
mod attachment_command_tests;
#[cfg(test)]
mod project_command_tests;
#[cfg(test)]
mod project_resource_command_tests;
#[cfg(test)]
mod config_command_tests;
#[cfg(test)]
mod auth_command_tests;
#[cfg(test)]
mod user_profile_command_tests;
#[cfg(test)]
mod label_command_tests;
#[cfg(test)]
mod issue_list_command_tests;
#[cfg(test)]
mod issue_get_command_tests;
#[cfg(test)]
mod issue_pull_requests_command_tests;
#[cfg(test)]
mod issue_pull_request_attach_command_tests;
#[cfg(test)]
mod issue_children_command_tests;
#[cfg(test)]
mod issue_create_command_tests;
#[cfg(test)]
mod issue_update_command_tests;
mod attachment_input;
mod auth_command_schema;
mod auth_commands;
mod autopilot_commands;
mod autopilot_output;
mod autopilot_resolver;
mod chat_commands;
mod client_factory;
mod command_dispatch;
pub mod config;
mod config_command_schema;
mod config_commands;
pub mod daemon;
mod daemon_command_schema;
mod daemon_commands;
mod disk_usage_commands;
mod disk_usage_output;
pub mod error;
mod execution_policy;
mod id_helpers;
mod issue_activity_schema;
mod issue_actor_output;
mod issue_actor_resolver;
mod issue_assign_commands;
mod issue_children_commands;
mod issue_command_schema;
mod issue_comment_add_commands;
mod issue_comment_list_commands;
mod issue_comment_mutation_commands;
mod issue_create_commands;
mod issue_description;
mod issue_get_commands;
mod issue_label_commands;
mod issue_label_schema;
mod issue_list_commands;
mod issue_list_schema;
mod issue_metadata_commands;
mod issue_metadata_schema;
mod issue_property_schema;
mod issue_pull_request_commands;
mod issue_pull_request_schema;
mod issue_reference;
mod issue_reorder_commands;
mod issue_rerun_commands;
mod issue_safety;
mod issue_search_commands;
mod issue_status_commands;
mod issue_subscriber_commands;
mod issue_subscriber_schema;
mod issue_task_commands;
mod issue_timeline_commands;
mod issue_timeline_schema;
mod issue_update_commands;
mod issue_usage_commands;
mod issue_value_helpers;
mod json_helpers;
mod label_command_schema;
mod label_commands;
mod label_reference;
mod login;
mod output_helpers;
mod path_safety;
mod project_command_schema;
mod project_commands;
mod project_resource_commands;
mod property_commands;
mod repo_commands;
mod root_command_schema;
mod runtime_commands;
mod runtime_delete;
mod runtime_output;
mod runtime_profile;
mod runtime_update;
mod setup_command_schema;
mod setup_commands;
mod skill_command_schema;
mod skill_commands;
mod squad_command_schema;
mod squad_commands;
mod task_reference;
mod text_input;
mod update_commands;
mod url_helpers;
mod user_command_schema;
mod user_commands;
mod version_output;
mod workspace_command_schema;
mod workspace_commands;
mod workspace_mcp_commands;

use anyhow::{bail, Context, Result};
use api::{http_timeout, ApiClient, HealthProbeError};
use chrono::{DateTime, FixedOffset};
use clap::{Args, Parser, Subcommand, ValueEnum};
use config::Environment;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::{form_urlencoded, Url};

pub(super) use agent_commands::{
    agent_mcp_path, run_agent_avatar, run_agent_copy, run_agent_create, run_agent_env_get,
    run_agent_env_set, run_agent_get, run_agent_lifecycle, run_agent_list, run_agent_mcp_list,
    run_agent_mcp_mutation, run_agent_skills_list, run_agent_skills_mutation, run_agent_tasks,
    run_agent_update, AgentArgs, AgentCommand, AgentCopyArgs, AgentCreateArgs, AgentEnvArgs,
    AgentEnvCommand, AgentEnvSetArgs, AgentMcpAction, AgentMcpArgs, AgentMcpListArgs,
    AgentMcpMutationArgs, AgentSkillsArgs, AgentSkillsCommand, AgentSkillsMutationArgs,
    AgentUpdateArgs,
};
pub(super) use agent_helpers::{
    apply_agent_permission_args, copied_agent_max_concurrent_tasks, format_agent_details_table,
    format_agent_list_table, resolve_agent_secret_json, validate_agent_custom_env,
};
use attachment_input::{
    append_unique_strings, collect_local_attachments, quick_create_attachment_ids,
    PendingAttachment,
};
pub(super) use auth_command_schema::{AuthArgs, AuthCommand, LoginArgs};
use auth_commands::{display_token_prefix, run_auth_logout, run_auth_status};
pub(super) use autopilot_commands::{
    run_autopilot_create, run_autopilot_delete, run_autopilot_get, run_autopilot_list,
    run_autopilot_runs, run_autopilot_trigger, run_autopilot_trigger_add,
    run_autopilot_trigger_delete, run_autopilot_trigger_rotate_url, run_autopilot_trigger_update,
    run_autopilot_update, AutopilotArgs, AutopilotCommand, AutopilotCreateArgs,
    AutopilotTriggerAddArgs, AutopilotTriggerRotateUrlArgs, AutopilotTriggerUpdateArgs,
    AutopilotUpdateArgs,
};
use autopilot_output::{
    autopilot_webhook_url, format_autopilot_runs_table, format_autopilot_table,
};
use autopilot_resolver::{
    load_autopilot_agent_names, resolve_autopilot_agent, resolve_autopilot_id,
    resolve_autopilot_subscribers, resolve_autopilot_trigger_id,
};
pub(super) use chat_commands::{
    run_attachment_download, run_attachment_upload, run_chat_read, AttachmentArgs,
    AttachmentCommand, ChatArgs, ChatCommand, ChatReadArgs, ChatThreadArgs,
};
pub(super) use client_factory::{
    new_api_client, new_unscoped_api_client, new_unscoped_authenticated_api_client,
    normalize_api_base_url, required_workspace_id, resolve_current_workspace_id,
};
pub(super) use command_dispatch::run_with_input;
pub(super) use config_command_schema::{ConfigArgs, ConfigCommand};
use config_commands::{
    config_display_values, format_config_table, run_config_set, run_config_show,
    validate_config_set,
};
pub(super) use daemon_command_schema::{
    DaemonArgs, DaemonCommand, DaemonDiskUsageArgs, DaemonLaunchArgs, DaemonLogsArgs,
    DaemonRestartArgs, DaemonStartArgs, DaemonStatusArgs,
};
pub use daemon_commands::run_private_helper;
use daemon_commands::{
    ensure_restart_is_background, format_daemon_status_table, known_daemon_profiles,
    parse_cli_duration, parse_log_lines, read_daemon_log_tail, render_daemon_status,
    require_known_daemon_profile, resolve_daemon_log_path, resolve_daemon_status_port,
    run_daemon_after_setup, run_daemon_disk_usage, run_daemon_logs, run_daemon_probe_runtimes,
    run_daemon_restart, run_daemon_start, run_daemon_status, run_daemon_stop,
    validate_daemon_health_port,
};
use disk_usage_commands::{
    disk_usage_needs_parent_status, disk_usage_task_context, enumerate_disk_usage_roots,
    fill_disk_usage_parent_statuses, limit_disk_usage_aggregate, limit_disk_usage_report,
    resolve_disk_usage_root, validate_disk_usage_args,
};
use disk_usage_output::{
    append_disk_usage_warning, format_disk_ratio, format_disk_usage_aggregate_table,
    format_disk_usage_report_table,
};
pub use error::command_error_output;
pub(super) use error::command_output_error;
pub(super) use execution_policy::{require_human_local_command, require_task_local_config_root};
pub(super) use id_helpers::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix};
pub(super) use issue_activity_schema::{
    IssueCancelTaskArgs, IssueCommentAddArgs, IssueCommentArgs, IssueCommentCommand,
    IssueCommentListArgs, IssueCommentResolutionArgs, IssueRerunArgs, IssueRunMessagesArgs,
    IssueRunsArgs, IssueSearchArgs, IssueUsageArgs,
};
use issue_actor_output::{format_issue_list_table, load_issue_actor_names, IssueActorNames};
use issue_actor_resolver::{
    resolve_issue_assignee_id, resolve_issue_assignee_name, resolve_issue_project_id,
    resolve_project_reference, resolve_subscriber_id, resolve_subscriber_name,
    ResolvedIssueAssignee,
};
use issue_assign_commands::run_issue_assign;
use issue_children_commands::{
    child_stage, format_issue_children_table, group_issue_children, run_issue_children,
};
pub(super) use issue_command_schema::{
    IssueArgs, IssueAssignArgs, IssueCommand, IssueCreateArgs, IssueReorderArgs, IssueStatusArgs,
    IssueUpdateArgs,
};
use issue_comment_add_commands::{resolve_issue_comment_content, run_issue_comment_add};
use issue_comment_list_commands::{format_issue_comments_table, run_issue_comment_list};
use issue_comment_mutation_commands::{run_issue_comment_delete, run_issue_comment_resolution};
use issue_create_commands::run_issue_create;
use issue_description::{resolve_issue_create_description, resolve_issue_update_description};
use issue_get_commands::{format_issue_get_table, run_issue_get};
use issue_label_commands::{
    format_issue_labels, run_issue_label_add, run_issue_label_list, run_issue_label_remove,
};
pub(super) use issue_label_schema::{
    IssueLabelArgs, IssueLabelCommand, IssueLabelListArgs, IssueLabelMutationArgs,
};
use issue_list_commands::{
    build_issue_list_query, build_metadata_filter, issue_list_has_more, run_issue_list,
};
pub(super) use issue_list_schema::IssueListArgs;
use issue_metadata_commands::{
    format_metadata_table, parse_metadata_value, run_issue_metadata_delete, run_issue_metadata_get,
    run_issue_metadata_list, run_issue_metadata_set,
};
pub(super) use issue_metadata_schema::{
    IssueMetadataArgs, IssueMetadataCommand, IssueMetadataDeleteArgs, IssueMetadataKeyArgs,
    IssueMetadataListArgs, IssueMetadataSetArgs,
};
pub(super) use issue_property_schema::{
    IssuePropertyArgs, IssuePropertyCommand, IssuePropertyListArgs, IssuePropertyMutationArgs,
    IssuePropertyUnsetArgs,
};
use issue_pull_request_commands::{
    format_issue_pull_requests_table, run_issue_pull_request_attach, run_issue_pull_requests,
};
pub(super) use issue_pull_request_schema::{
    IssuePullRequestArgs, IssuePullRequestAttachArgs, IssuePullRequestCommand,
};
use issue_reference::resolve_issue_ref;
use issue_reorder_commands::{compute_reorder_position, run_issue_reorder};
use issue_rerun_commands::run_issue_rerun;
use issue_safety::{active_duplicate_issue_message, guard_issue_description_local_links};
use issue_search_commands::{format_issue_search_table, run_issue_search};
use issue_status_commands::run_issue_status;
use issue_subscriber_commands::{
    format_issue_subscribers_table, run_issue_subscriber_list, run_issue_subscriber_mutation,
};
pub(super) use issue_subscriber_schema::{
    IssueSubscriberArgs, IssueSubscriberCommand, IssueSubscriberMutationArgs,
};
use issue_task_commands::{
    format_issue_run_messages_table, format_issue_runs_table, run_issue_cancel_task,
    run_issue_run_messages, run_issue_runs,
};
use issue_timeline_commands::{
    build_timeline_filter, filter_timeline, format_issue_timeline_table, run_issue_timeline,
};
pub(super) use issue_timeline_schema::IssueTimelineArgs;
use issue_update_commands::run_issue_update;
use issue_usage_commands::run_issue_usage;
use issue_value_helpers::{
    format_metadata_value, issue_labels, validate_issue_priority, validate_issue_status,
};
pub(super) use json_helpers::value_string;
pub(super) use label_command_schema::{LabelArgs, LabelCommand, LabelCreateArgs, LabelUpdateArgs};
use label_commands::{
    format_label_result, format_label_table, format_workspace_label_table, run_label_create,
    run_label_delete, run_label_get, run_label_list, run_label_update,
};
use label_reference::{resolve_label_id, resolve_label_reference};
use login::{
    build_login_url, build_workspace_creation_url, constant_time_equal, run_browser_login,
    run_login, validate_login_token, wait_for_login_callback, wait_for_workspace_creation,
    wait_for_workspace_creation_with_opener, AuthUser, LoginWorkspace,
    WORKSPACE_DISCOVERY_INTERVAL, WORKSPACE_DISCOVERY_TIMEOUT,
};
pub(super) use output_helpers::{display_id, format_table, truncate_text};
use path_safety::{ensure_file_within_workdir, lexical_normalize};
pub(super) use project_command_schema::{
    ProjectArgs, ProjectCommand, ProjectCreateArgs, ProjectResourceAddArgs, ProjectResourceArgs,
    ProjectResourceCommand, ProjectResourceUpdateArgs, ProjectUpdateArgs,
};
use project_commands::{
    format_project_details_table, format_project_list_table, format_project_mutation,
    project_actor_inputs, project_lead, run_project_create, run_project_delete, run_project_get,
    run_project_list, run_project_status, run_project_update, validate_project_status,
    PROJECT_STATUSES,
};
use project_resource_commands::{
    build_project_resource_add_ref, build_project_resource_update_ref, run_project_resource_add,
    run_project_resource_list, run_project_resource_remove, run_project_resource_update,
};
pub(super) use property_commands::{
    build_issue_property_rows, format_issue_property_rows, format_property_definitions,
    parse_property_options, resolve_property, run_issue_property_list, run_issue_property_set,
    run_issue_property_unset, run_property_archive, run_property_create, run_property_get,
    run_property_list, run_property_update, PropertyArchiveArgs, PropertyArgs, PropertyCommand,
    PropertyCreateArgs, PropertyDefinition, PropertyOption, PropertyUpdateArgs,
};
pub(super) use repo_commands::{
    repo_checkout_retry_delay, repo_urls, run_repo_add, run_repo_checkout, run_repo_list,
    run_repo_remove, RepoArgs, RepoCommand, RepoMutationArgs, RepoRemoveArgs, WorkspaceRepo,
};
pub(super) use root_command_schema::{UpdateArgs, VersionOutput};
pub(super) use runtime_commands::{
    run_runtime_activity, run_runtime_delete, run_runtime_list, run_runtime_rename,
    run_runtime_usage, RuntimeArgs, RuntimeCommand, RuntimeProfileArgs, RuntimeProfileCommand,
    RuntimeProfileCreateArgs, RuntimeProfileUpdateArgs,
};
use runtime_delete::{format_runtime_delete_result, runtime_delete_conflict};
use runtime_output::{format_runtime_rows, output_runtime_profiles};
use runtime_profile::{
    run_runtime_profile_create, run_runtime_profile_delete, run_runtime_profile_list,
    run_runtime_profile_set_path, run_runtime_profile_unset_path, run_runtime_profile_update,
};
use runtime_update::{
    format_runtime_update_result, run_runtime_update, run_runtime_update_with_policy,
};
pub(super) use setup_command_schema::{
    SetupArgs, SetupCloudArgs, SetupCommand, SetupError, SetupSelfHostArgs,
};
use setup_commands::{
    confirm_setup_overwrite, dispatch_daemon_after_setup, format_setup_value_change,
    prepare_setup_profile, prepare_setup_profile_input, read_setup_confirmation,
    resolve_setup_profile_input, run_setup, setup_callback_host, setup_daemon_action,
    setup_server_is_local, SetupDaemonAction,
};
pub(super) use skill_command_schema::{
    SkillArgs, SkillCommand, SkillCreateArgs, SkillDeleteArgs, SkillFilesArgs,
    SkillFilesCommand, SkillFilesDeleteArgs, SkillFilesListArgs, SkillFilesUpsertArgs,
    SkillGetArgs, SkillImportArgs, SkillRefreshArgs, SkillSearchArgs, SkillUpdateArgs,
};
use skill_commands::{
    format_skill_files_table, format_skill_import_table, format_skill_list_table,
    format_skill_search_table, read_skill_archive, resolve_skill_content,
    resolve_skill_content_sources, run_skill_create, run_skill_delete, run_skill_files_delete,
    run_skill_files_list, run_skill_files_upsert, run_skill_get, run_skill_import, run_skill_list,
    run_skill_refresh, run_skill_search, run_skill_update,
};
pub(super) use squad_command_schema::{
    SquadActivityArgs, SquadArgs, SquadCommand, SquadCreateArgs, SquadMemberAddArgs,
    SquadMemberArgs, SquadMemberCommand, SquadMemberRemoveArgs, SquadMemberSetRoleArgs,
    SquadUpdateArgs,
};
use squad_commands::{
    format_squad_details_table, format_squad_list_table, render_squad_member_output,
    run_squad_activity, run_squad_create, run_squad_delete, run_squad_get, run_squad_list,
    run_squad_member_add, run_squad_member_list, run_squad_member_remove,
    run_squad_member_set_role, run_squad_update, squad_member_count_display,
};
use task_reference::resolve_task_run_id;
pub(super) use text_input::{trim_one_trailing_newline, unescape_backslash_escapes};
use update_commands::{
    render_update_outcome, resolve_update_download_timeout, run_update, validate_update_timeout,
};
pub(super) use url_helpers::encoded_path_segment;
pub(super) use user_command_schema::{
    ProfileArgs, ProfileCommand, UpdateProfileArgs, UserArgs, UserCommand,
};
use user_commands::{
    format_user_profile_table, resolve_profile_description, run_user_profile_get,
    run_user_profile_update,
};
use version_output::run_version;
pub(super) use workspace_command_schema::{
    CreateWorkspaceArgs, UpdateWorkspaceArgs, WorkspaceArgs, WorkspaceCommand,
    WorkspaceMemberArgs, WorkspaceMemberCommand, WorkspaceMemberInviteArgs, WorkspaceMcpAddArgs,
    WorkspaceMcpArgs, WorkspaceMcpCommand, WorkspaceMcpUpdateArgs,
};
use workspace_commands::{
    build_workspace_create_body, build_workspace_update_body, format_workspace_details_table,
    format_workspace_members, format_workspace_table, normalize_workspace_invite_role,
    resolve_workspace_reference, run_workspace_create, run_workspace_get, run_workspace_list,
    run_workspace_member_invite, run_workspace_member_list, run_workspace_switch,
    run_workspace_update, WorkspaceSummary,
};
use workspace_mcp_commands::{
    format_workspace_mcp_servers, parse_workspace_mcp_server_config,
    resolve_workspace_mcp_server_config, run_workspace_mcp_add, run_workspace_mcp_list,
    run_workspace_mcp_remove, run_workspace_mcp_update, WorkspaceMcpServer,
};

pub const CLIENT_VERSION: &str = env!("CORDY_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("CORDY_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("CORDY_BUILD_DATE");
pub const BUILD_GO_VERSION: &str = env!("CORDY_BUILD_GO_VERSION");
pub const BUILD_OS: &str = env!("CORDY_BUILD_OS");
pub const BUILD_ARCH: &str = env!("CORDY_BUILD_ARCH");

pub const ROOT_LONG_VERSION: &str = concat!(
    env!("CORDY_BUILD_VERSION"),
    " (commit: ",
    env!("CORDY_BUILD_COMMIT"),
    ", built: ",
    env!("CORDY_BUILD_DATE"),
    ")\ngo: ",
    env!("CORDY_BUILD_GO_VERSION"),
    ", os/arch: ",
    env!("CORDY_BUILD_OS"),
    "/",
    env!("CORDY_BUILD_ARCH")
);

pub use cli_command_schema::Cli;
pub(super) use cli_command_schema::Command;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Debug)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
}

pub async fn run(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    run_with_input(cli, environment, &mut stdin).await
}

const CLOUD_SERVER_URL: &str = "https://api.cordy.ai";
const CLOUD_APP_URL: &str = "https://cordy.ai";

const VALID_ISSUE_SORT_COLUMNS: &[&str] = &[
    "position",
    "title",
    "created_at",
    "start_date",
    "due_date",
    "priority",
];

#[derive(Debug, Default, Deserialize)]
struct IssueListResponse {
    #[serde(default)]
    issues: Value,
    #[serde(default)]
    total: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Request;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{delete as delete_route, get, patch, post, put};
    use axum::{Json, Router};
    use clap::Parser;
    use std::fs;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn private_execenv_helper_dispatches_before_cli_parsing() {
        let missing = tempfile::tempdir()
            .expect("tempdir")
            .path()
            .join("missing-workdir");
        let input = serde_json::to_vec(&serde_json::json!({
            "action": "reuse",
            "reuse": {
                "WorkDir": missing,
                "Provider": "codex"
            }
        }))
        .expect("helper request");
        let mut output = Vec::new();

        let handled = run_private_helper(
            &[
                OsString::from("cordy"),
                OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
            ],
            Cursor::new(input),
            &mut output,
        )
        .await
        .expect("private helper");

        assert!(handled);
        let response: Value = serde_json::from_slice(&output).expect("helper response");
        assert!(response.get("environment").is_none());
        assert!(response.get("error").is_none());
    }

    #[tokio::test]
    async fn private_execenv_helper_requires_the_exact_private_argv() {
        let mut output = Vec::new();
        let handled = run_private_helper(
            &[
                OsString::from("cordy"),
                OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
                OsString::from("unexpected"),
            ],
            Cursor::new(Vec::<u8>::new()),
            &mut output,
        )
        .await
        .expect("ordinary CLI path");

        assert!(!handled);
        assert!(output.is_empty());
    }





    async fn test_server() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer token-from-env");
                assert_eq!(request.headers()["x-workspace-id"], "workspace-from-env");
                assert_eq!(request.headers()["x-client-platform"], "cli");
                assert_eq!(
                    request.headers()["x-client-capabilities"],
                    "stable_attachment_urls"
                );
                axum::Json(serde_json::json!({
                    "id": "user-1",
                    "name": "Ada",
                    "email": "ada@example.com",
                    "profile_description": "Maintainer"
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (format!("http://{address}"), task)
    }

    async fn patch_test_server() -> (
        String,
        Arc<Mutex<Option<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let captured = Arc::new(Mutex::new(None));
        let captured_by_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/me",
            patch(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_handler);
                async move {
                    *captured.lock().expect("capture body") = Some(body.clone());
                    Json(serde_json::json!({
                        "id": "user-1",
                        "name": "Ada",
                        "email": "ada@example.com",
                        "profile_description": body["profile_description"]
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (format!("http://{address}"), captured, task)
    }

    fn update_args(cli: &Cli) -> &UpdateProfileArgs {
        match &cli.command {
            Command::User(UserArgs {
                command:
                    UserCommand::Profile(ProfileArgs {
                        command: ProfileCommand::Update(args),
                    }),
            }) => args,
            _ => panic!("expected user profile update"),
        }
    }

    fn create_workspace_args(cli: &Cli) -> &CreateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Create(args),
            }) => args,
            _ => panic!("expected workspace create"),
        }
    }

    fn update_workspace_args(cli: &Cli) -> &UpdateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Update(args),
            }) => args,
            _ => panic!("expected workspace update"),
        }
    }

    fn issue_list_args(cli: &Cli) -> &IssueListArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::List(args),
            }) => args,
            _ => panic!("expected issue list"),
        }
    }

    fn issue_create_args(cli: &Cli) -> &IssueCreateArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Create(args),
            }) => args,
            _ => panic!("expected issue create"),
        }
    }

    fn issue_update_args(cli: &Cli) -> &IssueUpdateArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Update(args),
            }) => args,
            _ => panic!("expected issue update"),
        }
    }

    fn issue_assign_args(cli: &Cli) -> &IssueAssignArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Assign(args),
            }) => args,
            _ => panic!("expected issue assign"),
        }
    }

    fn issue_status_args(cli: &Cli) -> &IssueStatusArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Status(args),
            }) => args,
            _ => panic!("expected issue status"),
        }
    }

    fn issue_reorder_args(cli: &Cli) -> &IssueReorderArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Reorder(args),
            }) => args,
            _ => panic!("expected issue reorder"),
        }
    }

    fn issue_comment_add_args(cli: &Cli) -> &IssueCommentAddArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::Comment(IssueCommentArgs {
                        command: IssueCommentCommand::Add(args),
                    }),
            }) => args,
            _ => panic!("expected issue comment add"),
        }
    }

    fn issue_comment_list_args(cli: &Cli) -> &IssueCommentListArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::Comment(IssueCommentArgs {
                        command: IssueCommentCommand::List(args),
                    }),
            }) => args,
            _ => panic!("expected issue comment list"),
        }
    }

    fn issue_runs_args(cli: &Cli) -> &IssueRunsArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Runs(args),
            }) => args,
            _ => panic!("expected issue runs"),
        }
    }

    fn issue_run_messages_args(cli: &Cli) -> &IssueRunMessagesArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::RunMessages(args),
            }) => args,
            _ => panic!("expected issue run-messages"),
        }
    }

    fn issue_cancel_task_args(cli: &Cli) -> &IssueCancelTaskArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::CancelTask(args),
            }) => args,
            _ => panic!("expected issue cancel-task"),
        }
    }

    fn issue_usage_args(cli: &Cli) -> &IssueUsageArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Usage(args),
            }) => args,
            _ => panic!("expected issue usage"),
        }
    }

    fn issue_rerun_args(cli: &Cli) -> &IssueRerunArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Rerun(args),
            }) => args,
            _ => panic!("expected issue rerun"),
        }
    }

    fn issue_search_args(cli: &Cli) -> &IssueSearchArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Search(args),
            }) => args,
            _ => panic!("expected issue search"),
        }
    }

    #[tokio::test]
    async fn issue_assign_parser_and_local_validation_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--to-id",
            "11111111-1111-1111-1111-111111111111",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("assign CLI");
        let args = issue_assign_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(
            args.to_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Table);

        let missing = Cli::try_parse_from(["cordy", "issue", "assign", "CORD-18"])
            .expect("validation is at runtime");
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let error = run_with_input(&missing, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing target");
        assert!(error.to_string().contains("provide --to"));
    }

    #[tokio::test]
    async fn issue_assign_puts_resolved_actor_and_supports_unassign() {
        let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
        let bodies_by_update = Arc::clone(&bodies);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([])) }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(serde_json::json!([{"id":"11111111-1111-1111-1111-111111111111","name":"CodeBot"}])) }),
            )
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let bodies = Arc::clone(&bodies_by_update);
                    async move {
                        bodies.lock().expect("bodies").push(body);
                        Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let assign = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--to-id",
            "11111111-1111-1111-1111-111111111111",
            "--no-start",
        ])
        .expect("assign CLI");
        let output = run_with_input(&assign, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("assign");
        assert!(output.stderr.contains("assigned to agent:CodeBot"));
        let assign_body = bodies.lock().expect("bodies")[0].clone();
        assert_eq!(assign_body["assignee_type"], "agent");
        assert_eq!(
            assign_body["assignee_id"],
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(assign_body["suppress_run"], true);

        let unassign = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--unassign",
            "--output",
            "table",
        ])
        .expect("unassign CLI");
        let output = run_with_input(&unassign, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("unassign");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Issue CORD-18 unassigned.\n");
        let unassign_body = bodies.lock().expect("bodies")[1].clone();
        assert_eq!(unassign_body["assignee_type"], Value::Null);
        assert_eq!(unassign_body["assignee_id"], Value::Null);
        task.abort();
    }

    #[tokio::test]
    async fn issue_assign_rejects_no_start_with_unassign_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--unassign",
            "--no-start",
        ])
        .expect("assign CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("invalid no-start unassign");
        assert!(error.to_string().contains("--no-start"));
    }

    #[test]
    fn issue_status_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "status",
            "CORD-18",
            "custom_status",
            "--no-start",
            "--output",
            "json",
        ])
        .expect("status CLI");
        let args = issue_status_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.status, "custom_status");
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Json);
    }

    #[tokio::test]
    async fn issue_status_validates_then_puts_status_and_suppress_run() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture status") = Some(body);
                        Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18","status":"custom_status"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "status",
            "CORD-18",
            "custom_status",
            "--no-start",
            "--output",
            "json",
        ])
        .expect("status CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("status update");
        assert_eq!(
            output.stderr,
            "Issue CORD-18 status changed to custom_status.\n"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("status JSON")["status"],
            "custom_status"
        );
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["status"], "custom_status");
        assert_eq!(body["suppress_run"], true);
        task.abort();
    }

    #[tokio::test]
    async fn issue_status_rejects_malformed_status_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "status", "CORD-18", "not a status"])
            .expect("status CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("malformed status");
        assert!(error.to_string().contains("status key"));
    }

    #[test]
    fn issue_reorder_parser_enforces_exactly_one_real_target() {
        assert!(Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18"]).is_err());
        assert!(
            Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top", "--bottom"])
                .is_err()
        );
        let cli = Cli::try_parse_from([
            "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
        ])
        .expect("reorder CLI");
        let args = issue_reorder_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.before.as_deref(), Some("CORD-1"));
        assert_eq!(args.output, OutputFormat::Table);

        let false_top =
            Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top=false"])
                .expect("false bool reaches runtime");
        assert_eq!(issue_reorder_args(&false_top).top, Some(false));
    }

    #[test]
    fn issue_reorder_position_math_matches_board_drag_contract() {
        let positions = HashMap::from([
            (String::from("one"), 10.0),
            (String::from("two"), 20.0),
            (String::from("three"), 40.0),
        ]);
        assert_eq!(
            compute_reorder_position(
                &["two".into(), "one".into(), "three".into()],
                "two",
                &positions,
                20.0,
            ),
            9.0
        );
        assert_eq!(
            compute_reorder_position(
                &["one".into(), "two".into(), "three".into()],
                "two",
                &positions,
                20.0,
            ),
            25.0
        );
        assert_eq!(
            compute_reorder_position(
                &["one".into(), "three".into(), "two".into()],
                "two",
                &positions,
                20.0,
            ),
            41.0
        );
    }

    #[tokio::test]
    async fn issue_reorder_paginates_project_column_and_puts_computed_position() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"target-id","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/CORD-1",
                get(|| async { Json(serde_json::json!({"id":"other-id","identifier":"CORD-1"})) }),
            )
            .route(
                "/api/issues/target-id",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"target-id","identifier":"CORD-18","title":"Target",
                        "status":"todo","priority":"high","project_id":"project-1","position":20.0
                    }))
                })
                .put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture reorder") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"target-id","identifier":"CORD-18","title":"Target",
                            "status":"todo","priority":"high","position":body["position"]
                        }))
                    }
                }),
            )
            .route(
                "/api/issues",
                get(|request: Request| async move {
                    let query = request.uri().query().unwrap_or_default();
                    assert!(query.contains("workspace_id=workspace-1"));
                    assert!(query.contains("status=todo"));
                    assert!(query.contains("project_id=project-1"));
                    assert!(query.contains("sort=position"));
                    if query.contains("offset=0") {
                        Json(serde_json::json!({
                            "issues":[
                                {"id":"other-id","position":10.0},
                                {"id":"target-id","position":20.0}
                            ],
                            "total":3
                        }))
                    } else {
                        assert!(query.contains("offset=2"));
                        Json(serde_json::json!({
                            "issues":[{"id":"last-id","position":30.0}],
                            "total":3
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
        ])
        .expect("reorder CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("reorder issue");
        assert_eq!(output.stderr, "Issue CORD-18 reordered.\n");
        assert!(output.stdout.starts_with("KEY"));
        assert_eq!(
            captured
                .lock()
                .expect("body")
                .clone()
                .expect("captured body")["position"],
            9.0
        );
        task.abort();
    }

    #[tokio::test]
    async fn issue_reorder_rejects_false_selector_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--bottom=false"])
            .expect("false bool reaches runtime");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("false selector");
        assert!(error.to_string().contains("cannot be set to false"));
    }

    #[test]
    fn issue_comment_add_parser_and_content_sources_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content",
            "one\\ntwo",
            "--parent",
            "comment-1",
            "--attachment",
            "one.png",
            "--output",
            "table",
        ])
        .expect("comment add CLI");
        let args = issue_comment_add_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(args.parent.as_deref(), Some("comment-1"));
        assert_eq!(args.attachment, vec![String::from("one.png")]);
        assert_eq!(args.output, OutputFormat::Table);
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        assert_eq!(
            resolve_issue_comment_content(args, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .expect("inline content"),
            Some("one\ntwo".into())
        );

        let empty_file = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content-file",
            "",
        ])
        .expect("empty file reaches runtime");
        assert!(resolve_issue_comment_content(
            issue_comment_add_args(&empty_file),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty file is unset")
        .is_none());
    }

    #[tokio::test]
    async fn issue_comment_add_prevalidates_uploads_then_posts_attachment_ids() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_comment = Arc::clone(&captured);
        let uploads = Arc::new(Mutex::new(0_usize));
        let uploads_by_handler = Arc::clone(&uploads);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/upload-file",
                post(move |headers: HeaderMap, _body: axum::body::Bytes| {
                    let uploads = Arc::clone(&uploads_by_handler);
                    async move {
                        *uploads.lock().expect("uploads") += 1;
                        assert!(headers["content-type"]
                            .to_str()
                            .expect("content type")
                            .starts_with("multipart/form-data; boundary="));
                        Json(serde_json::json!({"id":"attachment-1"}))
                    }
                }),
            )
            .route(
                "/api/issues/issue-uuid/comments",
                post(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_comment);
                    async move {
                        *captured.lock().expect("comment body") = Some(body.clone());
                        Json(serde_json::json!({"id":"comment-1","content":body["content"]}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("proof.txt"), b"proof").expect("attachment");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content",
            "Completed\\nSee proof.",
            "--parent",
            "parent-comment",
            "--attachment",
            "proof.txt",
        ])
        .expect("comment add CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add comment");
        assert!(output.stderr.contains("Uploaded proof.txt"));
        assert!(output.stderr.contains("Comment added to issue CORD-18."));
        assert_eq!(*uploads.lock().expect("uploads"), 1);
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["content"], "Completed\nSee proof.");
        assert_eq!(body["parent_id"], "parent-comment");
        assert_eq!(body["attachment_ids"], serde_json::json!(["attachment-1"]));
        task.abort();
    }

    #[tokio::test]
    async fn issue_comment_add_rejects_missing_content_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "comment", "add", "CORD-18"])
            .expect("missing content reaches runtime");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing content");
        assert!(error.to_string().contains("--content-file is required"));
    }

    #[tokio::test]
    async fn issue_comment_delete_resolve_and_unresolve_match_go_http_contracts() {
        let app = Router::new()
            .route(
                "/api/comments/comment-1",
                delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/comments/comment-1/resolve",
                post(|| async {
                    Json(serde_json::json!({"id":"comment-1","resolved_at":"2026-08-24T00:00:00Z"}))
                })
                .delete(|| async {
                    Json(serde_json::json!({"id":"comment-1","resolved_at":null}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let delete = Cli::try_parse_from(["cordy", "issue", "comment", "delete", "comment-1"])
            .expect("delete CLI");
        let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete comment");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Comment comment-1 deleted.\n");

        let resolve = Cli::try_parse_from(["cordy", "issue", "comment", "resolve", "comment-1"])
            .expect("resolve CLI");
        let output = run_with_input(&resolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("resolve comment");
        assert_eq!(output.stderr, "Comment comment-1 resolved.\n");
        assert!(
            serde_json::from_str::<Value>(&output.stdout).expect("resolved JSON")["resolved_at"]
                .is_string()
        );

        let unresolve = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "unresolve",
            "comment-1",
            "--output",
            "table",
        ])
        .expect("unresolve CLI");
        let output = run_with_input(&unresolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("unresolve comment");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Comment comment-1 unresolved.\n");
        task.abort();
    }

    #[tokio::test]
    async fn issue_comment_list_parser_and_validation_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "list",
            "CORD-18",
            "--thread",
            "comment-1",
            "--tail",
            "0",
            "--summary",
            "--compact",
            "--full",
            "--before",
            "2026-08-24T00:00:00Z",
            "--before-id",
            "comment-2",
            "--output",
            "json",
        ])
        .expect("comment list CLI");
        let args = issue_comment_list_args(&cli);
        assert_eq!(args.thread.as_deref(), Some("comment-1"));
        assert_eq!(args.tail, Some(0));
        assert!(args.summary && args.compact && args.full);
        assert_eq!(args.output, OutputFormat::Json);

        let invalid = Cli::try_parse_from([
            "cordy", "issue", "comment", "list", "CORD-18", "--tail", "1",
        ])
        .expect("combination validation is at runtime");
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("tail requires thread");
        assert!(error.to_string().contains("--tail requires --thread"));
    }

    #[tokio::test]
    async fn issue_comment_list_sends_folded_recent_query_surfaces_cursor_and_compacts_json() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/comments",
                get(|request: Request| async move {
                    let query = request.uri().query().unwrap_or_default();
                    assert!(query.contains("summary=true"));
                    assert!(query.contains("fold=true"));
                    assert!(query.contains("recent=2"));
                    assert!(query.contains("before=2026-08-24T00%3A00%3A00Z"));
                    assert!(query.contains("before_id=comment-2"));
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "X-Cordy-Next-Before",
                        "2026-08-23T23:00:00Z".parse().expect("cursor"),
                    );
                    headers.insert(
                        "X-Cordy-Next-Before-Id",
                        "comment-older".parse().expect("cursor id"),
                    );
                    (
                        headers,
                        Json(vec![serde_json::json!({
                            "id":"comment-1","issue_id":"issue-uuid","source_task_id":null,
                            "author_type":"member","author_id":"member-1","type":"comment",
                            "content":"summary","created_at":"2026-08-24T00:00:00Z",
                            "updated_at":"2026-08-24T00:00:00Z","parent_id":null,
                            "attachments":[]
                        })]),
                    )
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "list",
            "CORD-18",
            "--recent",
            "2",
            "--summary",
            "--compact",
            "--before",
            "2026-08-24T00:00:00Z",
            "--before-id",
            "comment-2",
            "--output",
            "json",
        ])
        .expect("comment list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list comments");
        assert_eq!(
            output.stderr,
            "Next thread cursor: --before 2026-08-23T23:00:00Z --before-id comment-older\n"
        );
        let comments: Value = serde_json::from_str(&output.stdout).expect("comments JSON");
        let comment = &comments[0];
        assert!(comment.get("issue_id").is_none());
        assert!(comment.get("source_task_id").is_none());
        assert!(comment.get("updated_at").is_none());
        assert!(comment.get("parent_id").is_none());
        assert!(comment.get("attachments").is_none());
        task.abort();
    }

    #[test]
    fn issue_comment_list_table_truncates_and_formats_actor_fallback() {
        let comments = vec![serde_json::json!({
            "id":"comment-1","parent_id":null,"author_type":"agent","author_id":"agent-1",
            "type":"comment","content":"x".repeat(81),"created_at":"2026-08-24T12:34:56Z"
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
        let table = format_issue_comments_table(&comments, &actors);
        assert!(table.starts_with("ID"));
        assert!(table.contains("agent:CodeBot"));
        assert!(table.contains("2026-08-24T12:34"));
        assert!(table.contains("xxx..."));
    }

    #[test]
    fn issue_runs_parser_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "runs",
            "CORD-18",
            "--full-id",
            "--output",
            "json",
        ])
        .expect("runs CLI");
        let args = issue_runs_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert!(args.full_id);
        assert_eq!(args.output, OutputFormat::Json);

        let runs = vec![serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","agent_id":"agent-1",
            "status":"failed","started_at":"2026-08-24T12:34:56Z",
            "completed_at":"2026-08-24T12:40:00Z","error":"x".repeat(51)
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
        let short = format_issue_runs_table(&runs, false, &actors);
        assert!(short.contains("11111111"));
        assert!(!short.contains("11111111-1111"));
        assert!(short.contains("CodeBot"));
        assert!(short.contains("2026-08-24T12:34"));
        assert!(short.contains("xxx..."));
        let full = format_issue_runs_table(&runs, true, &actors);
        assert!(full.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[tokio::test]
    async fn issue_runs_resolves_issue_fetches_task_runs_and_actor_names() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/task-runs",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "id":"task-uuid","agent_id":"agent-1","status":"completed",
                        "started_at":"2026-08-24T12:34:56Z","completed_at":"2026-08-24T12:40:00Z"
                    })])
                }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"CodeBot"})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "runs", "CORD-18"]).expect("runs CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list runs");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("CodeBot"));
        assert!(output.stdout.contains("completed"));
        task.abort();
    }

    #[test]
    fn issue_run_controls_parser_and_message_table_match_go_contract() {
        let messages = Cli::try_parse_from([
            "cordy",
            "issue",
            "run-messages",
            "abcd",
            "--issue",
            "CORD-18",
            "--since",
            "4",
            "--output",
            "table",
        ])
        .expect("run-messages CLI");
        let args = issue_run_messages_args(&messages);
        assert_eq!(args.task_id, "abcd");
        assert_eq!(args.issue.as_deref(), Some("CORD-18"));
        assert_eq!(args.since, 4);
        assert_eq!(args.output, OutputFormat::Table);

        let cancel = Cli::try_parse_from([
            "cordy",
            "issue",
            "cancel-task",
            "11111111-1111-1111-1111-111111111111",
            "--output",
            "json",
        ])
        .expect("cancel-task CLI");
        assert_eq!(
            issue_cancel_task_args(&cancel).task_id,
            "11111111-1111-1111-1111-111111111111"
        );

        let table = format_issue_run_messages_table(&[
            serde_json::json!({
                "seq":1,"type":"text","tool":"","content":"done"
            }),
            serde_json::json!({
                "seq":2,"type":"tool_result","tool":"shell","content":"",
                "output":"x".repeat(81)
            }),
        ]);
        assert!(table.starts_with("SEQ"));
        assert!(table.contains("done"));
        assert!(table.contains("tool_result"));
        assert!(table.contains("xxx..."));
    }

    #[tokio::test]
    async fn issue_run_messages_resolves_scoped_prefix_and_sends_since() {
        let issue_id = "1881a167-4bb6-4602-944b-f40ce4192fe6";
        let task_id = "abcd1234-0000-0000-0000-000000000000";
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || async move {
                    Json(serde_json::json!({"id":issue_id,"identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/1881a167-4bb6-4602-944b-f40ce4192fe6/task-runs",
                get(move || async move { Json(vec![serde_json::json!({"id":task_id})]) }),
            )
            .route(
                "/api/tasks/abcd1234-0000-0000-0000-000000000000/messages",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("since=4"));
                    Json(vec![serde_json::json!({
                        "seq":5,"type":"text","content":"done"
                    })])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "run-messages",
            "abcd",
            "--issue",
            "CORD-18",
            "--since",
            "4",
        ])
        .expect("run-messages CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("run messages");
        let messages: Value = serde_json::from_str(&output.stdout).expect("messages JSON");
        assert_eq!(messages[0]["seq"], 5);
        task.abort();
    }

    #[tokio::test]
    async fn issue_cancel_task_posts_empty_body_and_requires_scope_for_prefix() {
        let task_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new().route(
            "/api/tasks/11111111-1111-1111-1111-111111111111/cancel",
            post(move |Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({}));
                Json(serde_json::json!({"id":task_id,"status":"cancelled"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "cancel-task",
            task_id,
            "--output",
            "table",
        ])
        .expect("cancel-task CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("cancel task");
        assert_eq!(
            output.stdout,
            "Task 11111111-1111-1111-1111-111111111111 -> status=cancelled\n"
        );

        let missing_scope = Cli::try_parse_from(["cordy", "issue", "cancel-task", "abcd"])
            .expect("short cancel CLI");
        let error = run_with_input(
            &missing_scope,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("short task prefix requires issue");
        assert!(error.to_string().contains("require --issue"));
        task.abort();
    }

    #[test]
    fn issue_usage_parser_and_number_format_match_go() {
        let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18", "--output", "json"])
            .expect("usage CLI");
        let args = issue_usage_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(args.output, OutputFormat::Json);
        assert_eq!(format_metadata_value(Some(&serde_json::json!(42.0))), "42");
        assert_eq!(
            format_metadata_value(Some(&serde_json::json!(1234567890123_u64))),
            "1234567890123"
        );
        assert_eq!(format_metadata_value(None), "null");
    }

    #[tokio::test]
    async fn issue_usage_resolves_issue_and_renders_aggregate_table() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/usage",
                get(|| async {
                    Json(serde_json::json!({
                        "total_input_tokens":1000,"total_output_tokens":200,
                        "total_cache_read_tokens":300,"total_cache_write_tokens":40,"task_count":2
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18"]).expect("usage CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue usage");
        assert!(output.stdout.starts_with("INPUT_TOKENS"));
        assert!(output.stdout.contains("1000"));
        assert!(output.stdout.contains("300"));
        assert!(output.stdout.contains("2"));
        task.abort();
    }

    #[tokio::test]
    async fn issue_rerun_posts_fresh_task_and_formats_agent_name() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/rerun",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({}));
                    Json(serde_json::json!({"id":"task-1","agent_id":"agent-1","status":"queued"}))
                }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"CodeBot"})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "rerun", "CORD-18", "--output", "table"])
            .expect("rerun CLI");
        assert_eq!(issue_rerun_args(&cli).issue_id, "CORD-18");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("rerun issue");
        assert_eq!(output.stdout, "Re-enqueued task task-1 on agent CodeBot\n");
        assert!(output.stderr.is_empty());
        task.abort();
    }


}
