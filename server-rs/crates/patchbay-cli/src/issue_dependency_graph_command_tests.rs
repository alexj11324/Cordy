use super::issue_dependency_graph_commands::format_dependency_graph_table;
use super::*;
use clap::Parser;

#[test]
fn dependency_graph_apply_accepts_one_complete_plan_source() {
    let cli = Cli::try_parse_from([
        "patchbay",
        "issue",
        "dependency-graph",
        "apply",
        "CORD-123",
        "--idempotency-key",
        "goal-123-v1",
        "--plan-stdin",
    ])
    .expect("dependency graph apply CLI");
    let Command::Issue(IssueArgs {
        command:
            IssueCommand::DependencyGraph(IssueDependencyGraphArgs {
                command: IssueDependencyGraphCommand::Apply(args),
            }),
    }) = cli.command
    else {
        panic!("expected dependency graph apply command");
    };
    assert_eq!(args.parent, "CORD-123");
    assert_eq!(args.idempotency_key, "goal-123-v1");
    assert!(args.plan_stdin);
    assert!(args.plan_file.is_none());
}

#[test]
fn dependency_graph_apply_rejects_two_plan_sources() {
    assert!(Cli::try_parse_from([
        "patchbay",
        "issue",
        "dependency-graph",
        "apply",
        "CORD-123",
        "--idempotency-key",
        "goal-123-v1",
        "--plan-file",
        "plan.json",
        "--plan-stdin",
    ])
    .is_err());
}

#[test]
fn dependency_graph_table_exposes_readiness_counts() {
    let output = format_dependency_graph_table(&serde_json::json!({
        "plan": {"id": "plan-1", "goal": "Ship graph", "status": "active"},
        "nodes": [
            {"temp_id": "root", "title": "Root", "status": "todo", "readiness": {"state": "ready", "satisfied_prerequisites": 0, "total_prerequisites": 0}},
            {"temp_id": "child", "title": "Child", "status": "blocked", "readiness": {"state": "blocked", "satisfied_prerequisites": 0, "total_prerequisites": 1}}
        ]
    }));
    assert!(output.contains("PLAN"));
    assert!(output.contains("1"));
    assert!(output.contains("root"));
    assert!(output.contains("child"));
}
