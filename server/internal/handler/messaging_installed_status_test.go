package handler

import (
	"context"
	"os"
	"path/filepath"
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
	var indexCount int
	err = pool.QueryRow(ctx, `SELECT count(*) FROM pg_indexes
		WHERE schemaname = current_schema() AND tablename = 'channel_installation'
		AND indexname = 'idx_channel_installation_installed_lease'
		AND indexdef LIKE '%installed%'`).Scan(&indexCount)
	if err != nil || indexCount != 1 {
		t.Fatalf("installed lease predicate was not migrated: %d %v", indexCount, err)
	}
}

func TestInstalledStatusMigrationPreservesRecordsAndRollsBackDB(t *testing.T) {
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
	tx, err := pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	// The temporary relation shadows only this connection's production table.
	// No global schema or other package's concurrently running fixture changes.
	_, err = tx.Exec(ctx, `CREATE TEMP TABLE channel_installation (
		id int PRIMARY KEY,
		status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
		config jsonb NOT NULL DEFAULT '{"fixture":"unchanged"}',
		installed_at timestamptz NOT NULL DEFAULT '2026-01-01T00:00:00Z',
		hosted_paused_at timestamptz DEFAULT '2026-02-01T00:00:00Z'
	) ON COMMIT DROP;
	INSERT INTO channel_installation (id) VALUES (1);
	INSERT INTO channel_installation (id, status) VALUES (2, 'revoked');`)
	if err != nil {
		t.Fatal(err)
	}
	for _, direction := range []string{"up", "down", "up"} {
		migration, err := os.ReadFile(filepath.Join("..", "..", "migrations", "578_channel_installation_installed_status."+direction+".sql"))
		if err != nil {
			t.Fatal(err)
		}
		if _, err := tx.Exec(ctx, string(migration)); err != nil {
			t.Fatalf("%s migration failed: %v", direction, err)
		}
		want := "installed"
		if direction == "down" {
			want = "active"
		}
		var state string
		var unchanged bool
		err = tx.QueryRow(ctx, `SELECT status,
			config = '{"fixture":"unchanged"}'::jsonb
			AND installed_at = '2026-01-01T00:00:00Z'::timestamptz
			AND hosted_paused_at = '2026-02-01T00:00:00Z'::timestamptz
			FROM channel_installation WHERE id = 1`).Scan(&state, &unchanged)
		if err != nil || state != want || !unchanged {
			t.Fatalf("%s changed installation identity/config/times: %q %t %v", direction, state, unchanged, err)
		}
		if err := tx.QueryRow(ctx, "SELECT status FROM channel_installation WHERE id = 2").Scan(&state); err != nil || state != "revoked" {
			t.Fatalf("%s altered revoked installation: %q %v", direction, state, err)
		}
		if _, err := tx.Exec(ctx, "INSERT INTO channel_installation (id) VALUES (3)"); err != nil {
			t.Fatal(err)
		}
		if err := tx.QueryRow(ctx, "SELECT status FROM channel_installation WHERE id = 3").Scan(&state); err != nil || state != want {
			t.Fatalf("%s default = %q: %v", direction, state, err)
		}
		if _, err := tx.Exec(ctx, "DELETE FROM channel_installation WHERE id = 3"); err != nil {
			t.Fatal(err)
		}
	}
}
