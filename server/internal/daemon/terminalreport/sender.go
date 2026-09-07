package terminalreport

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"
	"time"

	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// Transport sends once. Retry ownership belongs to Sender, independently of
// task execution, the provider process and the old finite HTTP retry budget.
type Transport interface {
	SendTerminalReport(context.Context, Report) (protocol.TerminalReportAck, error)
}

// PermanentError is a validated protocol rejection. Authentication, timeouts,
// throttling, unavailable endpoints and malformed replies remain pending.
type PermanentError struct{ Err error }

func (e *PermanentError) Error() string { return e.Err.Error() }
func (e *PermanentError) Unwrap() error { return e.Err }

type retryState struct {
	attempts int
	after    time.Time
}

type Sender struct {
	store     *Store
	transport Transport
	logger    *slog.Logger
	wake      chan struct{}
	mu        sync.Mutex
	retries   map[string]retryState
	now       func() time.Time
	statsMu   sync.RWMutex
	pending   int
	oldest    time.Time
}

func NewSender(store *Store, transport Transport, logger *slog.Logger) *Sender {
	return &Sender{store: store, transport: transport, logger: logger, wake: make(chan struct{}, 1), retries: make(map[string]retryState), now: time.Now}
}

func (s *Sender) Wake() {
	select {
	case s.wake <- struct{}{}:
	default:
	}
}

// Run replays on startup and periodically, so losing the wake hint cannot lose
// a report. Shutdown may interrupt a request; its durable file survives.
func (s *Sender) Run(ctx context.Context) {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		if ctx.Err() != nil {
			return
		}
		if err := s.Flush(ctx); err != nil && ctx.Err() == nil {
			s.logger.Error("terminal report queue needs attention", "error", err)
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		case <-s.wake:
		}
	}
}

// Flush executes at most one attempt per due report. The clock and transport
// seams allow deterministic outage and response-loss tests without sleeping.
func (s *Sender) Flush(ctx context.Context) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	reports, readErr := s.store.Pending()
	s.setStats(reports)
	var failures []error
	if readErr != nil {
		failures = append(failures, readErr)
	}
	for _, report := range reports {
		if err := ctx.Err(); err != nil {
			return errors.Join(append(failures, err)...)
		}
		id := report.Identity.ReportID
		state := s.retries[id]
		if s.now().Before(state.after) {
			continue
		}
		ack, err := s.transport.SendTerminalReport(ctx, report)
		if err == nil {
			err = validateAck(report, ack)
		}
		if err == nil {
			if err := s.store.Confirm(report); err != nil {
				failures = append(failures, fmt.Errorf("remove acknowledged report %s: %w", id, err))
				continue
			}
			delete(s.retries, id)
			s.logger.Info("terminal report acknowledged", "task_id", report.TaskID, "report_id", id)
			continue
		}
		var rejection *PermanentError
		if errors.As(err, &rejection) {
			if err := s.store.Reject(report); err != nil {
				failures = append(failures, err)
				continue
			}
			delete(s.retries, id)
			s.logger.Error("terminal report rejected; result retained", "task_id", report.TaskID, "report_id", id, "error", rejection)
			continue
		}
		state.attempts++
		state.after = s.now().Add(retryDelay(state.attempts))
		s.retries[id] = state
		s.logger.Warn("terminal report remains pending", "task_id", report.TaskID, "report_id", id, "attempt", state.attempts, "retry_at", state.after, "error", err)
	}
	remaining, err := s.store.Pending()
	s.setStats(remaining)
	return errors.Join(append(failures, err)...)
}

func validateAck(report Report, ack protocol.TerminalReportAck) error {
	if ack.TerminalReportIdentity != report.Identity || ack.TaskID != report.TaskID || ack.Status != "accepted" || (ack.TaskStatus != "completed" && ack.TaskStatus != "failed") {
		return errors.New("terminal report acknowledgement does not match persisted result")
	}
	return nil
}

func retryDelay(attempt int) time.Duration {
	if attempt >= 6 {
		return time.Minute
	}
	return time.Second * time.Duration(1<<attempt)
}

func (s *Sender) setStats(reports []Report) {
	s.statsMu.Lock()
	defer s.statsMu.Unlock()
	s.pending = len(reports)
	s.oldest = time.Time{}
	if len(reports) > 0 {
		s.oldest = reports[0].CreatedAt
	}
}

func (s *Sender) Stats() (pending int, oldestAge time.Duration) {
	s.statsMu.RLock()
	defer s.statsMu.RUnlock()
	if !s.oldest.IsZero() {
		oldestAge = time.Since(s.oldest)
		if oldestAge < 0 {
			oldestAge = 0
		}
	}
	return s.pending, oldestAge
}
