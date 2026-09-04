CREATE TABLE issue_pull_request (
    issue_id UUID NOT NULL,
    pull_request_id UUID NOT NULL,
    linked_by_type TEXT,
    linked_by_id UUID,
    linked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    close_intent BOOLEAN NOT NULL DEFAULT FALSE,
    reference_only BOOLEAN NOT NULL DEFAULT FALSE
);
