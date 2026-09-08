package main

import (
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/handler"
)

// IssueService publishes typed facts. Other current producers retain their
// existing contracts: dependency-graph creation uses the HTTP DTO, while
// Automation publishes a map and owns its own creation notifications. Keep
// those adapters here rather than making the business listeners interpret UI
// payloads or changing Automation's notification behavior in this slice.
func issueCreatedForSideEffects(event events.Event) (events.IssueCreated, bool) {
	if event.IssueCreated != nil {
		return *event.IssueCreated, true
	}
	payload, ok := event.Payload.(map[string]any)
	if !ok {
		return events.IssueCreated{}, false
	}
	issue, ok := payload["issue"].(handler.IssueResponse)
	return issueCreatedFromResponse(issue), ok
}

func issueCreatedForSubscribers(event events.Event) (events.IssueCreated, bool) {
	if event.IssueCreated != nil {
		return *event.IssueCreated, true
	}
	payload, ok := event.Payload.(map[string]any)
	if !ok {
		return events.IssueCreated{}, false
	}
	issue, ok := extractIssueFields(payload["issue"])
	return issueCreatedFromResponse(issue), ok
}

func issueCreatedFromResponse(issue handler.IssueResponse) events.IssueCreated {
	return events.IssueCreated{
		ID: issue.ID, WorkspaceID: issue.WorkspaceID, Title: issue.Title, Status: issue.Status,
		CreatorType: issue.CreatorType, CreatorID: issue.CreatorID, Description: issue.Description,
		OwnerType: issue.OwnerType, OwnerID: issue.OwnerID,
		ExecutorType: issue.ExecutorType, ExecutorID: issue.ExecutorID,
		ReviewerType: issue.ReviewerType, ReviewerID: issue.ReviewerID,
	}
}
