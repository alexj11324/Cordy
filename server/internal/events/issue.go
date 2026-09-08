package events

// IssueCreated is the committed creation snapshot used by activity,
// subscriptions and notifications. Transport adapters own Payload separately.
type IssueCreated struct {
	ID, WorkspaceID, Title, Status string
	CreatorType, CreatorID         string
	Description                    *string
	OwnerType, OwnerID             *string
	ExecutorType, ExecutorID       *string
	ReviewerType, ReviewerID       *string
}
