ALTER TABLE issue_pull_request
    ADD CONSTRAINT issue_pull_request_pkey
    PRIMARY KEY USING INDEX issue_pull_request_restore_uidx;
