package handler

import (
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func dependencyGraphValidationFixture(edges []dependencyGraphEdgeInput) dependencyGraphApplyInput {
	return dependencyGraphApplyInput{
		Goal:          "ship the dependency graph",
		ParentIssueID: "00000000-0000-0000-0000-000000000001",
		Tasks: []dependencyGraphTaskInput{
			{TempID: "a", Title: "first", AcceptanceCriteria: []string{"first is complete"}, Outputs: []string{"first output"}},
			{TempID: "b", Title: "second", AcceptanceCriteria: []string{"second is complete"}, Outputs: []string{"second output"}},
			{TempID: "c", Title: "third", AcceptanceCriteria: []string{"third is complete"}, Outputs: []string{"third output"}},
		},
		Edges: edges,
	}
}

func TestValidateDependencyGraphPlanRejectsCycle(t *testing.T) {
	input := dependencyGraphValidationFixture([]dependencyGraphEdgeInput{
		{From: "a", To: "b", Type: dependencyGraphHardType, Reason: "b needs a", ConsumedOutput: "first output"},
		{From: "b", To: "a", Type: dependencyGraphHardType, Reason: "a needs b", ConsumedOutput: "second output"},
	})
	_, err := validateDependencyGraphPlan(&input)
	if err == nil || !strings.Contains(err.Error(), "cycle") {
		t.Fatal("validateDependencyGraphPlan accepted a cyclic graph")
	}
}

func TestValidateDependencyGraphPlanRejectsTransitiveEdge(t *testing.T) {
	input := dependencyGraphValidationFixture([]dependencyGraphEdgeInput{
		{From: "a", To: "b", Type: dependencyGraphHardType, Reason: "b needs a", ConsumedOutput: "first output"},
		{From: "b", To: "c", Type: dependencyGraphHardType, Reason: "c needs b", ConsumedOutput: "second output"},
		{From: "a", To: "c", Type: dependencyGraphHardType, Reason: "c needs a", ConsumedOutput: "first output"},
	})
	if _, err := validateDependencyGraphPlan(&input); err == nil {
		t.Fatal("validateDependencyGraphPlan accepted a transitively redundant edge")
	}
}

func TestDependencyGraphCursorProjectMismatch(t *testing.T) {
	projectA := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	projectB := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	plan := db.DependencyGraphPlan{
		ID:        pgtype.UUID{Bytes: [16]byte{3}, Valid: true},
		UpdatedAt: pgtype.Timestamptz{Time: time.Unix(10, 0).UTC(), Valid: true},
	}
	cursor, err := encodeDependencyGraphCursor(&projectA, plan)
	if err != nil {
		t.Fatalf("encode cursor: %v", err)
	}
	_, err = decodeDependencyGraphCursor(cursor, &projectB)
	var graphErr *dependencyGraphError
	if !errors.As(err, &graphErr) || graphErr.code != "cursor_project_mismatch" {
		t.Fatalf("decode cursor error = %v, want cursor_project_mismatch", err)
	}
}
