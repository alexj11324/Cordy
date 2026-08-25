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

    #[test]
    fn issue_list_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "list",
            "--output",
            "json",
            "--full-id",
            "--status",
            "custom_status",
            "--priority",
            "urgent",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--project",
            "abcd",
            "--metadata",
            "ready=true",
            "--metadata",
            "score=42",
            "--limit",
            "20",
            "--offset",
            "5",
            "--sort",
            "created_at",
            "--direction",
            "DESC",
        ])
        .expect("issue list CLI");
        let args = issue_list_args(&cli);
        assert_eq!(args.output, OutputFormat::Json);
        assert!(args.full_id);
        assert_eq!(args.status.as_deref(), Some("custom_status"));
        assert_eq!(args.priority.as_deref(), Some("urgent"));
        assert_eq!(args.project.as_deref(), Some("abcd"));
        assert_eq!(
            args.metadata,
            vec![String::from("ready=true"), String::from("score=42")]
        );
        assert_eq!((args.limit, args.offset), (20, 5));
        assert_eq!(args.sort.as_deref(), Some("created_at"));
        assert_eq!(args.direction.as_deref(), Some("DESC"));
    }

    #[test]
    fn issue_list_metadata_filter_infers_primitives_and_rejects_duplicates() {
        let encoded = build_metadata_filter(&[
            "ready=true".into(),
            "score=42".into(),
            "forced=\"42\"".into(),
            "label=alpha".into(),
        ])
        .expect("metadata filter");
        let filter: Value = serde_json::from_str(&encoded).expect("metadata JSON");
        assert_eq!(filter["ready"], Value::Bool(true));
        assert_eq!(filter["score"], 42);
        assert_eq!(filter["forced"], "42");
        assert_eq!(filter["label"], "alpha");

        let error = build_metadata_filter(&["ready=true".into(), "ready=false".into()])
            .expect_err("duplicate metadata key");
        assert!(error.to_string().contains("given more than once"));
        let error =
            build_metadata_filter(&["missing-separator".into()]).expect_err("metadata key=value");
        assert!(error.to_string().contains("key=value form"));
    }

    #[test]
    fn issue_list_has_more_uses_offset_and_returned_count() {
        assert!(issue_list_has_more(1, 1, 3));
        assert!(!issue_list_has_more(1, 2, 3));
        assert!(issue_list_has_more(0, 0, 1));
    }

    #[test]
    fn issue_list_table_matches_go_columns_full_id_dates_and_actor_fallback() {
        let issues = vec![serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "identifier": "CORD-18",
            "title": "Migrate CLI",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "agent",
            "assignee_id": "22222222-2222-2222-2222-222222222222",
            "start_date": "2026-08-23T10:11:12Z",
            "due_date": "2026-08-30T00:00:00Z"
        })];
        let actors = IssueActorNames(HashMap::from([(
            "agent:22222222-2222-2222-2222-222222222222".into(),
            "CordyBot".into(),
        )]));
        let table = format_issue_list_table(&issues, true, &actors);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("ID"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("11111111-1111-1111-1111-111111111111"));
        assert!(table.contains("agent:CordyBot"));
        assert!(table.contains("2026-08-23"));
        assert!(table.contains("2026-08-30"));

        let fallback = format_issue_list_table(&issues, false, &IssueActorNames::default());
        assert!(fallback.contains("agent:22222222-2222-2222-2222-222222222222"));
        assert!(!fallback.lines().next().unwrap_or_default().contains(" ID "));
    }

    #[tokio::test]
    async fn issue_list_resolves_filters_and_sends_go_query_and_json_envelope() {
        let captured = Arc::new(Mutex::new(None::<String>));
        let captured_by_issues = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(serde_json::json!([{
                        "user_id": "11111111-1111-1111-1111-111111111111",
                        "name": "Ada Lovelace",
                        "email": "ada@example.com"
                    }]))
                }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/projects",
                get(|| async {
                    Json(serde_json::json!({
                        "projects": [{
                            "id": "abcd0000-0000-0000-0000-000000000000",
                            "title": "Rust migration",
                            "status": "active"
                        }]
                    }))
                }),
            )
            .route(
                "/api/issues",
                get(move |request: Request| {
                    let captured = Arc::clone(&captured_by_issues);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        *captured.lock().expect("capture query") =
                            request.uri().query().map(Into::into);
                        Json(serde_json::json!({
                            "issues": [{
                                "id": "issue-1",
                                "identifier": "CORD-18",
                                "title": "Migrate CLI"
                            }],
                            "total": 3
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "list",
            "--output",
            "json",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--assignee",
            "Ada",
            "--project",
            "abcd",
            "--metadata",
            "ready=true",
            "--limit",
            "2",
            "--offset",
            "1",
            "--sort",
            "created_at",
            "--direction",
            "DESC",
        ])
        .expect("issue list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue list");
        let envelope: Value = serde_json::from_str(&output.stdout).expect("list JSON");
        assert_eq!(envelope["total"], 3);
        assert_eq!(envelope["limit"], 2);
        assert_eq!(envelope["offset"], 1);
        assert_eq!(envelope["has_more"], Value::Bool(true));
        assert_eq!(envelope["issues"][0]["identifier"], "CORD-18");

        let query = captured
            .lock()
            .expect("captured query")
            .clone()
            .expect("query");
        let query = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(query["workspace_id"], "workspace-1");
        assert_eq!(query["status"], "custom_status");
        assert_eq!(query["priority"], "high");
        assert_eq!(query["limit"], "2");
        assert_eq!(query["offset"], "1");
        assert_eq!(query["assignee_id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(query["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(query["metadata"], r#"{"ready":true}"#);
        assert_eq!(query["sort"], "created_at");
        assert_eq!(query["direction"], "desc");
        task.abort();
    }

    #[tokio::test]
    async fn issue_list_rejects_invalid_sort_direction_and_conflicting_assignee_flags() {
        let client = ApiClient::new(
            "http://127.0.0.1:1".into(),
            "workspace-1".into(),
            "token".into(),
            String::new(),
            String::new(),
            std::time::Duration::from_secs(1),
            CLIENT_VERSION,
        )
        .expect("client");
        for (argv, expected) in [
            (
                vec!["cordy", "issue", "list", "--sort", "nonsense"],
                "invalid --sort",
            ),
            (
                vec!["cordy", "issue", "list", "--direction", "desc"],
                "--direction requires --sort",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--sort",
                    "created_at",
                    "--direction",
                    "sideways",
                ],
                "invalid --direction",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--sort",
                    "position",
                    "--direction",
                    "asc",
                ],
                "--direction requires --sort",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--assignee",
                    "Ada",
                    "--assignee-id",
                    "11111111-1111-1111-1111-111111111111",
                ],
                "mutually exclusive",
            ),
        ] {
            let cli = Cli::try_parse_from(argv).expect("CLI");
            let error = build_issue_list_query(&client, "workspace-1", issue_list_args(&cli))
                .await
                .expect_err("validation error");
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn issue_get_parser_defaults_to_json_and_accepts_only_one_reference() {
        let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Get { id, output },
            }) => {
                assert_eq!(id, "CORD-18");
                assert_eq!(output, OutputFormat::Json);
            }
            _ => panic!("expected issue get"),
        }
        assert!(Cli::try_parse_from(["cordy", "issue", "get"]).is_err());
        assert!(Cli::try_parse_from(["cordy", "issue", "get", "A-1", "B-2"]).is_err());
        assert!(
            Cli::try_parse_from(["cordy", "issue", "get", "CORD-18", "--output", "table"]).is_ok()
        );
    }

    #[tokio::test]
    async fn issue_ref_rejects_short_uuid_and_invalid_inputs_without_http() {
        let client = ApiClient::new(
            "http://127.0.0.1:1".into(),
            "workspace-1".into(),
            "token".into(),
            String::new(),
            String::new(),
            std::time::Duration::from_millis(50),
            CLIENT_VERSION,
        )
        .expect("client");
        for input in ["1881", "1881-a167", "1852"] {
            let error = resolve_issue_ref(&client, input)
                .await
                .expect_err("short prefix");
            assert!(error.to_string().contains("short UUID prefix"));
            assert!(error.to_string().contains("MUL-123"));
        }
        let error = resolve_issue_ref(&client, "not-an-id")
            .await
            .expect_err("invalid ref");
        assert!(error
            .to_string()
            .contains("not a recognized issue reference"));
        assert!(!error.to_string().contains("short UUID prefix"));
    }

    #[tokio::test]
    async fn issue_get_resolves_key_then_fetches_canonical_issue() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let first_hits = Arc::clone(&hits);
        let second_hits = Arc::clone(&hits);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || {
                    let hits = Arc::clone(&first_hits);
                    async move {
                        hits.lock().expect("hits").push("CORD-18".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18",
                            "title": "Resolver response"
                        }))
                    }
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111",
                get(move |request: Request| {
                    let hits = Arc::clone(&second_hits);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        hits.lock().expect("hits").push("canonical".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18",
                            "title": "Canonical issue",
                            "description": "Full details"
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue get");
        let issue: Value = serde_json::from_str(&output.stdout).expect("issue JSON");
        assert_eq!(issue["title"], "Canonical issue");
        assert_eq!(issue["description"], "Full details");
        assert_eq!(
            *hits.lock().expect("hits"),
            vec![String::from("CORD-18"), String::from("canonical")]
        );
        task.abort();
    }

    #[test]
    fn issue_get_table_matches_go_detail_columns() {
        let issue = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "identifier": "CORD-18",
            "title": "Migrate get",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "member",
            "assignee_id": "22222222-2222-2222-2222-222222222222",
            "start_date": "2026-08-24T10:00:00Z",
            "due_date": "2026-08-31T10:00:00Z",
            "description": "Preserve the complete description"
        });
        let actors = IssueActorNames(HashMap::from([(
            "member:22222222-2222-2222-2222-222222222222".into(),
            "Ada".into(),
        )]));
        let table = format_issue_get_table(&issue, &actors);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("DESCRIPTION"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("member:Ada"));
        assert!(table.contains("2026-08-24"));
        assert!(table.contains("2026-08-31"));
        assert!(table.contains("Preserve the complete description"));
    }

    #[test]
    fn issue_pull_requests_parser_supports_go_name_alias_and_defaults() {
        for name in ["pull-requests", "prs"] {
            let cli = Cli::try_parse_from(["cordy", "issue", name, "CORD-18"])
                .expect("pull requests CLI");
            match cli.command {
                Command::Issue(IssueArgs {
                    command: IssueCommand::PullRequests { id, output },
                }) => {
                    assert_eq!(id, "CORD-18");
                    assert_eq!(output, OutputFormat::Table);
                }
                _ => panic!("expected issue pull-requests"),
            }
        }
        assert!(Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-requests",
            "CORD-18",
            "--output",
            "json"
        ])
        .is_ok());
    }

    #[tokio::test]
    async fn issue_pull_requests_resolves_issue_and_preserves_json_wrapper() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let resolve_hits = Arc::clone(&hits);
        let pull_request_hits = Arc::clone(&hits);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || {
                    let hits = Arc::clone(&resolve_hits);
                    async move {
                        hits.lock().expect("hits").push("resolve".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18"
                        }))
                    }
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
                get(move |request: Request| {
                    let hits = Arc::clone(&pull_request_hits);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        hits.lock().expect("hits").push("pull-requests".into());
                        Json(serde_json::json!({
                            "pull_requests": [{
                                "number": 42,
                                "state": "open",
                                "title": "Rust CLI",
                                "url": "https://github.example/pr/42"
                            }],
                            "count": 1
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "prs", "CORD-18", "--output", "json"])
            .expect("pull requests CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("pull requests");
        let result: Value = serde_json::from_str(&output.stdout).expect("pull request JSON");
        assert_eq!(result["count"], 1);
        assert_eq!(result["pull_requests"][0]["number"], 42);
        assert_eq!(
            *hits.lock().expect("hits"),
            vec![String::from("resolve"), String::from("pull-requests")]
        );
        task.abort();
    }

    #[test]
    fn issue_pull_requests_table_uses_url_then_html_url_fallback() {
        let result = serde_json::json!({
            "pull_requests": [
                {
                    "number": 42,
                    "state": "open",
                    "title": "Direct URL",
                    "url": "https://github.example/pr/42",
                    "html_url": "https://ignored.example/pr/42"
                },
                {
                    "number": 43,
                    "state": "merged",
                    "title": "Fallback URL",
                    "html_url": "https://github.example/pr/43"
                }
            ]
        });
        let table = format_issue_pull_requests_table(&result);
        assert!(table.starts_with("NUMBER"));
        assert!(table.contains("Direct URL"));
        assert!(table.contains("https://github.example/pr/42"));
        assert!(!table.contains("https://ignored.example/pr/42"));
        assert!(table.contains("Fallback URL"));
        assert!(table.contains("https://github.example/pr/43"));
    }

    #[test]
    fn issue_pull_request_attach_parser_requires_url_and_matches_go_flags() {
        assert!(
            Cli::try_parse_from(["cordy", "issue", "pull-request", "attach", "CORD-18"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "https://github.com/owner/repo/pull/42",
            "--title",
            "Rust CLI",
            "--state",
            "open",
            "--branch",
            "cli",
            "--head-sha",
            "abc123",
            "--output",
            "json",
        ])
        .expect("attach CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::PullRequest(IssuePullRequestArgs {
                        command: IssuePullRequestCommand::Attach(args),
                    }),
            }) => {
                assert_eq!(args.issue_id, "CORD-18");
                assert_eq!(args.url, "https://github.com/owner/repo/pull/42");
                assert_eq!(args.title.as_deref(), Some("Rust CLI"));
                assert_eq!(args.state.as_deref(), Some("open"));
                assert_eq!(args.branch.as_deref(), Some("cli"));
                assert_eq!(args.head_sha.as_deref(), Some("abc123"));
                assert_eq!(args.output, OutputFormat::Json);
            }
            _ => panic!("expected issue pull-request attach"),
        }
    }

    #[tokio::test]
    async fn issue_pull_request_attach_rejects_empty_url_with_go_guidance() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "",
        ])
        .expect("empty URL reaches runtime validation");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("empty URL");
        assert_eq!(
            error.to_string(),
            "--url is required (https://github.com/{owner}/{repo}/pull/{number})"
        );
    }

    #[tokio::test]
    async fn issue_pull_request_attach_posts_trimmed_url_and_optional_metadata() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18"
                    }))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_handler);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture body") = Some(body);
                        Json(serde_json::json!({
                            "pull_request": {
                                "number": 42,
                                "state": "open",
                                "title": "Rust CLI",
                                "url": "https://github.com/owner/repo/pull/42"
                            }
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "  https://github.com/owner/repo/pull/42  ",
            "--title",
            "Rust CLI",
            "--state",
            "   ",
            "--branch",
            "cli",
            "--output",
            "json",
        ])
        .expect("attach CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("attach pull request");
        let result: Value = serde_json::from_str(&output.stdout).expect("attach JSON");
        assert_eq!(result["pull_request"]["number"], 42);
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("body");
        assert_eq!(body["url"], "https://github.com/owner/repo/pull/42");
        assert_eq!(body["title"], "Rust CLI");
        assert_eq!(body["branch"], "cli");
        assert!(body.get("state").is_none());
        assert!(body.get("head_sha").is_none());
        task.abort();
    }

    #[test]
    fn issue_children_parser_supports_alias_output_and_full_id_flag() {
        for name in ["children", "subissues"] {
            let cli = Cli::try_parse_from([
                "cordy",
                "issue",
                name,
                "CORD-18",
                "--output",
                "json",
                "--full-id",
            ])
            .expect("children CLI");
            match cli.command {
                Command::Issue(IssueArgs {
                    command:
                        IssueCommand::Children {
                            id,
                            output,
                            full_id,
                        },
                }) => {
                    assert_eq!(id, "CORD-18");
                    assert_eq!(output, OutputFormat::Json);
                    assert!(full_id);
                }
                _ => panic!("expected issue children"),
            }
        }
    }

    #[test]
    fn issue_children_sort_group_and_terminal_count_match_go() {
        let mut children = vec![
            serde_json::json!({"id":"u1","identifier":"CORD-4","stage":null,"status":"todo"}),
            serde_json::json!({"id":"s2a","identifier":"CORD-2","stage":2,"status":"cancelled","status_category":"cancelled"}),
            serde_json::json!({"id":"s1a","identifier":"CORD-1","stage":1,"status":"gate_approved","status_category":"done"}),
            serde_json::json!({"id":"s2b","identifier":"CORD-3","stage":2,"status":"in_progress","status_category":"in_progress"}),
            serde_json::json!({"id":"u2","identifier":"CORD-5","status":"done"}),
        ];
        children.sort_by_key(|child| child_stage(child).map_or((true, 0), |stage| (false, stage)));
        let identifiers = children
            .iter()
            .map(|child| value_string(child, "identifier"))
            .collect::<Vec<_>>();
        assert_eq!(
            identifiers,
            vec![
                String::from("CORD-1"),
                String::from("CORD-2"),
                String::from("CORD-3"),
                String::from("CORD-4"),
                String::from("CORD-5"),
            ]
        );
        let grouped = serde_json::to_value(group_issue_children(&children)).expect("group JSON");
        assert_eq!(grouped["total"], 5);
        assert_eq!(grouped["stages"][0]["stage"], 1);
        assert_eq!(grouped["stages"][0]["total"], 1);
        assert_eq!(grouped["stages"][0]["done"], 1);
        assert_eq!(grouped["stages"][1]["stage"], 2);
        assert_eq!(grouped["stages"][1]["total"], 2);
        assert_eq!(grouped["stages"][1]["done"], 1);
        assert_eq!(grouped["unstaged"].as_array().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn issue_children_resolves_parent_and_fetches_children_endpoint() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18"
                    }))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/children",
                get(|request: Request| async move {
                    assert_eq!(request.headers()["authorization"], "Bearer token-1");
                    Json(serde_json::json!({
                        "issues": [
                            {"id":"child-2","identifier":"CORD-20","stage":2,"status":"todo"},
                            {"id":"child-1","identifier":"CORD-19","stage":1,"status":"done"}
                        ]
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["cordy", "issue", "children", "CORD-18", "--output", "json"])
                .expect("children CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("children");
        let grouped: Value = serde_json::from_str(&output.stdout).expect("children JSON");
        assert_eq!(grouped["stages"][0]["stage"], 1);
        assert_eq!(grouped["stages"][1]["stage"], 2);
        assert_eq!(grouped["stages"][0]["done"], 1);
        task.abort();
    }

    #[test]
    fn issue_children_table_renders_stage_key_and_actor() {
        let children = vec![serde_json::json!({
            "id": "child-1",
            "identifier": "CORD-19",
            "stage": 1,
            "title": "First barrier",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "agent",
            "assignee_id": "agent-1"
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CordyBot".into())]));
        let table = format_issue_children_table(&children, &actors);
        assert!(table.starts_with("STAGE"));
        assert!(table.contains("CORD-19"));
        assert!(table.contains("First barrier"));
        assert!(table.contains("agent:CordyBot"));
    }

    #[test]
    fn issue_create_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "New issue",
            "--description",
            "Line 1\\nLine 2",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--parent",
            "CORD-1",
            "--stage",
            "2",
            "--project",
            "abcd",
            "--start-date",
            "2026-08-24",
            "--due-date",
            "2026-08-31",
            "--allow-duplicate",
            "--attachment",
            "one.png",
            "--attachment",
            "two.png",
            "--attachment-id",
            "attachment-1",
            "--output",
            "table",
        ])
        .expect("issue create CLI");
        let args = issue_create_args(&cli);
        assert_eq!(args.title.as_deref(), Some("New issue"));
        assert_eq!(args.description.as_deref(), Some("Line 1\\nLine 2"));
        assert_eq!(args.status.as_deref(), Some("custom_status"));
        assert_eq!(args.priority.as_deref(), Some("high"));
        assert_eq!(args.stage, Some(2));
        assert_eq!(args.start_date.as_deref(), Some("2026-08-24"));
        assert_eq!(args.due_date.as_deref(), Some("2026-08-31"));
        assert!(args.allow_duplicate);
        assert_eq!(args.attachment.len(), 2);
        assert_eq!(args.attachment_id, vec![String::from("attachment-1")]);
        assert_eq!(args.output, OutputFormat::Table);
    }

    #[test]
    fn issue_create_description_modes_preserve_go_input_semantics() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let inline = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "one\\ntwo",
        ])
        .expect("inline CLI");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&inline),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("inline description"),
            Some("one\ntwo".into())
        );

        let stdin = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description-stdin",
        ])
        .expect("stdin CLI");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&stdin),
                &environment,
                &mut Cursor::new(b"literal\\nvalue\n".to_vec())
            )
            .expect("stdin description"),
            Some("literal\\nvalue".into())
        );

        let conflict = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "text",
            "--description-stdin",
        ])
        .expect("conflict reaches runtime");
        let error = resolve_issue_create_description(
            issue_create_args(&conflict),
            &environment,
            &mut Cursor::new(b"stdin".to_vec()),
        )
        .expect_err("mutually exclusive sources");
        assert!(error.to_string().contains("mutually exclusive"));

        let empty_file = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "text",
            "--description-file",
            "",
        ])
        .expect("empty file flag reaches runtime");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&empty_file),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("empty file value is unset"),
            Some("text".into())
        );
    }

    #[test]
    fn issue_create_local_link_guard_is_agent_only_and_ignores_code() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let artifact = cwd.path().join("artifact.png");
        fs::write(&artifact, b"image").expect("artifact");
        let markdown = format!("[result]({})", artifact.display());

        let human = Environment::for_test(home.path().into(), cwd.path().into());
        let remediation = "Deliver it with `cordy issue create --attachment <path>`.";
        guard_issue_description_local_links(&markdown, &human, remediation)
            .expect("human links are allowed");

        let mut agent = Environment::for_test(home.path().into(), cwd.path().into());
        agent.set("CORDY_AGENT_ID", "agent-1");
        let error = guard_issue_description_local_links(&markdown, &agent, remediation)
            .expect_err("agent local link");
        assert!(error.to_string().contains("runtime-local path"));
        assert!(error.to_string().contains("--attachment"));
        guard_issue_description_local_links(
            &format!(
                "`[result]({})`\n```md\n[result]({})\n```",
                artifact.display(),
                artifact.display()
            ),
            &agent,
            remediation,
        )
        .expect("code spans and fences are ignored");
    }

    #[tokio::test]
    async fn issue_create_resolves_references_and_sends_complete_body() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_issue = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-10",
                get(|| async { Json(serde_json::json!({"id":"parent-uuid","identifier":"CORD-10"})) }),
            )
            .route(
                "/api/projects",
                get(|| async { Json(serde_json::json!({"projects":[{"id":"abcd0000-0000-0000-0000-000000000000","title":"Migration","status":"active"}]})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([{"user_id":"11111111-1111-1111-1111-111111111111","name":"Ada","email":"ada@example.com"}])) }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_issue);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture issue") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"issue-uuid","identifier":"CORD-18","title":body["title"],
                            "status":body["status"],"priority":body["priority"]
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
        environment.set("CORDY_QUICK_CREATE_TASK_ID", "task-quick");
        environment.set(
            "CORDY_QUICK_CREATE_ATTACHMENT_IDS",
            r#"["attachment-env","attachment-shared"]"#,
        );
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "New issue",
            "--description",
            "Line 1\\nLine 2",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--parent",
            "CORD-10",
            "--stage",
            "2",
            "--project",
            "abcd",
            "--assignee",
            "Ada",
            "--start-date",
            "2026-08-24",
            "--due-date",
            "2026-08-31",
            "--allow-duplicate",
            "--attachment-id",
            "attachment-flag",
            "--attachment-id",
            "attachment-shared",
        ])
        .expect("create CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create issue");
        let issue: Value = serde_json::from_str(&output.stdout).expect("issue JSON");
        assert_eq!(issue["identifier"], "CORD-18");
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["title"], "New issue");
        assert_eq!(body["description"], "Line 1\nLine 2");
        assert_eq!(body["status"], "custom_status");
        assert_eq!(body["priority"], "high");
        assert_eq!(body["parent_issue_id"], "parent-uuid");
        assert_eq!(body["stage"], 2);
        assert_eq!(body["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(body["assignee_type"], "member");
        assert_eq!(body["assignee_id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(body["start_date"], "2026-08-24");
        assert_eq!(body["due_date"], "2026-08-31");
        assert_eq!(body["allow_duplicate"], Value::Bool(true));
        assert_eq!(body["origin_type"], "quick_create");
        assert_eq!(body["origin_id"], "task-quick");
        assert_eq!(
            body["attachment_ids"],
            serde_json::json!(["attachment-flag", "attachment-shared", "attachment-env"])
        );
        task.abort();
    }

    #[tokio::test]
    async fn issue_create_surfaces_active_duplicate_message_verbatim() {
        let expected = "Active duplicate issue exists: CORD-1 Existing (status: in_progress).";
        let app = Router::new().route(
            "/api/issues",
            post(move || async move {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(serde_json::json!({"code":"active_duplicate_issue","error":expected})),
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
        let cli = Cli::try_parse_from(["cordy", "issue", "create", "--title", "Duplicate"])
            .expect("create CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("duplicate");
        assert_eq!(error.to_string(), expected);
        task.abort();
    }

    #[tokio::test]
    async fn issue_create_prevalidates_attachments_and_treats_upload_failure_as_partial_success() {
        let issue_posts = Arc::new(Mutex::new(0_usize));
        let uploads = Arc::new(Mutex::new(0_usize));
        let issue_posts_by_handler = Arc::clone(&issue_posts);
        let uploads_by_handler = Arc::clone(&uploads);
        let app = Router::new()
            .route(
                "/api/issues",
                post(move || {
                    let posts = Arc::clone(&issue_posts_by_handler);
                    async move {
                        *posts.lock().expect("posts") += 1;
                        Json(serde_json::json!({"id":"issue-1","identifier":"CORD-1","title":"With file","status":"todo","priority":"none"}))
                    }
                }),
            )
            .route(
                "/api/upload-file",
                post(move |headers: HeaderMap, _body: axum::body::Bytes| {
                    let uploads = Arc::clone(&uploads_by_handler);
                    async move {
                        *uploads.lock().expect("uploads") += 1;
                        assert!(headers["content-type"].to_str().expect("content type").starts_with("multipart/form-data; boundary="));
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "upload failed")
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("good.png"), b"image").expect("attachment");
        let external = tempfile::tempdir().expect("external");
        let external_file = external.path().join("bad.png");
        fs::write(&external_file, b"bad").expect("external attachment");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let invalid = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "Invalid",
            "--attachment",
            external_file.to_str().expect("external path"),
        ])
        .expect("invalid attachment CLI");
        let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("external attachment");
        assert!(error.to_string().contains("--allow-external-file"));
        assert_eq!(*issue_posts.lock().expect("posts"), 0);
        assert_eq!(*uploads.lock().expect("uploads"), 0);

        let valid = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "With file",
            "--attachment",
            "good.png",
        ])
        .expect("attachment CLI");
        let output = run_with_input(&valid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("partial success");
        assert_eq!(*issue_posts.lock().expect("posts"), 1);
        assert_eq!(*uploads.lock().expect("uploads"), 1);
        assert!(output.stderr.contains("issue already created, CORD-1"));
        task.abort();
    }

    #[test]
    fn issue_update_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--title",
            "Updated",
            "--description",
            "one\\ntwo",
            "--status",
            "in_review",
            "--priority",
            "urgent",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--project",
            "",
            "--start-date",
            "",
            "--due-date",
            "2026-08-31",
            "--parent",
            "",
            "--stage",
            "2",
            "--position",
            "1.5",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("issue update CLI");
        let args = issue_update_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.title.as_deref(), Some("Updated"));
        assert_eq!(args.description.as_deref(), Some("one\\ntwo"));
        assert_eq!(args.project.as_deref(), Some(""));
        assert_eq!(args.start_date.as_deref(), Some(""));
        assert_eq!(args.parent.as_deref(), Some(""));
        assert_eq!(args.stage, Some(2));
        assert_eq!(args.position, Some(1.5));
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Table);
    }

    #[tokio::test]
    async fn issue_update_rejects_invalid_enums_before_client_creation() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "update", "CORD-18", "--priority", "P1"])
            .expect("update CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("priority is rejected locally");
        assert!(error.to_string().contains("valid values"));
    }

    #[tokio::test]
    async fn issue_update_resolves_references_and_puts_only_changed_fields() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/issues/PARENT-1",
                get(|| async { Json(serde_json::json!({"id":"parent-uuid","identifier":"CORD-1"})) }),
            )
            .route(
                "/api/projects",
                get(|| async { Json(serde_json::json!({"projects":[{"id":"abcd0000-0000-0000-0000-000000000000","title":"Migration","status":"active"}]})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([{"user_id":"member-uuid","name":"Ada","email":"ada@example.com"}])) }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues/issue-uuid",
                put(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture update") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"issue-uuid","identifier":"CORD-18","title":body["title"],
                            "status":body["status"],"priority":body["priority"]
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
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--title",
            "Updated",
            "--description",
            "one\\ntwo",
            "--status",
            "in_review",
            "--priority",
            "urgent",
            "--assignee",
            "Ada",
            "--project",
            "abcd",
            "--start-date",
            "",
            "--due-date",
            "2026-08-31",
            "--parent",
            "PARENT-1",
            "--stage",
            "2",
            "--position",
            "1.5",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("update CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update issue");
        assert!(output.stdout.starts_with("KEY"));
        assert!(output.stdout.contains("CORD-18"));
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["title"], "Updated");
        assert_eq!(body["description"], "one\ntwo");
        assert_eq!(body["status"], "in_review");
        assert_eq!(body["priority"], "urgent");
        assert_eq!(body["assignee_type"], "member");
        assert_eq!(body["assignee_id"], "member-uuid");
        assert_eq!(body["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(body["start_date"], "");
        assert_eq!(body["due_date"], "2026-08-31");
        assert_eq!(body["parent_issue_id"], "parent-uuid");
        assert_eq!(body["stage"], 2);
        assert_eq!(body["position"], 1.5);
        assert_eq!(body["suppress_run"], true);
        task.abort();
    }

    #[tokio::test]
    async fn issue_update_supports_explicit_clears_and_rejects_no_changes() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture update") = Some(body);
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

        let clear = Cli::try_parse_from([
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--description",
            "",
            "--project",
            "",
            "--parent",
            "",
        ])
        .expect("clear CLI");
        run_with_input(&clear, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("clear fields");
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["description"], "");
        assert_eq!(body["project_id"], Value::Null);
        assert_eq!(body["parent_issue_id"], Value::Null);

        let no_changes =
            Cli::try_parse_from(["cordy", "issue", "update", "CORD-18"]).expect("no changes CLI");
        let error = run_with_input(
            &no_changes,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("no fields");
        assert!(error.to_string().contains("no fields to update"));
        task.abort();
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

    #[test]
    fn label_parser_and_tables_match_go_registry_contract() {
        let create = Cli::try_parse_from([
            "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000", "--output", "table",
        ])
        .expect("label create CLI");
        let Command::Label(LabelArgs {
            command: LabelCommand::Create(args),
        }) = &create.command
        else {
            panic!("expected label create");
        };
        assert_eq!(args.name.as_deref(), Some("Bug"));
        assert_eq!(args.color.as_deref(), Some("#ff0000"));
        assert_eq!(args.output, OutputFormat::Table);

        let label = serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","name":"Bug","color":"#ff0000",
            "created_at":"2026-08-24T12:34:56Z"
        });
        let short = format_workspace_label_table(std::slice::from_ref(&label), false);
        assert!(short.starts_with("ID"));
        assert!(short.contains("11111111"));
        assert!(short.contains("2026-08-24"));
        let details = format_label_result(&label, OutputFormat::Table, true).expect("details");
        assert!(details.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[tokio::test]
    async fn label_create_update_and_delete_use_go_http_and_output_contracts() {
        let label_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new()
            .route(
                "/api/labels",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"name":"Bug","color":"#ff0000"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Bug","color":"#ff0000"
                    }))
                }),
            )
            .route(
                "/api/labels/11111111-1111-1111-1111-111111111111",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"name":"Defect"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Defect","color":"#ff0000"
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
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

        let create = Cli::try_parse_from([
            "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000",
        ])
        .expect("label create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create label");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("created JSON")["name"],
            "Bug"
        );

        let update = Cli::try_parse_from([
            "cordy", "label", "update", label_id, "--name", "Defect", "--output", "table",
        ])
        .expect("label update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update label");
        assert!(updated.stdout.contains("Defect"));

        let delete =
            Cli::try_parse_from(["cordy", "label", "delete", label_id, "--output", "json"])
                .expect("label delete CLI");
        let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete label");
        let deleted: Value = serde_json::from_str(&deleted.stdout).expect("deleted JSON");
        assert_eq!(deleted["id"], label_id);
        assert_eq!(deleted["deleted"], true);
        task.abort();
    }

    #[test]
    fn project_read_parser_and_tables_match_go_registry_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "list",
            "--status",
            "in_progress",
            "--full-id",
            "--output",
            "json",
        ])
        .expect("project list CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::List {
                    output,
                    full_id,
                    status,
                },
        }) = &cli.command
        else {
            panic!("expected project list");
        };
        assert_eq!(*output, OutputFormat::Json);
        assert!(*full_id);
        assert_eq!(status.as_deref(), Some("in_progress"));

        let project = serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","title":"Migration",
            "status":"in_progress","lead_type":"member","lead_id":"member-1",
            "created_at":"2026-08-24T12:34:56Z","description":"Rust port"
        });
        let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
        let list = format_project_list_table(std::slice::from_ref(&project), &actors, false);
        assert!(list.starts_with("ID"));
        assert!(list.contains("11111111"));
        assert!(list.contains("Migration"));
        assert!(list.contains("member:Ada"));
        assert!(list.contains("2026-08-24"));
        let details = format_project_details_table(&project, &actors);
        assert!(details.contains("11111111-1111-1111-1111-111111111111"));
        assert!(details.contains("Rust port"));
    }

    #[tokio::test]
    async fn project_list_sends_workspace_status_and_preserves_json_array() {
        let app = Router::new().route(
            "/api/projects",
            get(|request: Request| async move {
                let query = request.uri().query().unwrap_or_default();
                assert!(query.contains("workspace_id=workspace-1"));
                assert!(query.contains("status=in_progress"));
                Json(serde_json::json!({
                    "projects":[{"id":"project-1","title":"Migration","status":"in_progress"}]
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
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "list",
            "--status",
            "in_progress",
            "--output",
            "json",
        ])
        .expect("project list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list projects");
        let projects: Value = serde_json::from_str(&output.stdout).expect("projects JSON");
        assert_eq!(projects[0]["title"], "Migration");
        task.abort();
    }

    #[tokio::test]
    async fn project_get_resolves_prefix_and_reports_attached_resources() {
        let project_id = "abcd1234-0000-0000-0000-000000000000";
        let app = Router::new()
            .route(
                "/api/projects",
                get(move || async move {
                    Json(serde_json::json!({
                        "projects":[{"id":project_id,"title":"Migration","status":"planned"}]
                    }))
                }),
            )
            .route(
                "/api/projects/abcd1234-0000-0000-0000-000000000000",
                get(move || async move {
                    Json(serde_json::json!({
                        "id":project_id,"title":"Migration","status":"planned",
                        "description":"Rust port","resource_count":2
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
        let cli = Cli::try_parse_from(["cordy", "project", "get", "abcd", "--output", "table"])
            .expect("project get CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get project");
        assert!(output.stdout.contains("Migration"));
        assert!(output.stderr.contains("2 resource(s) attached"));
        assert!(output.stderr.contains(project_id));
        task.abort();
    }

    #[test]
    fn project_mutation_parser_and_status_validation_match_go_contract() {
        let create = Cli::try_parse_from([
            "cordy",
            "project",
            "create",
            "--title",
            "Migration",
            "--status",
            "planned",
            "--repo",
            "https://github.com/acme/one",
            "--repo",
            "https://github.com/acme/two",
        ])
        .expect("project create CLI");
        let Command::Project(ProjectArgs {
            command: ProjectCommand::Create(args),
        }) = &create.command
        else {
            panic!("expected project create");
        };
        assert_eq!(args.repo.len(), 2);
        for status in PROJECT_STATUSES {
            validate_project_status(status).expect("valid project status");
        }
        assert!(validate_project_status("active")
            .expect_err("invalid status")
            .to_string()
            .contains("planned"));

        let update = Cli::try_parse_from([
            "cordy",
            "project",
            "update",
            "11111111-1111-1111-1111-111111111111",
            "--start-date=",
            "--due-date=",
        ])
        .expect("project update clears");
        let Command::Project(ProjectArgs {
            command: ProjectCommand::Update(args),
        }) = &update.command
        else {
            panic!("expected project update");
        };
        assert_eq!(args.start_date.as_deref(), Some(""));
        assert_eq!(args.due_date.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn project_create_bundles_repos_and_status_updates_return_go_outputs() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new()
            .route(
                "/api/projects",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["title"], "Migration");
                    assert_eq!(body["status"], "planned");
                    assert_eq!(body["resources"].as_array().expect("resources").len(), 2);
                    assert_eq!(
                        body["resources"][0]["resource_ref"]["url"],
                        "https://github.com/acme/one"
                    );
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "title":"Migration","status":"planned"
                    }))
                }),
            )
            .route(
                "/api/projects/11111111-1111-1111-1111-111111111111",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"status":"completed"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "title":"Migration","status":"completed"
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
        let create = Cli::try_parse_from([
            "cordy",
            "project",
            "create",
            "--title",
            "Migration",
            "--status",
            "planned",
            "--repo",
            "https://github.com/acme/one",
            "--repo",
            "https://github.com/acme/two",
        ])
        .expect("project create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create project");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("project JSON")["id"],
            project_id
        );

        let status = Cli::try_parse_from([
            "cordy",
            "project",
            "status",
            project_id,
            "completed",
            "--output",
            "table",
        ])
        .expect("project status CLI");
        let updated = run_with_input(&status, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update project status");
        assert!(updated.stdout.is_empty());
        assert_eq!(
            updated.stderr,
            "Project Migration status changed to completed.\n"
        );
        task.abort();
    }

    #[test]
    fn project_resource_add_parser_and_ref_shortcuts_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            "11111111-1111-1111-1111-111111111111",
            "--url",
            "https://github.com/acme/cordy",
            "--ref",
            "2024",
            "--default-branch-hint",
            "main",
            "--label",
            "Cordy",
        ])
        .expect("project resource add CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) = &cli.command
        else {
            panic!("expected project resource add");
        };
        assert_eq!(args.resource_type, "github_repo");
        assert_eq!(
            build_project_resource_add_ref(args).expect("github ref"),
            serde_json::json!({
                "url":"https://github.com/acme/cordy",
                "ref":"2024",
                "default_branch_hint":"main"
            })
        );

        let generic = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            "11111111-1111-1111-1111-111111111111",
            "--type",
            "documentation",
            "--ref",
            r#"{"url":"https://docs.example.com"}"#,
        ])
        .expect("generic project resource CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) = &generic.command
        else {
            panic!("expected generic project resource add");
        };
        assert_eq!(
            build_project_resource_add_ref(args).expect("generic ref"),
            serde_json::json!({"url":"https://docs.example.com"})
        );
    }

    #[tokio::test]
    async fn project_resource_list_and_add_use_go_http_and_output_contracts() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let resource_id = "22222222-2222-2222-2222-222222222222";
        let app = Router::new().route(
            "/api/projects/11111111-1111-1111-1111-111111111111/resources",
            get(move || async move {
                Json(serde_json::json!({"resources":[{
                    "id":resource_id,"resource_type":"github_repo",
                    "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                    "label":"Cordy"
                }]}))
            })
            .post(|Json(body): Json<Value>| async move {
                assert_eq!(body["resource_type"], "local_directory");
                assert_eq!(body["resource_ref"]["local_path"], "/srv/cordy");
                assert_eq!(body["resource_ref"]["daemon_id"], "daemon-1");
                assert_eq!(body["resource_ref"]["execution_mode"], "worktree");
                Json(serde_json::json!({
                    "id":"33333333-3333-3333-3333-333333333333",
                    "resource_type":"local_directory",
                    "resource_ref":{"local_path":"/srv/cordy","daemon_id":"daemon-1","execution_mode":"worktree"}
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

        let list = Cli::try_parse_from([
            "cordy", "project", "resource", "list", project_id, "--output", "table",
        ])
        .expect("project resource list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list project resources");
        assert!(listed.stdout.contains("22222222"));
        assert!(listed
            .stdout
            .contains("https://github.com/acme/cordy @ main"));
        assert!(listed.stdout.contains("Cordy"));

        let add = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            project_id,
            "--type",
            "local_directory",
            "--local-path",
            "/srv/cordy",
            "--daemon-id",
            "daemon-1",
            "--execution-mode",
            "worktree",
            "--output",
            "table",
        ])
        .expect("project resource add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add project resource");
        assert!(added
            .stdout
            .contains("33333333-3333-3333-3333-333333333333"));
        assert!(added.stdout.contains("/srv/cordy"));
        task.abort();
    }

    #[test]
    fn project_resource_update_rebuilds_opaque_refs_and_supports_clear_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "update",
            "11111111-1111-1111-1111-111111111111",
            "2222",
            "--default-branch-hint",
            "trunk",
            "--clear-label",
            "--position",
            "3",
            "--output",
            "table",
        ])
        .expect("project resource update CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Update(args),
                }),
        }) = &cli.command
        else {
            panic!("expected project resource update");
        };
        assert!(args.clear_label);
        assert_eq!(args.position, Some(3));
        let existing = serde_json::json!({
            "url":"https://github.com/acme/cordy",
            "ref":"main",
            "default_branch_hint":"main"
        });
        assert_eq!(
            build_project_resource_update_ref(args, "github_repo", existing.as_object())
                .expect("update ref")
                .expect("changed ref"),
            serde_json::json!({
                "url":"https://github.com/acme/cordy",
                "ref":"main",
                "default_branch_hint":"trunk"
            })
        );
    }

    #[tokio::test]
    async fn project_resource_update_and_remove_use_prefix_put_and_delete_contracts() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let resource_id = "22222222-2222-2222-2222-222222222222";
        let resource_path =
            "/api/projects/11111111-1111-1111-1111-111111111111/resources/22222222-2222-2222-2222-222222222222";
        let app = Router::new()
            .route(
                "/api/projects/11111111-1111-1111-1111-111111111111/resources",
                get(move || async move {
                    Json(serde_json::json!({"resources":[{
                        "id":resource_id,"resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                        "label":"Cordy"
                    }]}))
                }),
            )
            .route(
                resource_path,
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body["label"], Value::Null);
                    assert_eq!(body["position"], 3);
                    assert_eq!(
                        body["resource_ref"],
                        serde_json::json!({
                            "url":"https://github.com/acme/cordy",
                            "ref":"main",
                            "default_branch_hint":"trunk"
                        })
                    );
                    Json(serde_json::json!({
                        "id":"22222222-2222-2222-2222-222222222222",
                        "resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main","default_branch_hint":"trunk"},
                        "label":""
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
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

        let update = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "update",
            project_id,
            "2222",
            "--default-branch-hint",
            "trunk",
            "--clear-label",
            "--position",
            "3",
            "--output",
            "table",
        ])
        .expect("project resource update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update project resource");
        assert!(updated.stdout.contains(resource_id));
        assert!(updated
            .stdout
            .contains("https://github.com/acme/cordy @ main"));

        let remove = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "remove",
            project_id,
            resource_id,
        ])
        .expect("project resource remove CLI");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove project resource");
        assert!(removed.stdout.is_empty());
        assert_eq!(
            removed.stderr,
            format!("Resource {resource_id} removed from project {project_id}.\n")
        );
        task.abort();
    }

    #[tokio::test]
    async fn attachment_upload_and_download_match_go_file_and_output_contracts() {
        let app = Router::new()
            .route(
                "/api/upload-file",
                post(|request: Request| async move {
                    let content_type = request
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    assert!(content_type.starts_with("multipart/form-data; boundary="));
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("multipart body");
                    let body = String::from_utf8_lossy(&body);
                    assert!(body.contains("task-1"));
                    assert!(body.contains("chart[v2].png"));
                    Json(serde_json::json!({
                        "id":"attachment-1","content_type":"image/png",
                        "markdown_url":"/api/attachments/attachment-1/download"
                    }))
                }),
            )
            .route(
                "/api/attachments/attachment-1",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"attachment-1","filename":"../report.txt",
                        "download_url":"/downloads/report.txt","size_bytes":15
                    }))
                }),
            )
            .route(
                "/downloads/report.txt",
                get(|request: Request| async move {
                    assert!(request.headers().contains_key("authorization"));
                    "attachment body"
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("chart[v2].png"), b"png bytes").expect("upload file");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_test-token");

        let upload = Cli::try_parse_from(["cordy", "attachment", "upload", "chart[v2].png"])
            .expect("attachment upload CLI");
        let uploaded = run_with_input(&upload, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("upload attachment");
        assert_eq!(uploaded.stderr, "Uploaded: chart[v2].png\n");
        let uploaded_json: Value = serde_json::from_str(&uploaded.stdout).expect("upload JSON");
        assert_eq!(uploaded_json["id"], "attachment-1");
        assert_eq!(
            uploaded_json["markdown"],
            r#"![chart\[v2\].png](/api/attachments/attachment-1/download)"#
        );

        let download = Cli::try_parse_from([
            "cordy",
            "attachment",
            "download",
            "attachment-1",
            "-o",
            "attachments",
        ])
        .expect("attachment download CLI");
        let downloaded =
            run_with_input(&download, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("download attachment");
        let destination = cwd.path().join("attachments/report.txt");
        assert_eq!(
            fs::read_to_string(&destination).expect("downloaded file"),
            "attachment body"
        );
        assert!(downloaded
            .stderr
            .contains(destination.to_string_lossy().as_ref()));
        let downloaded_json: Value =
            serde_json::from_str(&downloaded.stdout).expect("download JSON");
        assert_eq!(downloaded_json["filename"], "report.txt");
        assert_eq!(downloaded_json["size"], "15");
        assert!(!downloaded.stdout.contains("../"));
        server.abort();
    }

    #[tokio::test]
    async fn repo_registry_add_remove_and_list_match_go_patch_contracts() {
        let repos = Arc::new(Mutex::new(vec![WorkspaceRepo {
            url: "https://git.example.com/web.git".into(),
            description: "web".into(),
        }]));
        let repos_get = Arc::clone(&repos);
        let repos_patch = Arc::clone(&repos);
        let app = Router::new().route(
            "/api/workspaces/ws-1",
            get(move || {
                let repos = Arc::clone(&repos_get);
                async move {
                    Json(serde_json::json!({
                        "id":"ws-1","repos":repos.lock().expect("repos").clone()
                    }))
                }
            })
            .patch(move |Json(body): Json<Value>| {
                let repos = Arc::clone(&repos_patch);
                async move {
                    let updated: Vec<WorkspaceRepo> =
                        serde_json::from_value(body["repos"].clone()).expect("repo patch body");
                    *repos.lock().expect("repos") = updated.clone();
                    Json(serde_json::json!({"id":"ws-1","repos":updated}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "ws-1");
        environment.set("CORDY_TOKEN", "token-1");

        let add = Cli::try_parse_from([
            "cordy",
            "repo",
            "add",
            "https://git.example.com/api.git",
            "https://git.example.com/api.git",
            "--url",
            "https://git.example.com/web.git",
            "--output",
            "json",
        ])
        .expect("repo add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add repos");
        let added: Value = serde_json::from_str(&added.stdout).expect("add JSON");
        assert_eq!(added["added"].as_array().expect("added").len(), 1);
        assert_eq!(added["repos"].as_array().expect("repos").len(), 2);

        let remove = Cli::try_parse_from([
            "cordy",
            "repo",
            "rm",
            "https://git.example.com/web.git",
            "--output",
            "table",
        ])
        .expect("repo remove alias");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove repo");
        assert!(removed.stdout.starts_with("REMOVED URL"));
        assert!(removed.stdout.contains("web.git"));

        let list = Cli::try_parse_from(["cordy", "repo", "list", "--output", "table"])
            .expect("repo list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list repos");
        assert!(listed.stdout.starts_with("URL"));
        assert!(listed.stdout.contains("api.git"));
        assert!(!listed.stdout.contains("web.git"));
        server.abort();
    }

    #[test]
    fn repo_registry_rejects_empty_duplicate_and_invalid_description_inputs() {
        assert_eq!(
            repo_urls(&[" a ".into()], &["a".into(), "b".into()]).expect("dedupe"),
            vec!["a", "b"]
        );
        assert!(repo_urls(&[], &[])
            .expect_err("missing URL")
            .to_string()
            .contains("at least one"));
        assert!(repo_urls(&[" ".into()], &[])
            .expect_err("empty URL")
            .to_string()
            .contains("cannot be empty"));
        assert!(Cli::try_parse_from([
            "cordy",
            "repo",
            "remove",
            "https://git.example.com/a.git",
            "--description",
            "x"
        ])
        .is_err());
    }

    #[tokio::test]
    async fn repo_checkout_forwards_task_context_and_retries_only_marked_busy() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_handler = Arc::clone(&attempts);
        let app = Router::new().route(
            "/repo/checkout",
            post(move |request: Request| {
                let attempts = Arc::clone(&attempts_handler);
                async move {
                    let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert_eq!(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer mat_checkout")
                    );
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("checkout body");
                    let body: Value = serde_json::from_slice(&body).expect("checkout JSON");
                    assert_eq!(body["url"], "https://github.com/acme/cordy.git");
                    assert_eq!(body["workspace_id"], "ws-1");
                    assert_eq!(body["agent_name"], "Rust Agent");
                    assert_eq!(body["task_id"], "task-1");
                    assert_eq!(body["checkout_mode"], "isolated");
                    assert_eq!(body["ref"], "release/v2");
                    assert_eq!(body["retry_busy"], true);
                    if attempt == 0 {
                        let mut response = axum::response::Response::builder()
                            .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                            .header("X-Cordy-Retryable", "repo-busy")
                            .header("Retry-After", "0")
                            .body(axum::body::Body::from("busy"))
                            .expect("busy response");
                        response
                            .headers_mut()
                            .insert("content-type", "text/plain".parse().expect("content type"));
                        return response;
                    }
                    axum::response::Response::builder()
                        .status(axum::http::StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"path":"/work/cordy","branch_name":"agent/rust/task-1"}"#,
                        ))
                        .expect("success response")
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_DAEMON_PORT", port.to_string());
        environment.set("CORDY_WORKSPACE_ID", "ws-1");
        environment.set("CORDY_AGENT_NAME", "Rust Agent");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_checkout");
        environment.set("CORDY_REPO_CHECKOUT_MODE", " isolated ");
        let cli = Cli::try_parse_from([
            "cordy",
            "repo",
            "checkout",
            "https://github.com/acme/cordy.git",
            "--ref",
            "release/v2",
        ])
        .expect("repo checkout CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("repo checkout");
        assert_eq!(output.stdout, "/work/cordy\n");
        assert!(output.stderr.contains("branch: agent/rust/task-1"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[test]
    fn repo_checkout_retry_delay_matches_go_seconds_date_and_caps() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-24T00:00:00Z")
            .expect("now")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            repo_checkout_retry_delay("7", now),
            std::time::Duration::from_secs(7)
        );
        assert_eq!(
            repo_checkout_retry_delay("60", now),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            repo_checkout_retry_delay("Mon, 24 Aug 2026 00:00:05 GMT", now),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            repo_checkout_retry_delay("invalid", now),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn config_agent_timeout_display_preserves_three_states() {
        let path = Path::new("/tmp/config.json");

        let disabled =
            format_config_table(path, "", &[("agent_timeout", Value::String("0s".into()))]);
        assert!(disabled.contains("0s (disabled)"));

        let positive =
            format_config_table(path, "", &[("agent_timeout", Value::String("30m".into()))]);
        assert!(positive.contains("30m"));
        assert!(!positive.contains("disabled"));

        let unset = format_config_table(path, "", &[("agent_timeout", Value::Null)]);
        assert!(unset.contains("(not set)"));
    }

    #[tokio::test]
    async fn config_show_table_and_json_exclude_credentials_and_unknown_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        fs::write(
            &profile_path,
            r#"{
  "server_url": "https://api.example.com",
  "workspace_id": "workspace-1",
  "agent_timeout": "0s",
  "disable_auto_update": true,
  "token": "mul_secret",
  "future_secret": "do-not-print"
}"#,
        )
        .expect("profile config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let table = Cli::try_parse_from(["cordy", "--profile", "dev", "config"])
            .expect("config default-show CLI");
        let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("config table");
        assert!(output.stdout.contains("Profile:      dev"));
        assert!(output.stdout.contains("agent_timeout:"));
        assert!(output.stdout.contains("0s (disabled)"));
        assert!(output.stdout.contains("disable_auto_update:"));
        assert!(!output.stdout.contains("mul_secret"));
        assert!(!output.stdout.contains("do-not-print"));

        let json = Cli::try_parse_from([
            "cordy",
            "--profile",
            "dev",
            "config",
            "show",
            "--output",
            "json",
        ])
        .expect("config JSON CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("config JSON");
        let config: Value = serde_json::from_str(&output.stdout).expect("config JSON output");
        assert_eq!(config["profile"], "dev");
        assert_eq!(config["server_url"], "https://api.example.com");
        assert_eq!(config["disable_auto_update"], true);
        assert!(config.get("token").is_none());
        assert!(config.get("future_secret").is_none());
    }

    #[tokio::test]
    async fn config_set_is_profile_scoped_and_preserves_unrelated_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let default_path = home.path().join(".cordy/config.json");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        let default_bytes = br#"{"server_url":"https://default.example","token":"mul_default"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        fs::write(
            &profile_path,
            r#"{"token":"mul_dev","future":{"keep":true}}"#,
        )
        .expect("profile config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        for (key, value, expected) in [
            (
                "server_url",
                "https://api.dev.example",
                "https://api.dev.example",
            ),
            ("heartbeat_interval", " 5s ", "5s"),
            ("max_concurrent_tasks", "4", "4"),
            ("disable_auto_reload", "true", "true"),
        ] {
            let cli =
                Cli::try_parse_from(["cordy", "--profile", "dev", "config", "set", key, value])
                    .expect("config set CLI");
            let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("config set");
            assert_eq!(output.stderr, format!("Set {key} = {expected}\n"));
        }
        let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
            .expect("saved JSON");
        assert_eq!(saved["token"], "mul_dev");
        assert_eq!(saved["future"]["keep"], true);
        assert_eq!(saved["heartbeat_interval"], "5s");
        assert_eq!(saved["max_concurrent_tasks"], 4);
        assert_eq!(saved["disable_auto_reload"], true);
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
    }

    #[test]
    fn config_set_whitelist_and_validation_match_registry_contract() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let root = cwd.path().join("data/cordy").display().to_string();
        let valid = [
            ("server_url", "https://api.example.com"),
            ("app_url", "https://app.example.com"),
            ("workspace_id", "workspace-1"),
            ("device_name", "host-a"),
            ("runtime_name", "runtime-a"),
            ("workspaces_root", "data/cordy"),
            ("max_concurrent_tasks", "8"),
            ("poll_interval", "1m30s"),
            ("heartbeat_interval", " 5s "),
            ("agent_timeout", "0s"),
            ("codex_semantic_inactivity_timeout", "15m"),
            ("codex_handshake_timeout", "45s"),
            ("disable_auto_update", "TRUE"),
            ("auto_update_check_interval", "12h"),
            ("disable_auto_reload", "false"),
        ];
        for (key, value) in valid {
            let (_, displayed) =
                validate_config_set(key, value, &environment).expect("valid config value");
            if key == "workspaces_root" {
                assert_eq!(displayed, root);
            }
        }
        for (key, value, message) in [
            ("token", "secret", "unknown config key"),
            ("server_url", "not a URL", "valid URL"),
            ("app_url", "ftp://example.com", "must use one of"),
            ("max_concurrent_tasks", "-1", ">= 0"),
            ("poll_interval", "0s", "positive"),
            ("heartbeat_interval", "abc", "duration"),
            ("agent_timeout", "-1s", ">= 0"),
            ("disable_auto_update", "maybe", "true"),
        ] {
            assert!(validate_config_set(key, value, &environment)
                .expect_err("invalid config value")
                .to_string()
                .contains(message));
        }
    }

    #[tokio::test]
    async fn config_commands_fail_closed_without_task_local_root() {
        let home = tempfile::tempdir().expect("owner home");
        let cwd = tempfile::tempdir().expect("task cwd");
        let owner_path = home.path().join(".cordy/config.json");
        fs::create_dir_all(owner_path.parent().expect("owner parent")).expect("owner dir");
        let owner_bytes = br#"{"server_url":"https://owner.invalid","token":"mul_owner"}"#;
        fs::write(&owner_path, owner_bytes).expect("owner config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        environment.set("CORDY_TASK_ID", "task-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "config",
            "set",
            "server_url",
            "https://task.example",
        ])
        .expect("task config set CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing task root");
        assert!(error.to_string().contains("task-local Cordy config root"));
        assert_eq!(fs::read(&owner_path).expect("owner unchanged"), owner_bytes);

        let task_root = tempfile::tempdir().expect("task root");
        environment.set(
            config::TASK_CONFIG_ROOT_ENV,
            task_root.path().display().to_string(),
        );
        run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("task-local config set");
        let task: Value = serde_json::from_slice(
            &fs::read(task_root.path().join("config.json")).expect("task config"),
        )
        .expect("task config JSON");
        assert_eq!(task["server_url"], "https://task.example");
        assert_eq!(
            fs::read(&owner_path).expect("owner still unchanged"),
            owner_bytes
        );
    }

    #[tokio::test]
    async fn auth_status_matches_human_table_and_json_contracts() {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(
                    request.headers()["authorization"],
                    "Bearer mul_env_status_token"
                );
                assert!(request.headers().get("x-workspace-id").is_none());
                assert!(request.headers().get("x-agent-id").is_none());
                Json(serde_json::json!({"name":"Ada","email":"ada@example.com"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "mul_env_status_token");

        let table = Cli::try_parse_from(["cordy", "auth", "status"]).expect("status CLI");
        let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("table status");
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            format!(
                "Server:  http://{address}\nUser:    Ada (ada@example.com)\nToken:   {}\n",
                display_token_prefix("mul_env_status_token")
            )
        );

        let json = Cli::try_parse_from(["cordy", "auth", "status", "--output", "json"])
            .expect("JSON status CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("JSON status");
        let status: Value = serde_json::from_str(&output.stdout).expect("status JSON");
        assert_eq!(status["authenticated"], true);
        assert_eq!(status["user"]["email"], "ada@example.com");
        assert_eq!(
            status["token"],
            display_token_prefix("mul_env_status_token")
        );
        server.abort();
    }

    #[tokio::test]
    async fn auth_status_task_context_requires_mat_token_and_never_prints_it() {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(
                    request.headers()["authorization"],
                    "Bearer mat_task_status_secret"
                );
                Json(serde_json::json!({"name":"Task Agent","email":"task@example.test"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let task_root = tempfile::tempdir().expect("task root");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_task_status_secret");
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        let cli = Cli::try_parse_from(["cordy", "auth", "status", "--output", "json"])
            .expect("task status CLI");
        let missing_root = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("task-local config root required");
        assert!(missing_root
            .to_string()
            .contains(config::TASK_CONFIG_ROOT_ENV));

        environment.set(
            config::TASK_CONFIG_ROOT_ENV,
            task_root.path().display().to_string(),
        );
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("task status");
        assert!(!output.stdout.contains("mat_task_status_secret"));
        assert!(serde_json::from_str::<Value>(&output.stdout)
            .expect("task status JSON")
            .get("token")
            .is_none());

        environment.set("CORDY_TOKEN", "mul_owner_token");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("human token rejected in task");
        assert!(error.to_string().contains("task-scoped mat_ token"));
        server.abort();
    }

    #[test]
    fn auth_logout_only_clears_current_profile_and_is_task_guarded() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let default_path = home.path().join(".cordy/config.json");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        let default_bytes = br#"{"token":"mul_default","workspace_id":"default"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        fs::write(
            &profile_path,
            r#"{"token":"mul_dev","server_url":"https://dev.example","future":7}"#,
        )
        .expect("profile config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_TOKEN", "mul_env_must_not_affect_logout");
        let cli = Cli::try_parse_from(["cordy", "--profile", "dev", "auth", "logout"])
            .expect("logout CLI");
        let output = run_auth_logout(&cli, &environment).expect("logout");
        assert_eq!(output.stderr, "Token removed. You are now logged out.\n");
        let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
            .expect("profile JSON");
        assert!(saved.get("token").is_none());
        assert_eq!(saved["future"], 7);
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
        assert_eq!(
            run_auth_logout(&cli, &environment)
                .expect("idempotent logout")
                .stderr,
            "Not authenticated.\n"
        );

        environment.set("CORDY_AGENT_ID", "agent-1");
        assert!(run_auth_logout(&cli, &environment)
            .expect_err("task logout rejected")
            .to_string()
            .contains("not available inside a daemon-managed task"));
    }

    #[tokio::test]
    async fn user_profile_get_is_a_real_configured_api_command() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let config_dir = home.path().join(".cordy");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("config.json"),
            r#"{"server_url":"http://127.0.0.1:1","token":"config-token","workspace_id":"config-workspace","future_field":true}"#,
        )
        .expect("config");
        let (server_url, server) = test_server().await;
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("{server_url}/ws?discard=yes"));
        environment.set("CORDY_TOKEN", "token-from-env");
        environment.set("CORDY_WORKSPACE_ID", "workspace-from-env");
        let cli = Cli::try_parse_from(["cordy", "user", "profile", "get", "--output", "json"])
            .expect("parse CLI");

        let output = run(&cli, &environment).await.expect("run profile get");
        let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(json["profile_description"], "Maintainer");
        server.abort();
    }

    #[tokio::test]
    async fn user_profile_update_patches_resolved_description() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let (server_url, captured, server) = patch_test_server().await;
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", server_url);
        environment.set("CORDY_TOKEN", "token-from-env");
        let cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description",
            r"Reviewer\nTypeScript",
            "--output",
            "json",
        ])
        .expect("parse CLI");
        let mut input = Cursor::new(Vec::<u8>::new());

        let output = run_with_input(&cli, &environment, &mut input)
            .await
            .expect("update profile");

        assert_eq!(
            captured
                .lock()
                .expect("captured body")
                .as_ref()
                .expect("body")["profile_description"],
            "Reviewer\nTypeScript"
        );
        let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(json["profile_description"], "Reviewer\nTypeScript");
        server.abort();
    }

    #[test]
    fn profile_update_text_sources_match_go_semantics() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let stdin_cli =
            Cli::try_parse_from(["cordy", "user", "profile", "update", "--description-stdin"])
                .expect("stdin CLI");
        let mut input = Cursor::new(b"first line\nsecond \\n literal\n".to_vec());
        assert_eq!(
            resolve_profile_description(update_args(&stdin_cli), &environment, &mut input)
                .expect("stdin description"),
            "first line\nsecond \\n literal"
        );

        fs::write(
            cwd.path().join("description.md"),
            "标题 / Заголовок\n\n中文段落\n",
        )
        .expect("description file");
        let file_cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            "description.md",
        ])
        .expect("file CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&file_cli),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("file description"),
            "标题 / Заголовок\n\n中文段落"
        );

        let empty_cli =
            Cli::try_parse_from(["cordy", "user", "profile", "update", "--description", ""])
                .expect("empty inline CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&empty_cli),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("empty inline clears"),
            ""
        );
    }

    #[test]
    fn profile_update_rejects_ambiguous_or_empty_input() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let ambiguous = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description",
            "inline",
            "--description-stdin",
        ])
        .expect("ambiguous CLI");
        assert!(resolve_profile_description(
            update_args(&ambiguous),
            &environment,
            &mut Cursor::new(b"stdin".to_vec())
        )
        .expect_err("ambiguous sources")
        .to_string()
        .contains("mutually exclusive"));

        let missing =
            Cli::try_parse_from(["cordy", "user", "profile", "update"]).expect("missing CLI");
        assert!(resolve_profile_description(
            update_args(&missing),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("missing source")
        .to_string()
        .contains("nothing to update"));

        let clear_with_input = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--clear",
            "--description",
            "inline",
        ])
        .expect("clear conflict CLI");
        assert!(resolve_profile_description(
            update_args(&clear_with_input),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("clear conflict")
        .to_string()
        .contains("--clear cannot be combined"));
    }

    #[test]
    fn profile_update_file_input_fails_closed_outside_workdir() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let outside = tempfile::tempdir().expect("outside dir");
        let external_path = outside.path().join("description.md");
        fs::write(&external_path, "external description").expect("external file");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let external_path = external_path.to_string_lossy().into_owned();
        let guarded = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            &external_path,
        ])
        .expect("guarded CLI");
        assert!(resolve_profile_description(
            update_args(&guarded),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("external file rejected")
        .to_string()
        .contains("--allow-external-file"));

        let allowed = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            &external_path,
            "--allow-external-file",
        ])
        .expect("allowed CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&allowed),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("external file allowed"),
            "external description"
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_update_rejects_workdir_symlink_that_escapes() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let outside = tempfile::tempdir().expect("outside dir");
        let external_path = outside.path().join("description.md");
        fs::write(&external_path, "escaped description").expect("external file");
        symlink(&external_path, cwd.path().join("description.md")).expect("symlink");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            "description.md",
        ])
        .expect("symlink CLI");

        assert!(resolve_profile_description(
            update_args(&cli),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("escaping symlink rejected")
        .to_string()
        .contains("--allow-external-file"));
    }


    #[test]
    fn table_output_matches_go_vertical_table_contract() {
        let profile = serde_json::json!({"id":"user-1","name":"Ada","email":"ada@example.com"});
        assert_eq!(
            format_user_profile_table(&profile),
            "ID                   user-1\nNAME                 Ada\nEMAIL                ada@example.com\nPROFILE DESCRIPTION  (not set)\n"
        );
    }




}
