package handler

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestInstalledIsTheCanonicalInstallationStatus(t *testing.T) {
	now := time.Now()
	row := db.ListChannelConnectionStatesRow{
		Status: "installed", State: pgtype.Text{String: "healthy", Valid: true},
		ObserverToken: pgtype.Text{String: "current", Valid: true},
		WsLeaseToken: pgtype.Text{String: "current", Valid: true},
		WsLeaseExpiresAt: pgtype.Timestamptz{Time: now.Add(time.Minute), Valid: true},
		ObservedAt: pgtype.Timestamptz{Time: now.Add(-time.Second), Valid: true},
	}
	if got := projectConnectionStatus(row, now); got.State != "healthy" {
		t.Fatalf("installed connection was not recognized: %+v", got)
	}
}

func TestInstalledStatusDatabaseDefaultDB(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("integration test requires DATABASE_URL")
	}
	ctx := context.Background()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	var expression string
	err = pool.QueryRow(ctx, `SELECT column_default FROM information_schema.columns
		WHERE table_schema = current_schema() AND table_name = 'channel_installation' AND column_name = 'status'`).Scan(&expression)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(expression, "'installed'") {
		t.Fatalf("installation status default still uses ambiguous semantics: %s", expression)
	}
}
