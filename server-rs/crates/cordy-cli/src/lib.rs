//! Cordy CLI — incremental Rust replacement for `server/cmd/cordy`.
//!
//! The S10 migration deliberately registers only fully functional commands.
//! Shared configuration, API, error, and safe text-input behavior is ported
//! with each vertical slice rather than exposing placeholder command trees.
mod agent_commands;
mod agent_helpers;
mod api;
mod api_error;
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
#[cfg(test)]
mod issue_assign_command_tests;
#[cfg(test)]
mod issue_status_command_tests;
#[cfg(test)]
mod issue_reorder_command_tests;
#[cfg(test)]
mod issue_comment_add_command_tests;
#[cfg(test)]
mod issue_comment_mutation_command_tests;
#[cfg(test)]
mod issue_comment_list_command_tests;
#[cfg(test)]
mod issue_runs_command_tests;
#[cfg(test)]
mod issue_run_controls_command_tests;
#[cfg(test)]
mod issue_usage_command_tests;
#[cfg(test)]
mod issue_rerun_command_tests;
#[cfg(test)]
mod cli_test_helpers;
#[cfg(test)]
mod private_helper_command_tests;
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
mod daemon_diagnostics_commands;
mod daemon_execenv_commands;
mod daemon_lifecycle_commands;
mod daemon_log_commands;
mod daemon_status_commands;
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
pub use daemon_execenv_commands::run_private_helper;
use daemon_lifecycle_commands::{
    ensure_restart_is_background, parse_cli_duration, run_daemon_after_setup,
    run_daemon_restart, run_daemon_start, run_daemon_stop, validate_daemon_health_port,
};
use daemon_diagnostics_commands::{run_daemon_disk_usage, run_daemon_probe_runtimes};
use daemon_log_commands::{
    parse_log_lines, read_daemon_log_tail, resolve_daemon_log_path, run_daemon_logs,
};
use daemon_status_commands::{
    format_daemon_status_table, known_daemon_profiles, render_daemon_status,
    require_known_daemon_profile, resolve_daemon_status_port, run_daemon_status,
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
    run_login, run_login_with_urls, validate_login_token, wait_for_login_callback,
    wait_for_workspace_creation,
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
