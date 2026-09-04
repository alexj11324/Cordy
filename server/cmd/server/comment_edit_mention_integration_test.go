package main

import (
	"fmt"
	"io"
	"testing"
)

func TestEditCommentTriggers(t *testing.T) {
	agentID := getAgentID(t)
	issueID := createIssue(t, "Edit comment triggers integration test")
	t.Cleanup(func() {
		clearTasks(t, issueID)
		resp := authRequest(t, "DELETE", "/api/issues/"+issueID, nil)
		resp.Body.Close()
	})

	t.Run("edit adds agent mention enqueues task", func(t *testing.T) {
		clearTasks(t, issueID)
		commentID := postComment(t, issueID, "plain comment no mentions", nil)
		clearTasks(t, issueID)

		newContent := fmt.Sprintf("[@Agent](mention://agent/%s) please review", agentID)
		resp := authRequest(t, "PUT", "/api/comments/"+commentID, map[string]any{
			"content": newContent,
		})
		if resp.StatusCode != 200 {
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			t.Fatalf("UpdateComment: expected 200, got %d: %s", resp.StatusCode, body)
		}
		resp.Body.Close()

		if n := countPendingTasks(t, issueID); n != 1 {
			t.Errorf("expected 1 pending task after adding agent mention via edit, got %d", n)
		}
	})

	t.Run("edit removes agent mention cancels task", func(t *testing.T) {
		clearTasks(t, issueID)
		content := fmt.Sprintf("[@Agent](mention://agent/%s) fix this", agentID)
		commentID := postComment(t, issueID, content, nil)

		if n := countPendingTasks(t, issueID); n != 1 {
			t.Fatalf("expected 1 pending task from initial mention, got %d", n)
		}

		resp := authRequest(t, "PUT", "/api/comments/"+commentID, map[string]any{
			"content": "removed the mention, nevermind",
		})
		if resp.StatusCode != 200 {
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			t.Fatalf("UpdateComment: expected 200, got %d: %s", resp.StatusCode, body)
		}
		resp.Body.Close()

		if n := countPendingTasks(t, issueID); n != 0 {
			t.Errorf("expected 0 pending tasks after removing mention via edit, got %d", n)
		}
	})

	t.Run("edit changes content but keeps same mention re-triggers", func(t *testing.T) {
		clearTasks(t, issueID)
		content := fmt.Sprintf("[@Agent](mention://agent/%s) fix bug A", agentID)
		commentID := postComment(t, issueID, content, nil)

		if n := countPendingTasks(t, issueID); n != 1 {
			t.Fatalf("expected 1 pending task from initial mention, got %d", n)
		}

		clearTasks(t, issueID)

		newContent := fmt.Sprintf("[@Agent](mention://agent/%s) actually fix bug B instead", agentID)
		resp := authRequest(t, "PUT", "/api/comments/"+commentID, map[string]any{
			"content": newContent,
		})
		if resp.StatusCode != 200 {
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			t.Fatalf("UpdateComment: expected 200, got %d: %s", resp.StatusCode, body)
		}
		resp.Body.Close()

		if n := countPendingTasks(t, issueID); n != 1 {
			t.Errorf("expected 1 pending task after content change re-trigger, got %d", n)
		}
	})

	t.Run("edit on agent-executor issue cancels and re-triggers executor task", func(t *testing.T) {
		executorIssue := createIssueWithAgentExecutor(t, "Edit executor trigger test", agentID)
		t.Cleanup(func() {
			clearTasks(t, executorIssue)
			resp := authRequest(t, "DELETE", "/api/issues/"+executorIssue, nil)
			resp.Body.Close()
		})
		clearTasks(t, executorIssue)

		commentID := postComment(t, executorIssue, "fix the login page", nil)
		if n := countPendingTasks(t, executorIssue); n != 1 {
			t.Fatalf("expected 1 pending task from on_comment trigger, got %d", n)
		}

		clearTasks(t, executorIssue)

		resp := authRequest(t, "PUT", "/api/comments/"+commentID, map[string]any{
			"content": "actually fix the signup page instead",
		})
		if resp.StatusCode != 200 {
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			t.Fatalf("UpdateComment: expected 200, got %d: %s", resp.StatusCode, body)
		}
		resp.Body.Close()

		if n := countPendingTasks(t, executorIssue); n != 1 {
			t.Errorf("expected 1 pending task after edit re-triggered executor, got %d", n)
		}
	})
}
