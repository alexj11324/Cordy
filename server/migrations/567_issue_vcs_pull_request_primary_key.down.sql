ALTER TABLE issue_vcs_pull_request
    ADD CONSTRAINT issue_vcs_pull_request_pkey
    PRIMARY KEY USING INDEX issue_vcs_pull_request_restore_uidx;
