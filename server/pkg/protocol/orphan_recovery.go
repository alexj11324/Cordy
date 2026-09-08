package protocol

// PendingTerminalReport identifies an execution whose result is already on the
// daemon's durable outbox. It is awaiting delivery, not an orphan to execute again.
type PendingTerminalReport struct {
	TaskID     string `json:"task_id"`
	ClaimFence string `json:"claim_fence"`
}

// RecoverOrphansRequest remains optional for older daemons. Exclusions apply
// only to the same runtime and exact current execution fence.
type RecoverOrphansRequest struct {
	PendingTerminalReports []PendingTerminalReport `json:"pending_terminal_reports,omitempty"`
}
