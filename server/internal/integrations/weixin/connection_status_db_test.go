package weixin

import (
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

func TestWeixinSuccessfulPollConfirmsConnectionDB(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("integration test requires DATABASE_URL")
	}
	pool, err := pgxpool.New(context.Background(), dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	var calls atomic.Int64
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) > 1 {
			<-r.Context().Done()
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ret":0,"msgs":[]}`))
	}))
	defer server.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	confirmed := false
	ctx = channel.WithRuntimeReporter(ctx, func(_ context.Context, observation channel.RuntimeObservation) bool {
		confirmed = observation.State == "healthy"
		cancel()
		return true
	})
	// No receive cursor exists for this fresh ID; the real DB lookup returns
	// the empty cursor. The empty provider batch never creates a cursor row.
	adapter := &weixinChannel{installationID: dbid.NewV7(), pool: pool,
		client: NewClient(server.URL, "fixture-token", server.Client()), logger: slog.Default(),
		handler: func(context.Context, channel.InboundMessage) error { return nil }}
	if err := adapter.Connect(ctx); err != nil {
		t.Fatal(err)
	}
	if !confirmed {
		t.Fatal("successful authenticated Weixin poll was not reported")
	}
}
