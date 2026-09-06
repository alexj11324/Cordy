package daemon

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"sync/atomic"
	"time"

	"github.com/patchbay-ai/patchbay/server/pkg/agent"
	"github.com/patchbay-ai/patchbay/server/pkg/taskfailure"
)

func providerFailureReason(result agent.Result) taskfailure.Reason {
	switch result.ProviderErrorCode {
	case "usageLimitExceeded", "sessionBudgetExceeded", "billing_error":
		return taskfailure.ReasonAgentProviderQuotaLimit
	case "rateLimitExceeded", "serverOverloaded", "rate_limit", "overloaded":
		return taskfailure.ReasonAgentProviderCapacityOrRateLimit
	case "unauthorized", "authentication_failed", "oauth_org_not_allowed", "account_on_hold":
		return taskfailure.ReasonAgentProviderAuthOrAccess
	}
	for _, reason := range taskfailure.AllReasons() {
		if reason.IsAgentError() && reason.String() == result.ProviderErrorCode {
			return reason
		}
	}
	return taskfailure.Classify(result.Error)
}

func canRecoverCapacity(result agent.Result) bool {
	if result.Status != "failed" || !result.RecoveryResumeSafe || result.SessionID == "" || result.ResumeRejected {
		return false
	}
	// Apply the same permanent-request guard used by terminal task reporting
	// before a transient code can admit the run into unbounded recovery.
	if _, poisoned := classifyPoisonedError(result.Error); poisoned {
		return false
	}
	// Structured errors take precedence over human-readable messages. Unknown
	// variants fail closed; a bare 429 is not evidence of temporary rate limiting.
	if result.ProviderErrorCode != "" {
		return result.ProviderErrorCode == "serverOverloaded" || result.ProviderErrorCode == "rateLimitExceeded" || result.ProviderErrorCode == "rate_limit" || result.ProviderErrorCode == "overloaded"
	}
	if taskfailure.Classify(result.Error) != taskfailure.ReasonAgentProviderCapacityOrRateLimit {
		return false
	}
	lower := strings.ToLower(result.Error)
	return strings.Contains(lower, "selected model is at capacity") || strings.Contains(lower, "no capacity available") || strings.Contains(lower, "overloaded") || strings.Contains(lower, "rate limit") || strings.Contains(lower, "rate_limit")
}

func capacityRecoveryDelay(attempt int) time.Duration {
	delays := [...]time.Duration{15 * time.Second, 30 * time.Second, time.Minute, 2 * time.Minute, 5 * time.Minute}
	if attempt >= len(delays) {
		return delays[len(delays)-1]
	}
	return delays[attempt]
}

const capacityRecoveryPrompt = "Continue the interrupted task from the current conversation and working directory. Check the actual progress and the outcome of any previous tool action before repeating it. Do not restart completed work."

// recoverCapacity keeps the task lease and cancellation context while the
// provider is busy. Each executeAndDrain has already joined the old process and
// flushed its messages. Waiting happens outside its inactivity watchdog. Only
// a real terminal result finishes recovery; a successful handshake does not.
func (d *Daemon) recoverCapacity(ctx context.Context, backend agent.Backend, result agent.Result, tools int32, opts agent.ExecOptions, logger *slog.Logger, taskID, codexHome string, seq *atomic.Int32, wait func(context.Context, time.Duration) error) (agent.Result, int32, error) {
	for attempt := 0; canRecoverCapacity(result); attempt++ {
		if ctx.Err() != nil {
			result.Status = "cancelled"
			return result, tools, nil
		}
		delay := capacityRecoveryDelay(attempt)
		logger.Warn("model capacity recovery waiting", "task_id", taskID, "session_id", result.SessionID, "attempt", attempt+1, "delay", delay, "error", result.Error)
		// Use the existing transcript channel so all clients can see why the task
		// remains active without introducing a new task state or UI-only timer.
		notice := fmt.Sprintf("\n\nModel temporarily busy. Progress is preserved; retrying in %s (attempt %d). You can stop this task at any time.\n\n", delay, attempt+1)
		reportCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
		reportErr := d.client.ReportTaskMessages(reportCtx, taskID, []TaskMessageData{{Seq: int(seq.Add(1)), Type: "thinking", Content: notice}})
		cancel()
		if reportErr != nil {
			logger.Warn("capacity recovery notice failed", "error", reportErr)
		}
		if err := wait(ctx, delay); err != nil || ctx.Err() != nil {
			result.Status = "cancelled"
			return result, tools, nil
		}
		opts.ResumeSessionID = result.SessionID
		opts.RequireResume = true
		prior := result
		next, nextTools, err := d.executeAndDrain(ctx, backend, capacityRecoveryPrompt, opts, logger, taskID, codexHome, seq)
		tools += nextTools
		if err != nil {
			// Preserve progress and usage, but stop on an unclassified launch failure.
			prior.Status = "failed"
			prior.Error = err.Error()
			prior.ProviderErrorCode = ""
			prior.RecoveryResumeSafe = false
			return prior, tools, nil
		}
		if next.UsageCumulative && next.SessionID == prior.SessionID {
			// Cursor reports authoritative totals for this same session. Keep
			// the latest snapshot, retaining models absent from that snapshot.
			totals := make(map[string]agent.TokenUsage, len(prior.Usage)+len(next.Usage))
			for model, usage := range prior.Usage {
				totals[model] = usage
			}
			sessionTotals := mergeUsage(prior.UsageOutsideSession, next.Usage)
			for model := range next.Usage {
				totals[model] = sessionTotals[model]
			}
			next.Usage = totals
		} else {
			next.Usage = mergeUsage(prior.Usage, next.Usage)
		}
		next.UsageOutsideSession = prior.UsageOutsideSession
		if next.SessionID == "" {
			next.SessionID = prior.SessionID
			next.RecoveryResumeSafe = false
		}
		result = next
	}
	return result, tools, nil
}
