package daemon

import (
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
)

// repoCheckoutModeFor picks the Git metadata layout for a task's
// `patchbay repo checkout`. Under Codex's workspace-write sandbox a linked
// worktree's gitdir resolves into the shared cache and stays read-only even
// when the task workdir is an explicit writable root, so `git add` /
// `git commit` fail from inside the checkout — Linux hit this in
// patchbay-ai/patchbay#2925, Codex's native Windows sandbox in
// patchbay-ai/patchbay#6449.
//
// Both platforms now default to danger-full-access (execenv's
// codexSandboxPolicyFor), so in practice only a user who opted into
// windows.sandbox still trips the Windows case. The layout stays a per-platform
// choice rather than a per-policy one: it is decided before a task's resolved
// sandbox config is known, one workdir is reused across tasks whose policies
// can differ, and task-local metadata is correct under either policy.
func repoCheckoutModeFor(provider, goos string) string {
	if provider != "codex" {
		return ""
	}
	switch goos {
	case "linux", "windows":
		return repoCheckoutModeIsolated
	default:
		return ""
	}
}

func validateTaskIdentity(task Task) error {
	if strings.TrimSpace(task.AgentID) == "" {
		return fmt.Errorf("%w: task %s has no authoritative agent_id", errInvalidTaskIdentity, task.ID)
	}
	if task.Agent == nil {
		return fmt.Errorf("%w: task %s has no agent payload (agent_id=%s)", errInvalidTaskIdentity, task.ID, task.AgentID)
	}
	if task.Agent.ID != task.AgentID {
		return fmt.Errorf("%w: task %s agent_id=%s but agent.id=%s", errInvalidTaskIdentity, task.ID, task.AgentID, task.Agent.ID)
	}
	return nil
}

func taskScopedAuthToken(task Task) (string, error) {
	token := strings.TrimSpace(task.AuthToken)
	if token == "" {
		return "", errors.New("server did not provide task-scoped auth token")
	}
	if !strings.HasPrefix(token, "mat_") {
		return "", errors.New("server provided non-task-scoped auth token")
	}
	return token, nil
}

func taskOrviloEnvironment(task Task, agentName, token, configRoot, workspacesRoot, serverURL string, healthPort, slot int, tempDir string) map[string]string {
	return map[string]string{
		"ORVILO_TOKEN":        token,
		cli.TaskConfigRootEnv: configRoot,
		TaskWorkspacesRootEnv: workspacesRoot,
		"ORVILO_SERVER_URL":   serverURL,
		"ORVILO_DAEMON_PORT":  strconv.Itoa(healthPort),
		"ORVILO_WORKSPACE_ID": task.WorkspaceID,
		"ORVILO_AGENT_NAME":   agentName,
		"ORVILO_AGENT_ID":     task.AgentID,
		"ORVILO_TASK_ID":      task.ID,
		"ORVILO_TASK_SLOT":    strconv.Itoa(slot),
		"TMPDIR":              tempDir,
		"TMP":                 tempDir,
		"TEMP":                tempDir,
	}
}

// shortID returns the first 8 characters of an ID for readable logs.
func shortID(id string) string {
	if len(id) <= 8 {
		return id
	}
	return id[:8]
}

// truncateLog truncates a string to maxLen, appending "…" if truncated.
// Also collapses newlines to spaces for single-line log output.
func truncateLog(s string, maxLen int) string {
	s = strings.ReplaceAll(s, "\n", " ")
	s = strings.TrimSpace(s)
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "…"
}
