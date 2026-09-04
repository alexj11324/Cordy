CREATE UNIQUE INDEX CONCURRENTLY issue_vcs_pull_request_restore_uidx
    ON issue_vcs_pull_request (issue_id, pull_request_id);
