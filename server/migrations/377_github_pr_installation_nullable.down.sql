-- App-less PRs cannot be represented by the pre-377 schema. Remove their
-- dependent rows explicitly before restoring the old NOT NULL constraint.
DELETE FROM github_pull_request_check_run
WHERE pr_id IN (
    SELECT id FROM github_pull_request WHERE installation_id IS NULL
);

DELETE FROM github_pull_request_check_suite
WHERE pr_id IN (
    SELECT id FROM github_pull_request WHERE installation_id IS NULL
);

DELETE FROM issue_pull_request
WHERE pull_request_id IN (
    SELECT id FROM github_pull_request WHERE installation_id IS NULL
);

DELETE FROM github_pull_request
WHERE installation_id IS NULL;

ALTER TABLE github_pull_request
    ALTER COLUMN installation_id SET NOT NULL;
