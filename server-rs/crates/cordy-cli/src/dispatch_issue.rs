//! Issue command dispatch.
//!
//! All issue reads, mutations, comments, task runs, and metadata routing live
//! in one domain module so the root dispatcher only coordinates top-level
//! commands and shared input forwarding.

use std::io::Read;

use super::*;

pub(super) async fn run_issue_command<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueArgs,
    input: &mut R,
) -> Result<RunOutput> {
    match args {
        IssueArgs {
            command: IssueCommand::List(args),
        } => run_issue_list(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Get { id, output },
        } => run_issue_get(cli, environment, id, *output).await,
        IssueArgs {
            command: IssueCommand::PullRequests { id, output },
        } => run_issue_pull_requests(cli, environment, id, *output).await,
        IssueArgs {
            command:
                IssueCommand::PullRequest(IssuePullRequestArgs {
                    command: IssuePullRequestCommand::Attach(args),
                }),
        } => run_issue_pull_request_attach(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Children {
                    id,
                    output,
                    full_id,
                },
        } => run_issue_children(cli, environment, id, *output, *full_id).await,
        IssueArgs {
            command: IssueCommand::Create(args),
        } => run_issue_create(cli, environment, args, input).await,
        IssueArgs {
            command: IssueCommand::Update(args),
        } => run_issue_update(cli, environment, args, input).await,
        IssueArgs {
            command: IssueCommand::Assign(args),
        } => run_issue_assign(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Status(args),
        } => run_issue_status(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Reorder(args),
        } => run_issue_reorder(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::List(args),
                }),
        } => run_issue_comment_list(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Add(args),
                }),
        } => run_issue_comment_add(cli, environment, args, input).await,
        IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Delete { comment_id },
                }),
        } => run_issue_comment_delete(cli, environment, comment_id).await,
        IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Resolve(args),
                }),
        } => run_issue_comment_resolution(cli, environment, args, true).await,
        IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Unresolve(args),
                }),
        } => run_issue_comment_resolution(cli, environment, args, false).await,
        IssueArgs {
            command: IssueCommand::Runs(args),
        } => run_issue_runs(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::RunMessages(args),
        } => run_issue_run_messages(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Usage(args),
        } => run_issue_usage(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Rerun(args),
        } => run_issue_rerun(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::CancelTask(args),
        } => run_issue_cancel_task(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Search(args),
        } => run_issue_search(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::List { issue_id, output },
                }),
        } => run_issue_subscriber_list(cli, environment, issue_id, *output).await,
        IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Add(args),
                }),
        } => run_issue_subscriber_mutation(cli, environment, args, true).await,
        IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Remove(args),
                }),
        } => run_issue_subscriber_mutation(cli, environment, args, false).await,
        IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::List(args),
                }),
        } => run_issue_label_list(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Add(args),
                }),
        } => run_issue_label_add(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Remove(args),
                }),
        } => run_issue_label_remove(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::List(args),
                }),
        } => run_issue_metadata_list(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Get(args),
                }),
        } => run_issue_metadata_get(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Set(args),
                }),
        } => run_issue_metadata_set(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Delete(args),
                }),
        } => run_issue_metadata_delete(cli, environment, args).await,
        IssueArgs {
            command: IssueCommand::Timeline(args),
        } => run_issue_timeline(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::List(args),
                }),
        } => run_issue_property_list(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Set(args),
                }),
        } => run_issue_property_set(cli, environment, args).await,
        IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Unset(args),
                }),
        } => run_issue_property_unset(cli, environment, args).await,
    }
}
