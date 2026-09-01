package issueroles

import "github.com/patchbay-ai/patchbay/server/internal/issuestatus"

const (
	OwnerMember   = "member"
	ExecutorAgent = "agent"
	ExecutorTeam  = "team"
)

type ActorRef struct {
	Type string
	ID   string
}

type WorkflowViolation string

const (
	ActiveExecutorRequired WorkflowViolation = "active_executor_required"
	ReviewHandoffRequired  WorkflowViolation = "review_handoff_required"
)

func IsOwnerType(v string) bool    { return v == OwnerMember }
func IsExecutorType(v string) bool { return v == ExecutorAgent || v == ExecutorTeam }
func IsReviewerType(v string) bool { return IsOwnerType(v) || IsExecutorType(v) }

func ValidatePair(typeValue, idValue, field string, typeOK func(string) bool) string {
	hasType := typeValue != ""
	hasID := idValue != ""
	if hasType != hasID {
		return field + " type and id must be set together"
	}
	if hasType && !typeOK(typeValue) {
		return "invalid " + field + " type"
	}
	return ""
}

func LeavesReviewForImplementation(previousCategory, nextCategory string) bool {
	return previousCategory == issuestatus.InReview && nextCategory == issuestatus.InProgress
}

func WorkflowGate(previousCategory, nextCategory string, nextExecutor, nextReviewer *ActorRef) *WorkflowViolation {
	if issuestatus.RequiresExecutor(nextCategory) && nextExecutor == nil {
		v := ActiveExecutorRequired
		return &v
	}
	if previousCategory != issuestatus.InReview && nextCategory == issuestatus.InReview {
		if nextReviewer == nil || equal(nextReviewer, nextExecutor) {
			v := ReviewHandoffRequired
			return &v
		}
	}
	return nil
}

func equal(a, b *ActorRef) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return a.Type == b.Type && a.ID == b.ID
}
