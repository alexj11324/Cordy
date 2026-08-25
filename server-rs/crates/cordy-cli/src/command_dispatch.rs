use std::io::Read;

use super::dispatch_agent::run_agent_command;
use super::dispatch_autopilot::run_autopilot_command;
use super::dispatch_skill::run_skill_command;
use super::*;

pub(super) async fn run_with_input<R: Read>(
    cli: &Cli,
    environment: &Environment,
    input: &mut R,
) -> Result<RunOutput> {
    match &cli.command {
        Command::Agent(args) => run_agent_command(cli, environment, args, input).await,
        Command::Skill(args) => run_skill_command(cli, environment, args, input).await,
        Command::Autopilot(args) => run_autopilot_command(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::List(args),
        }) => run_issue_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Get { id, output },
        }) => run_issue_get(cli, environment, id, *output).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::PullRequests { id, output },
        }) => run_issue_pull_requests(cli, environment, id, *output).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::PullRequest(IssuePullRequestArgs {
                    command: IssuePullRequestCommand::Attach(args),
                }),
        }) => run_issue_pull_request_attach(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Children {
                    id,
                    output,
                    full_id,
                },
        }) => run_issue_children(cli, environment, id, *output, *full_id).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Create(args),
        }) => run_issue_create(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Update(args),
        }) => run_issue_update(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Assign(args),
        }) => run_issue_assign(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Status(args),
        }) => run_issue_status(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Reorder(args),
        }) => run_issue_reorder(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::List(args),
                }),
        }) => run_issue_comment_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Add(args),
                }),
        }) => run_issue_comment_add(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Delete { comment_id },
                }),
        }) => run_issue_comment_delete(cli, environment, comment_id).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Resolve(args),
                }),
        }) => run_issue_comment_resolution(cli, environment, args, true).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Unresolve(args),
                }),
        }) => run_issue_comment_resolution(cli, environment, args, false).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Runs(args),
        }) => run_issue_runs(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::RunMessages(args),
        }) => run_issue_run_messages(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Usage(args),
        }) => run_issue_usage(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Rerun(args),
        }) => run_issue_rerun(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::CancelTask(args),
        }) => run_issue_cancel_task(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Search(args),
        }) => run_issue_search(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::List { issue_id, output },
                }),
        }) => run_issue_subscriber_list(cli, environment, issue_id, *output).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Add(args),
                }),
        }) => run_issue_subscriber_mutation(cli, environment, args, true).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Remove(args),
                }),
        }) => run_issue_subscriber_mutation(cli, environment, args, false).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::List(args),
                }),
        }) => run_issue_label_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Add(args),
                }),
        }) => run_issue_label_add(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Remove(args),
                }),
        }) => run_issue_label_remove(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::List(args),
                }),
        }) => run_issue_metadata_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Get(args),
                }),
        }) => run_issue_metadata_get(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Set(args),
                }),
        }) => run_issue_metadata_set(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Delete(args),
                }),
        }) => run_issue_metadata_delete(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Timeline(args),
        }) => run_issue_timeline(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::List(args),
                }),
        }) => run_issue_property_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Set(args),
                }),
        }) => run_issue_property_set(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Unset(args),
                }),
        }) => run_issue_property_unset(cli, environment, args).await,
        Command::Auth(AuthArgs {
            command: AuthCommand::Status { output },
        }) => run_auth_status(cli, environment, *output).await,
        Command::Auth(AuthArgs {
            command: AuthCommand::Logout,
        }) => run_auth_logout(cli, environment),
        Command::Login(args) => run_login(cli, environment, args).await,
        Command::Config(ConfigArgs { command: None }) => {
            run_config_show(cli, environment, OutputFormat::Table)
        }
        Command::Config(ConfigArgs {
            command: Some(ConfigCommand::Show { output }),
        }) => run_config_show(cli, environment, *output),
        Command::Config(ConfigArgs {
            command: Some(ConfigCommand::Set { key, value }),
        }) => run_config_set(cli, environment, key, value),
        Command::User(UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Get { output },
                }),
        }) => run_user_profile_get(cli, environment, *output).await,
        Command::User(UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Update(args),
                }),
        }) => run_user_profile_update(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::List { output, full_id },
        }) => run_workspace_list(cli, environment, *output, *full_id).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Get { workspace, output },
        }) => run_workspace_get(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Create(args),
        }) => run_workspace_create(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Update(args),
        }) => run_workspace_update(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Switch { workspace },
        }) => run_workspace_switch(cli, environment, workspace).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::List { workspace, output },
                }),
        }) => run_workspace_member_list(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::Invite(args),
                }),
        }) => run_workspace_member_invite(cli, environment, args).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::List { workspace, output },
                }),
        }) => run_workspace_mcp_list(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Add(args),
                }),
        }) => run_workspace_mcp_add(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Update(args),
                }),
        }) => run_workspace_mcp_update(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command:
                        WorkspaceMcpCommand::Remove {
                            server_id,
                            workspace,
                            output,
                        },
                }),
        }) => {
            run_workspace_mcp_remove(cli, environment, server_id, workspace.as_deref(), *output)
                .await
        }
        Command::Squad(SquadArgs {
            command: SquadCommand::List { output },
        }) => run_squad_list(cli, environment, *output).await,
        Command::Squad(SquadArgs {
            command: SquadCommand::Get { squad_id, output },
        }) => run_squad_get(cli, environment, squad_id, *output).await,
        Command::Squad(SquadArgs {
            command: SquadCommand::Create(args),
        }) => run_squad_create(cli, environment, args).await,
        Command::Squad(SquadArgs {
            command: SquadCommand::Update(args),
        }) => run_squad_update(cli, environment, args).await,
        Command::Squad(SquadArgs {
            command: SquadCommand::Delete { squad_id, output },
        }) => run_squad_delete(cli, environment, squad_id, *output).await,
        Command::Squad(SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::List { squad_id, output },
                }),
        }) => run_squad_member_list(cli, environment, squad_id, *output).await,
        Command::Squad(SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::Add(args),
                }),
        }) => run_squad_member_add(cli, environment, args).await,
        Command::Squad(SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::SetRole(args),
                }),
        }) => run_squad_member_set_role(cli, environment, args).await,
        Command::Squad(SquadArgs {
            command:
                SquadCommand::Member(SquadMemberArgs {
                    command: SquadMemberCommand::Remove(args),
                }),
        }) => run_squad_member_remove(cli, environment, args).await,
        Command::Squad(SquadArgs {
            command: SquadCommand::Activity(args),
        }) => run_squad_activity(cli, environment, args).await,
        Command::Label(LabelArgs {
            command: LabelCommand::List { output, full_id },
        }) => run_label_list(cli, environment, *output, *full_id).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Get { id, output },
        }) => run_label_get(cli, environment, id, *output).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Create(args),
        }) => run_label_create(cli, environment, args).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Update(args),
        }) => run_label_update(cli, environment, args).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Delete { id, output },
        }) => run_label_delete(cli, environment, id, *output).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::List {
                    output,
                    full_id,
                    status,
                },
        }) => run_project_list(cli, environment, *output, *full_id, status.as_deref()).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Get { id, output },
        }) => run_project_get(cli, environment, id, *output).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Create(args),
        }) => run_project_create(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Update(args),
        }) => run_project_update(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Delete { id, output },
        }) => run_project_delete(cli, environment, id, *output).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Status { id, status, output },
        }) => run_project_status(cli, environment, id, status, *output).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::List {
                            project_id,
                            output,
                            full_id,
                        },
                }),
        }) => run_project_resource_list(cli, environment, project_id, *output, *full_id).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) => run_project_resource_add(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Update(args),
                }),
        }) => run_project_resource_update(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::Remove {
                            project_id,
                            resource_id,
                            output,
                        },
                }),
        }) => run_project_resource_remove(cli, environment, project_id, resource_id, *output).await,
        Command::Property(PropertyArgs {
            command:
                PropertyCommand::List {
                    output,
                    include_archived,
                },
        }) => run_property_list(cli, environment, *output, *include_archived).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Get { property, output },
        }) => run_property_get(cli, environment, property, *output).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Create(args),
        }) => run_property_create(cli, environment, args).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Update(args),
        }) => run_property_update(cli, environment, args).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Archive(args),
        }) => run_property_archive(cli, environment, args, true).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Unarchive(args),
        }) => run_property_archive(cli, environment, args, false).await,
        Command::Chat(ChatArgs {
            command: ChatCommand::History(args),
        }) => run_chat_read(cli, environment, "/api/chat/history", None, args, true).await,
        Command::Chat(ChatArgs {
            command: ChatCommand::Thread(args),
        }) => {
            run_chat_read(
                cli,
                environment,
                "/api/chat/thread",
                args.id.as_deref(),
                &args.read,
                false,
            )
            .await
        }
        Command::Attachment(AttachmentArgs {
            command:
                AttachmentCommand::Download {
                    attachment_id,
                    output_dir,
                },
        }) => run_attachment_download(cli, environment, attachment_id, output_dir).await,
        Command::Attachment(AttachmentArgs {
            command: AttachmentCommand::Upload { path, task },
        }) => run_attachment_upload(cli, environment, path, task.as_deref()).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::List { output },
        }) => run_repo_list(cli, environment, *output).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Add(args),
        }) => run_repo_add(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Remove(args),
        }) => run_repo_remove(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Checkout { url, checkout_ref },
        }) => run_repo_checkout(environment, url, checkout_ref.as_deref()).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::List { output },
        }) => run_runtime_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Usage {
                    runtime_id,
                    output,
                    days,
                },
        }) => run_runtime_usage(cli, environment, runtime_id, *output, *days).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::Activity { runtime_id, output },
        }) => run_runtime_activity(cli, environment, runtime_id, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Rename {
                    runtime_id,
                    name,
                    machine,
                    output,
                },
        }) => run_runtime_rename(cli, environment, runtime_id, name, *machine, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Delete {
                    runtime_id,
                    cascade,
                    output,
                },
        }) => run_runtime_delete(cli, environment, runtime_id, *cascade, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Update {
                    runtime_id,
                    target_version,
                    output,
                    wait,
                },
        }) => {
            run_runtime_update(
                cli,
                environment,
                runtime_id,
                target_version.as_deref(),
                *output,
                *wait,
            )
            .await
        }
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::List { output },
                }),
        }) => run_runtime_profile_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Create(args),
                }),
        }) => run_runtime_profile_create(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Update(args),
                }),
        }) => run_runtime_profile_update(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Delete { profile_id },
                }),
        }) => run_runtime_profile_delete(cli, environment, profile_id).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::SetPath { profile_id, path },
                }),
        }) => run_runtime_profile_set_path(cli, environment, profile_id, path.as_deref()),
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::UnsetPath { profile_id },
                }),
        }) => run_runtime_profile_unset_path(cli, environment, profile_id),
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Start(args),
        }) => run_daemon_start(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Status(args),
        }) => run_daemon_status(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Logs(args),
        }) => run_daemon_logs(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Restart(args),
        }) => run_daemon_restart(cli, environment, args).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::Stop,
        }) => run_daemon_stop(cli, environment).await,
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::ProbeRuntimes,
        }) => run_daemon_probe_runtimes(cli, environment),
        Command::Daemon(DaemonArgs {
            command: DaemonCommand::DiskUsage(args),
        }) => run_daemon_disk_usage(cli, environment, args).await,
        Command::Setup(args) => run_setup(cli, environment, args, input).await,
        Command::Update(args) => run_update(cli, environment, args).await,
        Command::Version { output } => run_version(*output),
    }
}
