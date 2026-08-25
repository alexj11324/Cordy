use super::*;

pub(super) fn update_args(cli: &Cli) -> &UpdateProfileArgs {
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

pub(super) fn create_workspace_args(cli: &Cli) -> &CreateWorkspaceArgs {
    match &cli.command {
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Create(args),
        }) => args,
        _ => panic!("expected workspace create"),
    }
}

pub(super) fn update_workspace_args(cli: &Cli) -> &UpdateWorkspaceArgs {
    match &cli.command {
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Update(args),
        }) => args,
        _ => panic!("expected workspace update"),
    }
}

pub(super) fn issue_list_args(cli: &Cli) -> &IssueListArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::List(args),
        }) => args,
        _ => panic!("expected issue list"),
    }
}

pub(super) fn issue_create_args(cli: &Cli) -> &IssueCreateArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Create(args),
        }) => args,
        _ => panic!("expected issue create"),
    }
}

pub(super) fn issue_update_args(cli: &Cli) -> &IssueUpdateArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Update(args),
        }) => args,
        _ => panic!("expected issue update"),
    }
}

pub(super) fn issue_assign_args(cli: &Cli) -> &IssueAssignArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Assign(args),
        }) => args,
        _ => panic!("expected issue assign"),
    }
}

pub(super) fn issue_status_args(cli: &Cli) -> &IssueStatusArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Status(args),
        }) => args,
        _ => panic!("expected issue status"),
    }
}

pub(super) fn issue_reorder_args(cli: &Cli) -> &IssueReorderArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Reorder(args),
        }) => args,
        _ => panic!("expected issue reorder"),
    }
}

pub(super) fn issue_comment_add_args(cli: &Cli) -> &IssueCommentAddArgs {
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

pub(super) fn issue_comment_list_args(cli: &Cli) -> &IssueCommentListArgs {
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

pub(super) fn issue_runs_args(cli: &Cli) -> &IssueRunsArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Runs(args),
        }) => args,
        _ => panic!("expected issue runs"),
    }
}

pub(super) fn issue_run_messages_args(cli: &Cli) -> &IssueRunMessagesArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::RunMessages(args),
        }) => args,
        _ => panic!("expected issue run-messages"),
    }
}

pub(super) fn issue_cancel_task_args(cli: &Cli) -> &IssueCancelTaskArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::CancelTask(args),
        }) => args,
        _ => panic!("expected issue cancel-task"),
    }
}

pub(super) fn issue_usage_args(cli: &Cli) -> &IssueUsageArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Usage(args),
        }) => args,
        _ => panic!("expected issue usage"),
    }
}

pub(super) fn issue_rerun_args(cli: &Cli) -> &IssueRerunArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Rerun(args),
        }) => args,
        _ => panic!("expected issue rerun"),
    }
}

pub(super) fn issue_search_args(cli: &Cli) -> &IssueSearchArgs {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::Search(args),
        }) => args,
        _ => panic!("expected issue search"),
    }
}
