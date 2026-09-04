package migrations

import (
	"context"
	"os"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"
)

func TestRustToGoSchemaReconciliationRoundTrip(t *testing.T) {
	dbURL := os.Getenv("DATABASE_URL")
	if dbURL == "" {
		t.Skip("integration test requires Postgres at DATABASE_URL")
	}

	ctx := context.Background()
	pool, err := pgxpool.New(ctx, dbURL)
	if err != nil {
		t.Fatalf("connect to Postgres: %v", err)
	}
	defer pool.Close()

	conn, err := pool.Acquire(ctx)
	if err != nil {
		t.Fatalf("acquire Postgres connection: %v", err)
	}
	defer conn.Release()

	const schema = "rust_to_go_reconciliation_migration_test"
	cleanup := func() {
		_, _ = pool.Exec(context.Background(), "DROP SCHEMA IF EXISTS "+schema+" CASCADE")
	}
	cleanup()
	t.Cleanup(cleanup)
	if _, err := conn.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatalf("create isolated migration schema: %v", err)
	}
	if _, err := conn.Exec(ctx, `SELECT set_config('search_path', $1, false)`, schema); err != nil {
		t.Fatalf("set isolated migration search path: %v", err)
	}

	if _, err := conn.Exec(ctx, `
		CREATE TABLE schema_migrations (
			version TEXT PRIMARY KEY,
			applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
		);
		INSERT INTO schema_migrations (version) VALUES
			('454_issue_roles'),
			('465_linear_installation_foundation');

		CREATE TABLE issue (reviewer_type TEXT, reviewer_id UUID);
		CREATE TABLE linear_connection (
			id UUID NOT NULL,
			access_token_encrypted TEXT NOT NULL,
			refresh_token_encrypted TEXT NOT NULL
		);
		CREATE TABLE linear_oauth_state (
			id UUID NOT NULL,
			code_verifier_encrypted TEXT NOT NULL
		);
		CREATE TABLE linear_project_binding (id UUID NOT NULL);
		CREATE TABLE linear_issue_link (id UUID NOT NULL);
		CREATE TABLE linear_sync_inbox (id UUID NOT NULL);
		CREATE TABLE linear_sync_outbox (id UUID NOT NULL);
		CREATE TABLE linear_member_binding (id UUID NOT NULL);
		CREATE TABLE linear_sync_conflict (id UUID NOT NULL);

		INSERT INTO linear_connection (
			id, access_token_encrypted, refresh_token_encrypted
		) VALUES (
			'00000000-0000-0000-0000-000000000001', 'AAEC', 'AwQF'
		);
		INSERT INTO linear_oauth_state (id, code_verifier_encrypted)
		VALUES ('00000000-0000-0000-0000-000000000002', 'BgcI');
	`); err != nil {
		t.Fatalf("create Rust schema fixture: %v", err)
	}

	applyMigrationFile(t, ctx, conn.Conn(), "584_rust_to_go_schema_reconciliation.up.sql")

	var access, refresh, verifier []byte
	if err := conn.QueryRow(ctx, `
		SELECT access_token_encrypted, refresh_token_encrypted
		FROM linear_connection
	`).Scan(&access, &refresh); err != nil {
		t.Fatalf("read converted Linear connection secrets: %v", err)
	}
	if err := conn.QueryRow(ctx, `
		SELECT code_verifier_encrypted FROM linear_oauth_state
	`).Scan(&verifier); err != nil {
		t.Fatalf("read converted Linear OAuth secret: %v", err)
	}
	if string(access) != string([]byte{0, 1, 2}) ||
		string(refresh) != string([]byte{3, 4, 5}) ||
		string(verifier) != string([]byte{6, 7, 8}) {
		t.Fatalf("converted secrets = %v %v %v", access, refresh, verifier)
	}

	assertInsertCheckViolation(t, ctx, conn.Conn(), `
		INSERT INTO issue (reviewer_type, reviewer_id) VALUES ('agent', NULL)
	`)

	var defaultValue *string
	if err := conn.QueryRow(ctx, `
		SELECT column_default
		FROM information_schema.columns
		WHERE table_schema = $1
		  AND table_name = 'linear_connection'
		  AND column_name = 'id'
	`, schema).Scan(&defaultValue); err != nil {
		t.Fatalf("inspect Go id default: %v", err)
	}
	if defaultValue == nil || *defaultValue != "gen_random_uuid()" {
		t.Fatalf("Go id default = %v, want gen_random_uuid()", defaultValue)
	}

	applyMigrationFile(t, ctx, conn.Conn(), "584_rust_to_go_schema_reconciliation.down.sql")
	// The migration intentionally changes the selected columns' PostgreSQL
	// result type. A real rollback restarts the process; clear this test
	// connection's prepared statement cache to model that boundary.
	if err := conn.Conn().DeallocateAll(ctx); err != nil {
		t.Fatalf("clear prepared statements after rollback: %v", err)
	}

	var accessText, refreshText, verifierText string
	if err := conn.QueryRow(ctx, `
		SELECT access_token_encrypted, refresh_token_encrypted
		FROM linear_connection
	`).Scan(&accessText, &refreshText); err != nil {
		t.Fatalf("read restored Linear connection secrets: %v", err)
	}
	if err := conn.QueryRow(ctx, `
		SELECT code_verifier_encrypted FROM linear_oauth_state
	`).Scan(&verifierText); err != nil {
		t.Fatalf("read restored Linear OAuth secret: %v", err)
	}
	if accessText != "AAEC" || refreshText != "AwQF" || verifierText != "BgcI" {
		t.Fatalf(
			"restored secrets = %q %q %q, want original base64",
			accessText,
			refreshText,
			verifierText,
		)
	}

	if _, err := conn.Exec(ctx, `
		INSERT INTO issue (reviewer_type, reviewer_id) VALUES ('agent', NULL)
	`); err != nil {
		t.Fatalf("Rust rollback left Go reviewer constraint: %v", err)
	}
	if err := conn.QueryRow(ctx, `
		SELECT column_default
		FROM information_schema.columns
		WHERE table_schema = $1
		  AND table_name = 'linear_connection'
		  AND column_name = 'id'
	`, schema).Scan(&defaultValue); err != nil {
		t.Fatalf("inspect restored Rust id default: %v", err)
	}
	if defaultValue != nil {
		t.Fatalf("restored Rust id default = %q, want NULL", *defaultValue)
	}
}
