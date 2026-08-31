//! Patchbay command-line client.
//!
//! Only fully functional commands are registered; placeholder command trees
//! are never exposed.
#[cfg(test)]
mod agent_command_tests;
mod agent_commands;
mod agent_helpers;
mod api;
#[cfg(test)]
mod attachment_command_tests;
mod attachment_input;
mod auth_command_schema;
#[cfg(test)]
mod auth_command_tests;
mod auth_commands;
#[cfg(test)]
mod automation_command_tests;
mod automation_commands;
mod automation_output;
mod automation_resolver;
#[cfg(test)]
mod chat_command_tests;
mod chat_commands;
mod cli_command_schema;
mod client_factory;
mod command_dispatch;
mod completion_commands;
pub mod config;
mod config_command_schema;
#[cfg(test)]
mod config_command_tests;
mod config_commands;
pub mod daemon;
mod daemon_command_schema;
#[cfg(test)]
mod daemon_command_tests;
mod daemon_commands;
#[cfg(test)]
mod disk_usage_command_tests;
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
#[cfg(test)]
mod issue_dependency_graph_command_tests;
mod issue_dependency_graph_commands;
mod issue_description;
mod issue_get_commands;
#[cfg(test)]
mod issue_label_command_tests;
mod issue_label_commands;
mod issue_label_schema;
mod issue_list_commands;
mod issue_list_schema;
#[cfg(test)]
mod issue_metadata_command_tests;
mod issue_metadata_commands;
mod issue_metadata_schema;
mod issue_property_schema;
mod issue_pull_request_commands;
mod issue_pull_request_schema;
mod issue_reference;
mod issue_reorder_commands;
mod issue_rerun_commands;
mod issue_safety;
#[cfg(test)]
mod issue_search_command_tests;
mod issue_search_commands;
mod issue_status_commands;
#[cfg(test)]
mod issue_subscriber_command_tests;
mod issue_subscriber_commands;
mod issue_subscriber_schema;
mod issue_task_commands;
#[cfg(test)]
mod issue_timeline_command_tests;
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
#[cfg(test)]
mod login_command_tests;
mod output_helpers;
mod path_safety;
mod project_command_schema;
#[cfg(test)]
mod project_command_tests;
mod project_commands;
#[cfg(test)]
mod project_resource_command_tests;
mod project_resource_commands;
#[cfg(test)]
mod property_command_tests;
mod property_commands;
#[cfg(test)]
mod repo_command_tests;
mod repo_commands;
mod root_command_schema;
#[cfg(test)]
mod root_command_tests;
#[cfg(test)]
mod runtime_command_tests;
mod runtime_commands;
mod runtime_delete;
mod runtime_output;
mod runtime_profile;
mod runtime_update;
mod setup_command_schema;
#[cfg(test)]
mod setup_command_tests;
mod setup_commands;
mod skill_command_schema;
#[cfg(test)]
mod skill_command_tests;
mod skill_commands;
mod task_reference;
mod team_command_schema;
#[cfg(test)]
mod team_command_tests;
mod team_commands;
mod text_input;
mod update_commands;
mod url_helpers;
mod user_command_schema;
mod user_commands;
#[cfg(test)]
mod user_profile_command_tests;
mod version_output;
mod workspace_command_schema;
#[cfg(test)]
mod workspace_command_tests;
mod workspace_commands;
mod workspace_mcp_commands;

use anyhow::Result;
pub(crate) use api::HttpError;
use api::{http_timeout, ApiClient};
use clap::ValueEnum;
use config::Environment;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
#[cfg(test)]
use std::{collections::HashMap, ffi::OsString, fs, sync::Arc};
#[cfg(test)]
use url::form_urlencoded;

#[cfg(test)]
pub(crate) use agent_commands::agent_mcp_path;
pub(crate) use agent_commands::{
    run_agent_avatar, run_agent_copy, run_agent_create, run_agent_env_get, run_agent_env_set,
    run_agent_get, run_agent_lifecycle, run_agent_list, run_agent_mcp_list, run_agent_mcp_mutation,
    run_agent_skills_list, run_agent_skills_mutation, run_agent_tasks, run_agent_update, AgentArgs,
    AgentCommand, AgentEnvArgs, AgentEnvCommand, AgentMcpAction, AgentMcpArgs, AgentMcpCommand,
    AgentSkillsArgs, AgentSkillsCommand,
};
pub(crate) use agent_helpers::{
    apply_agent_permission_args, copied_agent_max_concurrent_tasks, format_agent_details_table,
    format_agent_list_table, resolve_agent_secret_json, validate_agent_custom_env,
};
use attachment_input::{
    append_unique_strings, collect_local_attachments, quick_create_attachment_ids,
};
pub(crate) use auth_command_schema::{AuthArgs, AuthCommand, LoginArgs};
#[cfg(test)]
use auth_commands::display_token_prefix;
use auth_commands::{run_auth_logout, run_auth_status};
pub(crate) use automation_commands::{
    run_automation_create, run_automation_delete, run_automation_get, run_automation_list,
    run_automation_runs, run_automation_trigger, run_automation_trigger_add,
    run_automation_trigger_delete, run_automation_trigger_rotate_url,
    run_automation_trigger_update, run_automation_update, AutomationArgs, AutomationCommand,
};
use automation_output::{
    automation_webhook_url, format_automation_runs_table, format_automation_table,
};
use automation_resolver::{
    load_automation_agent_names, resolve_automation_agent, resolve_automation_id,
    resolve_automation_subscribers, resolve_automation_trigger_id,
};
pub(crate) use chat_commands::{
    run_attachment_download, run_attachment_upload, run_chat_read, AttachmentArgs,
    AttachmentCommand, ChatArgs, ChatCommand,
};
pub(crate) use client_factory::{
    new_api_client, new_unscoped_api_client, new_unscoped_authenticated_api_client,
    normalize_api_base_url, required_workspace_id, resolve_current_workspace_id,
};
pub(crate) use command_dispatch::run_with_input;
use completion_commands::run_completion;
pub(crate) use config_command_schema::{ConfigArgs, ConfigCommand};
#[cfg(test)]
use config_commands::{format_config_table, validate_config_set};
use config_commands::{run_config_set, run_config_show};
pub(crate) use daemon_command_schema::{
    DaemonArgs, DaemonCommand, DaemonDiskUsageArgs, DaemonLaunchArgs, DaemonLogsArgs,
    DaemonRestartArgs, DaemonStartArgs, DaemonStatusArgs,
};
pub use daemon_commands::run_private_helper;
#[cfg(test)]
use daemon_commands::{
    ensure_restart_is_background, format_daemon_status_table, known_daemon_profiles,
    read_daemon_log_tail, render_daemon_status, require_known_daemon_profile,
    resolve_daemon_log_path, resolve_daemon_status_port,
};
use daemon_commands::{
    parse_cli_duration, parse_log_lines, run_daemon_after_setup, run_daemon_disk_usage,
    run_daemon_logs, run_daemon_probe_runtimes, run_daemon_restart, run_daemon_start,
    run_daemon_status, run_daemon_stop,
};
use disk_usage_commands::{
    disk_usage_needs_parent_status, disk_usage_task_context, enumerate_disk_usage_roots,
    fill_disk_usage_parent_statuses, limit_disk_usage_aggregate, limit_disk_usage_report,
    resolve_disk_usage_root, validate_disk_usage_args, with_disk_usage_status_deadline,
};
#[cfg(test)]
use disk_usage_output::format_disk_ratio;
use disk_usage_output::{
    append_disk_usage_warning, format_disk_usage_aggregate_table, format_disk_usage_report_table,
};
pub use error::command_error_output;
pub(crate) use error::command_output_error;
pub(crate) use execution_policy::{require_human_local_command, require_task_local_config_root};
pub(crate) use id_helpers::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix};
pub(crate) use issue_activity_schema::{
    IssueCancelTaskArgs, IssueCommentAddArgs, IssueCommentArgs, IssueCommentCommand,
    IssueCommentListArgs, IssueCommentResolutionArgs, IssueMessageMainArgs, IssueRerunArgs,
    IssueRunMessagesArgs, IssueRunsArgs, IssueSearchArgs, IssueUsageArgs,
};
use issue_actor_output::{format_issue_list_table, load_issue_actor_names, IssueActorNames};
use issue_actor_resolver::{
    normalize_assignee_input, resolve_issue_assignee_id, resolve_issue_assignee_name,
    resolve_issue_project_id, resolve_project_reference, resolve_subscriber_id,
    resolve_subscriber_name, retry_actor_get, ResolvedIssueAssignee,
};
use issue_assign_commands::run_issue_assign;
use issue_children_commands::run_issue_children;
#[cfg(test)]
use issue_children_commands::{child_stage, format_issue_children_table, group_issue_children};
pub(crate) use issue_command_schema::{
    IssueArgs, IssueAssignArgs, IssueCommand, IssueCreateArgs, IssueDependencyGraphApplyArgs,
    IssueDependencyGraphArgs, IssueDependencyGraphCommand, IssueReorderArgs, IssueStatusArgs,
    IssueUpdateArgs,
};
#[cfg(test)]
use issue_comment_add_commands::resolve_issue_comment_content;
use issue_comment_add_commands::run_issue_comment_add;
#[cfg(test)]
use issue_comment_list_commands::format_issue_comments_table;
use issue_comment_list_commands::run_issue_comment_list;
use issue_comment_mutation_commands::{run_issue_comment_delete, run_issue_comment_resolution};
use issue_create_commands::run_issue_create;
use issue_dependency_graph_commands::{
    run_issue_dependency_graph_apply, run_issue_dependency_graph_get,
};
use issue_description::{resolve_issue_create_description, resolve_issue_update_description};
#[cfg(test)]
use issue_get_commands::format_issue_get_table;
use issue_get_commands::run_issue_get;
use issue_label_commands::{run_issue_label_add, run_issue_label_list, run_issue_label_remove};
pub(crate) use issue_label_schema::{
    IssueLabelArgs, IssueLabelCommand, IssueLabelListArgs, IssueLabelMutationArgs,
};
use issue_list_commands::run_issue_list;
#[cfg(test)]
use issue_list_commands::{build_issue_list_query, build_metadata_filter, issue_list_has_more};
pub(crate) use issue_list_schema::IssueListArgs;
#[cfg(test)]
use issue_metadata_commands::{format_metadata_table, parse_metadata_value};
use issue_metadata_commands::{
    run_issue_metadata_delete, run_issue_metadata_get, run_issue_metadata_list,
    run_issue_metadata_set,
};
pub(crate) use issue_metadata_schema::{
    IssueMetadataArgs, IssueMetadataCommand, IssueMetadataDeleteArgs, IssueMetadataKeyArgs,
    IssueMetadataListArgs, IssueMetadataSetArgs,
};
pub(crate) use issue_property_schema::{
    IssuePropertyArgs, IssuePropertyCommand, IssuePropertyListArgs, IssuePropertyMutationArgs,
    IssuePropertyUnsetArgs,
};
#[cfg(test)]
use issue_pull_request_commands::format_issue_pull_requests_table;
use issue_pull_request_commands::{run_issue_pull_request_attach, run_issue_pull_requests};
pub(crate) use issue_pull_request_schema::{
    IssuePullRequestArgs, IssuePullRequestAttachArgs, IssuePullRequestCommand,
};
use issue_reference::resolve_issue_ref;
#[cfg(test)]
use issue_reorder_commands::compute_reorder_position;
use issue_reorder_commands::run_issue_reorder;
use issue_rerun_commands::run_issue_rerun;
#[cfg(test)]
use issue_safety::guard_issue_description_local_links;
#[cfg(test)]
use issue_search_commands::format_issue_search_table;
use issue_search_commands::run_issue_search;
use issue_status_commands::run_issue_status;
#[cfg(test)]
use issue_subscriber_commands::format_issue_subscribers_table;
use issue_subscriber_commands::{run_issue_subscriber_list, run_issue_subscriber_mutation};
pub(crate) use issue_subscriber_schema::{
    IssueSubscriberArgs, IssueSubscriberCommand, IssueSubscriberMutationArgs,
};
#[cfg(test)]
use issue_task_commands::{format_issue_run_messages_table, format_issue_runs_table};
use issue_task_commands::{
    run_issue_cancel_task, run_issue_message_main, run_issue_run_messages, run_issue_runs,
};
use issue_timeline_commands::run_issue_timeline;
#[cfg(test)]
use issue_timeline_commands::{
    build_timeline_filter, filter_timeline, format_issue_timeline_table,
};
pub(crate) use issue_timeline_schema::IssueTimelineArgs;
use issue_update_commands::run_issue_update;
use issue_usage_commands::run_issue_usage;
use issue_value_helpers::{
    format_metadata_value, issue_labels, validate_issue_priority, validate_issue_status,
};
pub(crate) use json_helpers::value_string;
pub(crate) use label_command_schema::{LabelArgs, LabelCommand, LabelCreateArgs, LabelUpdateArgs};
#[cfg(test)]
use label_commands::{format_label_result, format_workspace_label_table};
use label_commands::{
    format_label_table, run_label_create, run_label_delete, run_label_get, run_label_list,
    run_label_update,
};
use label_reference::{resolve_label_id, resolve_label_reference};
#[cfg(test)]
use login::{
    build_login_url, build_workspace_creation_url, constant_time_equal, wait_for_login_callback,
    wait_for_workspace_creation_with_opener,
};
use login::{run_login, run_login_with_urls, AuthUser};
pub(crate) use output_helpers::{display_id, format_table, truncate_text};
use path_safety::{ensure_file_within_workdir, lexical_normalize};
pub(crate) use project_command_schema::{
    ProjectArgs, ProjectCommand, ProjectCreateArgs, ProjectResourceAddArgs, ProjectResourceArgs,
    ProjectResourceCommand, ProjectResourceUpdateArgs, ProjectUpdateArgs,
};
#[cfg(test)]
use project_commands::{
    format_project_details_table, format_project_list_table, validate_project_status,
    PROJECT_STATUSES,
};
use project_commands::{
    run_project_create, run_project_delete, run_project_get, run_project_list, run_project_status,
    run_project_update,
};
#[cfg(test)]
use project_resource_commands::{
    build_project_resource_add_ref, build_project_resource_update_ref,
};
use project_resource_commands::{
    run_project_resource_add, run_project_resource_list, run_project_resource_remove,
    run_project_resource_update,
};
#[cfg(test)]
pub(crate) use property_commands::{
    build_issue_property_rows, format_issue_property_rows, format_property_definitions,
    parse_property_options, resolve_property, PropertyDefinition, PropertyOption,
};
pub(crate) use property_commands::{
    run_issue_property_list, run_issue_property_set, run_issue_property_unset,
    run_property_archive, run_property_create, run_property_get, run_property_list,
    run_property_update, PropertyArgs, PropertyCommand,
};
#[cfg(test)]
pub(crate) use repo_commands::{repo_checkout_retry_delay, repo_urls, WorkspaceRepo};
pub(crate) use repo_commands::{
    run_repo_add, run_repo_checkout, run_repo_list, run_repo_remove, RepoArgs, RepoCommand,
};
pub(crate) use root_command_schema::{UpdateArgs, VersionOutput};
pub(crate) use runtime_commands::{
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
use runtime_update::run_runtime_update;
#[cfg(test)]
use runtime_update::{format_runtime_update_result, run_runtime_update_with_policy};
pub(crate) use setup_command_schema::{SetupArgs, SetupCommand, SetupError};
#[cfg(test)]
use setup_commands::{
    confirm_setup_overwrite, prepare_setup_profile, resolve_setup_profile_input,
    setup_callback_host, SetupDaemonAction,
};
use setup_commands::{
    dispatch_daemon_after_setup, read_setup_confirmation, run_setup, setup_daemon_action,
};
pub(crate) use skill_command_schema::{
    SkillArgs, SkillCommand, SkillCreateArgs, SkillDeleteArgs, SkillFilesArgs, SkillFilesCommand,
    SkillFilesDeleteArgs, SkillFilesListArgs, SkillFilesUpsertArgs, SkillGetArgs, SkillImportArgs,
    SkillRefreshArgs, SkillSearchArgs, SkillUpdateArgs,
};
#[cfg(test)]
use skill_commands::{
    format_skill_files_table, format_skill_import_table, format_skill_list_table,
    format_skill_search_table, read_skill_archive, resolve_skill_content,
    resolve_skill_content_sources,
};
use skill_commands::{
    run_skill_create, run_skill_delete, run_skill_files_delete, run_skill_files_list,
    run_skill_files_upsert, run_skill_get, run_skill_import, run_skill_list, run_skill_refresh,
    run_skill_search, run_skill_update,
};
use task_reference::resolve_task_run_id;
pub(crate) use team_command_schema::{
    TeamActivityArgs, TeamArgs, TeamCommand, TeamCreateArgs, TeamMemberAddArgs, TeamMemberArgs,
    TeamMemberCommand, TeamMemberRemoveArgs, TeamMemberSetRoleArgs, TeamUpdateArgs,
};
#[cfg(test)]
use team_commands::{
    format_team_details_table, format_team_list_table, render_team_member_output,
    team_member_count_display,
};
use team_commands::{
    run_team_activity, run_team_create, run_team_delete, run_team_get, run_team_list,
    run_team_member_add, run_team_member_list, run_team_member_remove, run_team_member_set_role,
    run_team_update,
};
pub(crate) use text_input::{trim_one_trailing_newline, unescape_backslash_escapes};
use update_commands::run_update;
#[cfg(test)]
use update_commands::{
    render_update_outcome, resolve_update_download_timeout, validate_update_timeout,
};
pub(crate) use url_helpers::encoded_path_segment;
pub(crate) use user_command_schema::{
    ProfileArgs, ProfileCommand, UpdateProfileArgs, UserArgs, UserCommand,
};
#[cfg(test)]
use user_commands::{format_user_profile_table, resolve_profile_description};
use user_commands::{run_user_profile_get, run_user_profile_update};
use version_output::run_version;
pub(crate) use workspace_command_schema::{
    CreateWorkspaceArgs, UpdateWorkspaceArgs, WorkspaceArgs, WorkspaceCommand, WorkspaceMcpAddArgs,
    WorkspaceMcpArgs, WorkspaceMcpCommand, WorkspaceMcpUpdateArgs, WorkspaceMemberArgs,
    WorkspaceMemberCommand, WorkspaceMemberInviteArgs,
};
#[cfg(test)]
use workspace_commands::{
    build_workspace_create_body, build_workspace_update_body, format_workspace_details_table,
    format_workspace_table, normalize_workspace_invite_role, resolve_workspace_reference,
    WorkspaceSummary,
};
use workspace_commands::{
    resolve_workspace_arg, run_workspace_create, run_workspace_get, run_workspace_list,
    run_workspace_member_invite, run_workspace_member_list, run_workspace_switch,
    run_workspace_update,
};
#[cfg(test)]
use workspace_mcp_commands::parse_workspace_mcp_server_config;
use workspace_mcp_commands::{
    format_workspace_mcp_servers, run_workspace_mcp_add, run_workspace_mcp_list,
    run_workspace_mcp_remove, run_workspace_mcp_update, WorkspaceMcpServer,
};

pub const CLIENT_VERSION: &str = env!("PATCHBAY_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("PATCHBAY_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("PATCHBAY_BUILD_DATE");
pub const BUILD_OS: &str = env!("PATCHBAY_BUILD_OS");
pub const BUILD_ARCH: &str = env!("PATCHBAY_BUILD_ARCH");

pub const ROOT_LONG_VERSION: &str = concat!(
    env!("PATCHBAY_BUILD_VERSION"),
    " (commit: ",
    env!("PATCHBAY_BUILD_COMMIT"),
    ", built: ",
    env!("PATCHBAY_BUILD_DATE"),
    ")\nos/arch: ",
    env!("PATCHBAY_BUILD_OS"),
    "/",
    env!("PATCHBAY_BUILD_ARCH")
);

pub use cli_command_schema::Cli;
pub(crate) use cli_command_schema::Command;

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

const CLOUD_SERVER_URL: &str = "https://api.aspectlylabs.com";
const CLOUD_APP_URL: &str = "https://patchbay.aspectlylabs.com";

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
pub(crate) use tests::{
    create_workspace_args, issue_search_args, patch_test_server, test_server, update_args,
    update_workspace_args,
};

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Request;
    use axum::http::HeaderMap;
    use axum::routing::{delete as delete_route, get, patch, post, put};
    use axum::{Json, Router};
    use clap::Parser;
    use std::fs;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
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
                OsString::from("patchbay"),
                OsString::from(patchbay_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
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
                OsString::from("patchbay"),
                OsString::from(patchbay_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
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

    pub(crate) async fn test_server() -> (String, tokio::task::JoinHandle<()>) {
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

    pub(crate) async fn patch_test_server() -> (
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

    pub(crate) fn update_args(cli: &Cli) -> &UpdateProfileArgs {
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

    pub(crate) fn create_workspace_args(cli: &Cli) -> &CreateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Create(args),
            }) => args,
            _ => panic!("expected workspace create"),
        }
    }

    pub(crate) fn update_workspace_args(cli: &Cli) -> &UpdateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Update(args),
            }) => args,
            _ => panic!("expected workspace update"),
        }
    }

    pub(crate) fn issue_list_args(cli: &Cli) -> &IssueListArgs {
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

    pub(crate) fn issue_update_args(cli: &Cli) -> &IssueUpdateArgs {
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

    pub(crate) fn issue_status_args(cli: &Cli) -> &IssueStatusArgs {
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

    pub(crate) fn issue_runs_args(cli: &Cli) -> &IssueRunsArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Runs(args),
            }) => args,
            _ => panic!("expected issue runs"),
        }
    }

    pub(crate) fn issue_run_messages_args(cli: &Cli) -> &IssueRunMessagesArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::RunMessages(args),
            }) => args,
            _ => panic!("expected issue run-messages"),
        }
    }

    pub(crate) fn issue_cancel_task_args(cli: &Cli) -> &IssueCancelTaskArgs {
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

    pub(crate) fn issue_rerun_args(cli: &Cli) -> &IssueRerunArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Rerun(args),
            }) => args,
            _ => panic!("expected issue rerun"),
        }
    }

    pub(crate) fn issue_search_args(cli: &Cli) -> &IssueSearchArgs {
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
            "patchbay",
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
            "PatchbayBot".into(),
        )]));
        let table = format_issue_list_table(&issues, true, &actors);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("ID"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("11111111-1111-1111-1111-111111111111"));
        assert!(table.contains("agent:PatchbayBot"));
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
            .route("/api/teams", get(|| async { Json(serde_json::json!([])) }))
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
                vec!["patchbay", "issue", "list", "--sort", "nonsense"],
                "invalid --sort",
            ),
            (
                vec!["patchbay", "issue", "list", "--direction", "desc"],
                "--direction requires --sort",
            ),
            (
                vec![
                    "patchbay",
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
                    "patchbay",
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
                    "patchbay",
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
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "get", "CORD-18"]).expect("issue get CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Get { id, output },
            }) => {
                assert_eq!(id, "CORD-18");
                assert_eq!(output, OutputFormat::Json);
            }
            _ => panic!("expected issue get"),
        }
        assert!(Cli::try_parse_from(["patchbay", "issue", "get"]).is_err());
        assert!(Cli::try_parse_from(["patchbay", "issue", "get", "A-1", "B-2"]).is_err());
        assert!(
            Cli::try_parse_from(["patchbay", "issue", "get", "CORD-18", "--output", "table"])
                .is_ok()
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
            assert!(error.to_string().contains("PB-123"));
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "get", "CORD-18"]).expect("issue get CLI");
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
            let cli = Cli::try_parse_from(["patchbay", "issue", name, "CORD-18"])
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["patchbay", "issue", "prs", "CORD-18", "--output", "json"])
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
            Cli::try_parse_from(["patchbay", "issue", "pull-request", "attach", "CORD-18"])
                .is_err()
        );
        let cli = Cli::try_parse_from([
            "patchbay",
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
            "--close-intent",
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
                assert!(args.close_intent);
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
        assert!(body.get("close_intent").is_none());
        assert!(body.get("state").is_none());
        assert!(body.get("head_sha").is_none());
        task.abort();
    }

    #[test]
    fn issue_children_parser_supports_alias_output_and_full_id_flag() {
        for name in ["children", "subissues"] {
            let cli = Cli::try_parse_from([
                "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay", "issue", "children", "CORD-18", "--output", "json",
        ])
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
        let actors = IssueActorNames(HashMap::from([(
            "agent:agent-1".into(),
            "PatchbayBot".into(),
        )]));
        let table = format_issue_children_table(&children, false, &actors);
        assert!(table.starts_with("STAGE"));
        assert!(table.contains("CORD-19"));
        assert!(table.contains("First barrier"));
        assert!(table.contains("agent:PatchbayBot"));
        assert!(!table.contains("child-1"));
        let full = format_issue_children_table(&children, true, &actors);
        assert!(full.contains("ID"));
        assert!(full.contains("child-1"));
    }

    #[test]
    fn issue_create_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
        let remediation = "Deliver it with `patchbay issue create --attachment <path>`.";
        guard_issue_description_local_links(&markdown, &human, remediation)
            .expect("human links are allowed");

        let mut agent = Environment::for_test(home.path().into(), cwd.path().into());
        agent.set("PATCHBAY_AGENT_ID", "agent-1");
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
            .route("/api/teams", get(|| async { Json(serde_json::json!([])) }))
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        environment.set("PATCHBAY_QUICK_CREATE_TASK_ID", "task-quick");
        environment.set(
            "PATCHBAY_QUICK_CREATE_ATTACHMENT_IDS",
            r#"["attachment-env","attachment-shared"]"#,
        );
        let cli = Cli::try_parse_from([
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["patchbay", "issue", "create", "--title", "Duplicate"])
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");

        let invalid = Cli::try_parse_from([
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "update", "CORD-18", "--priority", "P1"])
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
            .route("/api/teams", get(|| async { Json(serde_json::json!([])) }))
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");

        let clear = Cli::try_parse_from([
            "patchbay",
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

        let no_changes = Cli::try_parse_from(["patchbay", "issue", "update", "CORD-18"])
            .expect("no changes CLI");
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
            "patchbay",
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

        let missing = Cli::try_parse_from(["patchbay", "issue", "assign", "CORD-18"])
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
            .route("/api/teams", get(|| async { Json(serde_json::json!([])) }))
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");

        let assign = Cli::try_parse_from([
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
        let cli = Cli::try_parse_from(["patchbay", "issue", "status", "CORD-18", "not a status"])
            .expect("status CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("malformed status");
        assert!(error.to_string().contains("status key"));
    }

    #[test]
    fn issue_reorder_parser_enforces_exactly_one_real_target() {
        assert!(Cli::try_parse_from(["patchbay", "issue", "reorder", "CORD-18"]).is_err());
        assert!(Cli::try_parse_from([
            "patchbay", "issue", "reorder", "CORD-18", "--top", "--bottom"
        ])
        .is_err());
        let cli = Cli::try_parse_from([
            "patchbay", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
        ])
        .expect("reorder CLI");
        let args = issue_reorder_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.before.as_deref(), Some("CORD-1"));
        assert_eq!(args.output, OutputFormat::Table);

        let false_top =
            Cli::try_parse_from(["patchbay", "issue", "reorder", "CORD-18", "--top=false"])
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
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
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "reorder", "CORD-18", "--bottom=false"])
                .expect("false bool reaches runtime");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("false selector");
        assert!(error.to_string().contains("cannot be set to false"));
    }

    #[test]
    fn issue_comment_add_parser_and_content_sources_match_go() {
        let cli = Cli::try_parse_from([
            "patchbay",
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
        let cli = Cli::try_parse_from(["patchbay", "issue", "comment", "add", "CORD-18"])
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");

        let delete = Cli::try_parse_from(["patchbay", "issue", "comment", "delete", "comment-1"])
            .expect("delete CLI");
        let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete comment");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Comment comment-1 deleted.\n");

        let resolve = Cli::try_parse_from(["patchbay", "issue", "comment", "resolve", "comment-1"])
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
            "patchbay",
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
            "patchbay",
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
            "patchbay", "issue", "comment", "list", "CORD-18", "--tail", "1",
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
                        "X-Patchbay-Next-Before",
                        "2026-08-23T23:00:00Z".parse().expect("cursor"),
                    );
                    headers.insert(
                        "X-Patchbay-Next-Before-Id",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["patchbay", "issue", "runs", "CORD-18"]).expect("runs CLI");
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
            "patchbay",
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
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "patchbay",
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

        let missing_scope = Cli::try_parse_from(["patchbay", "issue", "cancel-task", "abcd"])
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
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "usage", "CORD-18", "--output", "json"])
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "usage", "CORD-18"]).expect("usage CLI");
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["patchbay", "issue", "rerun", "CORD-18", "--output", "table"])
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
            "patchbay", "label", "create", "--name", "Bug", "--color", "#ff0000", "--output",
            "table",
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
        environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
        environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
        environment.set("PATCHBAY_TOKEN", "token-1");

        let create = Cli::try_parse_from([
            "patchbay", "label", "create", "--name", "Bug", "--color", "#ff0000",
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
            "patchbay", "label", "update", label_id, "--name", "Defect", "--output", "table",
        ])
        .expect("label update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update label");
        assert!(updated.stdout.contains("Defect"));

        let delete =
            Cli::try_parse_from(["patchbay", "label", "delete", label_id, "--output", "json"])
                .expect("label delete CLI");
        let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete label");
        let deleted: Value = serde_json::from_str(&deleted.stdout).expect("deleted JSON");
        assert_eq!(deleted["id"], label_id);
        assert_eq!(deleted["deleted"], true);
        task.abort();
    }
}
