CREATE INDEX CONCURRENTLY idx_issue_pull_request_pr
    ON issue_pull_request (pull_request_id);
