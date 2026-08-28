use super::*;
use axum::extract::Request;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use std::fs;
use std::io::Cursor;
use tokio::net::TcpListener;

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
    environment.set("PATCHBAY_SERVER_URL", format!("http://{address}"));
    environment.set("PATCHBAY_WORKSPACE_ID", "workspace-1");
    environment.set("PATCHBAY_TASK_ID", "task-1");
    environment.set("PATCHBAY_TOKEN", "mat_test-token");

    let upload = Cli::try_parse_from(["patchbay", "attachment", "upload", "chart[v2].png"])
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
        "patchbay",
        "attachment",
        "download",
        "attachment-1",
        "-o",
        "attachments",
    ])
    .expect("attachment download CLI");
    let downloaded = run_with_input(&download, &environment, &mut Cursor::new(Vec::<u8>::new()))
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
    let downloaded_json: Value = serde_json::from_str(&downloaded.stdout).expect("download JSON");
    assert_eq!(downloaded_json["filename"], "report.txt");
    assert_eq!(downloaded_json["size"], "15");
    assert!(!downloaded.stdout.contains("../"));
    server.abort();
}
