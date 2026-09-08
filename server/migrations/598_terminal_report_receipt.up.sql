CREATE TABLE terminal_report_receipt (
    report_id UUID NOT NULL,
    task_id UUID NOT NULL,
    claim_fence BIGINT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    task_status TEXT NOT NULL CHECK (task_status IN ('completed', 'failed')),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
