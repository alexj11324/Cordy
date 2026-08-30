use std::io::Read;

use super::*;

pub(super) async fn run_with_input<R: Read>(
    cli: &Cli,
    environment: &Environment,
    input: &mut R,
) -> Result<RunOutput> {
    match &cli.command {
        Command::Agent(AgentArgs {
            command:
                AgentCommand::List {
                    output,
                    include_archived,
                },
        }) => run_agent_list(cli, environment, *output, *include_archived).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Get { id, output },
        }) => run_agent_get(cli, environment, id, *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) => run_agent_create(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Update(args),
        }) => run_agent_update(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Archive { id, output },
        }) => run_agent_lifecycle(cli, environment, id, "archive", "archived", *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Restore { id, output },
        }) => run_agent_lifecycle(cli, environment, id, "restore", "restored", *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Tasks { id, output },
        }) => run_agent_tasks(cli, environment, id, *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Avatar { id, file, output },
        }) => run_agent_avatar(cli, environment, id, file.as_deref(), *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::List { agent_id, output },
                }),
        }) => run_agent_skills_list(cli, environment, agent_id, *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Set(args),
                }),
        }) => run_agent_skills_mutation(cli, environment, args, false).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Add(args),
                }),
        }) => run_agent_skills_mutation(cli, environment, args, true).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Get { agent_id, output },
                }),
        }) => run_agent_env_get(cli, environment, agent_id, *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Set(args),
                }),
        }) => run_agent_env_set(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::List(args),
                }),
        }) => run_agent_mcp_list(cli, environment, args).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Add(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Add).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Enable(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Enable).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Disable(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Disable).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Remove(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Remove).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Copy(args),
        }) => run_agent_copy(cli, environment, args, input).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::List { output },
        }) => run_skill_list(cli, environment, *output).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Get(args),
        }) => run_skill_get(cli, environment, args).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Create(args),
        }) => run_skill_create(cli, environment, args, input).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Update(args),
        }) => run_skill_update(cli, environment, args, input).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Delete(args),
        }) => run_skill_delete(cli, environment, args, input).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Import(args),
        }) => run_skill_import(cli, environment, args).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Refresh(args),
        }) => run_skill_refresh(cli, environment, args).await,
        Command::Skill(SkillArgs {
            command: SkillCommand::Search(args),
        }) => run_skill_search(cli, environment, args).await,
        Command::Skill(SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::List(args),
                }),
        }) => run_skill_files_list(cli, environment, args).await,
        Command::Skill(SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::Upsert(args),
                }),
        }) => run_skill_files_upsert(cli, environment, args, input).await,
        Command::Skill(SkillArgs {
            command:
                SkillCommand::Files(SkillFilesArgs {
                    command: SkillFilesCommand::Delete(args),
                }),
        }) => run_skill_files_delete(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::List {
                    status,
                    output,
                    full_id,
                },
        }) => run_autopilot_list(cli, environment, status, *output, *full_id).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Get { id, output },
        }) => run_autopilot_get(cli, environment, id, *output).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Create(args),
        }) => run_autopilot_create(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Update(args),
        }) => run_autopilot_update(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Delete { id },
        }) => run_autopilot_delete(cli, environment, id).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Trigger { id, output },
        }) => run_autopilot_trigger(cli, environment, id, *output).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::Runs {
                    id,
                    limit,
                    offset,
                    output,
                },
        }) => run_autopilot_runs(cli, environment, id, *limit, *offset, *output).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerAdd(args),
        }) => run_autopilot_trigger_add(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerUpdate(args),
        }) => run_autopilot_trigger_update(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::TriggerDelete {
                    autopilot_id,
                    trigger_id,
                },
        }) => run_autopilot_trigger_delete(cli, environment, autopilot_id, trigger_id).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerRotateUrl(args),
        }) => run_autopilot_trigger_rotate_url(cli, environment, args, input).await,
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
            command:
                IssueCommand::DependencyGraph(IssueDependencyGraphArgs {
                    command: IssueDependencyGraphCommand::Get { id, output },
                }),
        }) => run_issue_dependency_graph_get(cli, environment, id, *output).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::DependencyGraph(IssueDependencyGraphArgs {
                    command: IssueDependencyGraphCommand::Apply(args),
                }),
        }) => run_issue_dependency_graph_apply(cli, environment, args, input).await,
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
            command: IssueCommand::MessageMain(args),
        }) => run_issue_message_main(cli, environment, args, input).await,
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
        Command::Team(TeamArgs {
            command: TeamCommand::List { output },
        }) => run_team_list(cli, environment, *output).await,
        Command::Team(TeamArgs {
            command: TeamCommand::Get { team_id, output },
        }) => run_team_get(cli, environment, team_id, *output).await,
        Command::Team(TeamArgs {
            command: TeamCommand::Create(args),
        }) => run_team_create(cli, environment, args).await,
        Command::Team(TeamArgs {
            command: TeamCommand::Update(args),
        }) => run_team_update(cli, environment, args).await,
        Command::Team(TeamArgs {
            command: TeamCommand::Delete { team_id, output },
        }) => run_team_delete(cli, environment, team_id, *output).await,
        Command::Team(TeamArgs {
            command:
                TeamCommand::Member(TeamMemberArgs {
                    command: TeamMemberCommand::List { team_id, output },
                }),
        }) => run_team_member_list(cli, environment, team_id, *output).await,
        Command::Team(TeamArgs {
            command:
                TeamCommand::Member(TeamMemberArgs {
                    command: TeamMemberCommand::Add(args),
                }),
        }) => run_team_member_add(cli, environment, args).await,
        Command::Team(TeamArgs {
            command:
                TeamCommand::Member(TeamMemberArgs {
                    command: TeamMemberCommand::SetRole(args),
                }),
        }) => run_team_member_set_role(cli, environment, args).await,
        Command::Team(TeamArgs {
            command:
                TeamCommand::Member(TeamMemberArgs {
                    command: TeamMemberCommand::Remove(args),
                }),
        }) => run_team_member_remove(cli, environment, args).await,
        Command::Team(TeamArgs {
            command: TeamCommand::Activity(args),
        }) => run_team_activity(cli, environment, args).await,
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
        Command::Completion { shell } => run_completion(*shell),
        Command::Version { output } => run_version(*output),
    }
}
