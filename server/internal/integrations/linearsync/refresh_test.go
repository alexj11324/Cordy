package linearsync

import (
	"context"
	"errors"
	"net/http"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/integrations/linear"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestLinearWorkerRetriesTransientRefreshFailure(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
	}{
		{"network", errors.New("connection reset by peer")},
		{"timeout", context.DeadlineExceeded},
		{"rate limit", &linear.ProviderError{Kind: linear.ErrorRateLimited, Status: http.StatusTooManyRequests, Message: "rate limited"}},
		{"server error", &linear.ProviderError{Kind: linear.ErrorProvider, Status: http.StatusBadGateway, Message: "upstream unavailable"}},
		{"invalid response", &linear.ProviderError{Kind: linear.ErrorInvalidResponse, Message: "missing access_token"}},
		{"unclassified error text", errors.New("invalid_grant")},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx := context.Background()
			api := &fakeLinearAPI{authErr: tc.err}
			f := setupWorker(t, "publish", api)
			dbfx.Exec(t, `UPDATE linear_connection SET token_expires_at=now()-interval '1 minute' WHERE id=$1`, f.connectionID)
			issueID := dbfx.Issue(t, "Retry refresh before publishing", testutil.Cols{"project_id": f.projectID})

			if !f.worker.processOneOutbox(ctx) {
				t.Fatal("worker did not claim the outbox row")
			}
			var status, lastError string
			var access, refresh []byte
			dbfx.QueryRow(t, `SELECT status,last_error,access_token_encrypted,refresh_token_encrypted FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&status, &lastError, &access, &refresh)
			if status != "active" || lastError != tc.err.Error() {
				t.Fatalf("transient failure changed connection status=%q error=%q", status, lastError)
			}
			for _, credential := range []struct {
				sealed []byte
				want   string
			}{{access, "access"}, {refresh, "refresh"}} {
				plain, err := f.box.Open(credential.sealed)
				if err != nil || string(plain) != credential.want {
					t.Fatalf("transient failure replaced stored credential: %v", err)
				}
			}
			var attempts int
			var retryIn float64
			var processed, dead, locked bool
			dbfx.QueryRow(t, `SELECT attempts,extract(epoch FROM available_at-now()),processed_at IS NOT NULL,dead_lettered_at IS NOT NULL,locked_by IS NOT NULL FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&attempts, &retryIn, &processed, &dead, &locked)
			if attempts != 1 || retryIn <= 0 || processed || dead || locked {
				t.Fatalf("refresh retry: attempts=%d delay=%v processed=%v dead=%v locked=%v", attempts, retryIn, processed, dead, locked)
			}
			if created, _, _, refreshed := api.calls(); created != 0 || refreshed != 1 {
				t.Fatalf("failed refresh calls: created=%d refreshed=%d", created, refreshed)
			}

			api.authErr = nil
			api.refresh = linear.Token{AccessToken: "fresh-access", RefreshToken: "fresh-refresh", ExpiresIn: time.Hour}
			dbfx.Exec(t, `UPDATE linear_sync_outbox SET available_at=now() WHERE issue_id=$1`, issueID)
			if !f.worker.processOneOutbox(ctx) {
				t.Fatal("worker did not retry after provider recovery")
			}
			dbfx.QueryRow(t, `SELECT processed_at IS NOT NULL FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&processed)
			dbfx.QueryRow(t, `SELECT status,coalesce(last_error,'') FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&status, &lastError)
			if !processed || status != "active" || lastError != "" {
				t.Fatalf("recovery: processed=%v status=%q error=%q", processed, status, lastError)
			}
			if created, _, _, refreshed := api.calls(); created != 1 || refreshed != 2 {
				t.Fatalf("recovery calls: created=%d refreshed=%d", created, refreshed)
			}
		})
	}
}
