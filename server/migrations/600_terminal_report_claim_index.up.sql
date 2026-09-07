CREATE UNIQUE INDEX CONCURRENTLY terminal_report_receipt_claim_idx ON terminal_report_receipt (task_id, claim_fence);
