CREATE INDEX CONCURRENTLY idx_issue_vcs_pull_request_pr
    ON issue_vcs_pull_request (pull_request_id);
