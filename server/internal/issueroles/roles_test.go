package issueroles

import (
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
)

func TestWorkflowGate_ActiveExecutorRequired(t *testing.T) {
	t.Parallel()
	got := WorkflowGate(issuestatus.Todo, issuestatus.InProgress, nil, nil)
	if got == nil || *got != ActiveExecutorRequired {
		t.Fatalf("todo→in_progress without executor: got %v", got)
	}
}

func TestWorkflowGate_ReviewRequiresDistinctReviewer(t *testing.T) {
	t.Parallel()
	executor := &ActorRef{Type: ExecutorAgent, ID: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}
	got := WorkflowGate(issuestatus.InProgress, issuestatus.InReview, executor, executor)
	if got == nil || *got != ReviewHandoffRequired {
		t.Fatalf("executor==reviewer: got %v", got)
	}
	got = WorkflowGate(issuestatus.InProgress, issuestatus.InReview, executor, nil)
	if got == nil || *got != ReviewHandoffRequired {
		t.Fatalf("missing reviewer: got %v", got)
	}
	reviewer := &ActorRef{Type: ExecutorAgent, ID: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"}
	if WorkflowGate(issuestatus.InProgress, issuestatus.InReview, executor, reviewer) != nil {
		t.Fatal("distinct reviewer should pass")
	}
}

func TestWorkflowGate_TerminalStatuses(t *testing.T) {
	t.Parallel()
	if WorkflowGate(issuestatus.Todo, issuestatus.Done, nil, nil) != nil {
		t.Fatal("todo→done must not require an executor")
	}
	if WorkflowGate(issuestatus.InProgress, issuestatus.Cancelled, nil, nil) != nil {
		t.Fatal("in_progress→cancelled must not require an executor")
	}
}

func TestValidatePair(t *testing.T) {
	t.Parallel()
	if err := ValidatePair(OwnerMember, "id-1", "owner", IsOwnerType); err != "" {
		t.Fatalf("valid owner: %s", err)
	}
	if err := ValidatePair(ExecutorAgent, "id-1", "owner", IsOwnerType); err == "" {
		t.Fatal("agent owner should be rejected")
	}
	if err := ValidatePair(OwnerMember, "", "owner", IsOwnerType); err == "" {
		t.Fatal("type without id should be rejected")
	}
	if err := ValidatePair(ExecutorTeam, "id-1", "executor", IsExecutorType); err != "" {
		t.Fatalf("valid team executor: %s", err)
	}
	if err := ValidatePair(OwnerMember, "id-1", "executor", IsExecutorType); err == "" {
		t.Fatal("member executor should be rejected")
	}
}

func TestLeavesReviewForImplementation(t *testing.T) {
	t.Parallel()
	if !LeavesReviewForImplementation(issuestatus.InReview, issuestatus.InProgress) {
		t.Fatal("expected review return")
	}
	if LeavesReviewForImplementation(issuestatus.InProgress, issuestatus.InReview) {
		t.Fatal("did not expect review return")
	}
}
