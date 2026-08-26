use super::cli_test_helpers::*;
use super::*;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
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
