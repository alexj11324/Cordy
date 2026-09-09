package migrations

import (
	"context"
	"errors"
	"fmt"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

func TestIssueActiveExecutorDatabaseGuard(t *testing.T) {
	dsn := os.Getenv("DATABASE_URL")
	if dsn == "" {
		t.Skip("requires PostgreSQL at DATABASE_URL")
	}
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close(ctx)
	schema := pgx.Identifier{fmt.Sprintf("executor_guard_%d", time.Now().UnixNano())}.Sanitize()
	if _, err = conn.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	defer conn.Exec(ctx, "DROP SCHEMA "+schema+" CASCADE")
	if _, err = conn.Exec(ctx, "SET search_path TO "+schema); err != nil {
		t.Fatal(err)
	}
	exec := func(sql string, args ...any) {
		t.Helper()
		if _, e := conn.Exec(ctx, sql, args...); e != nil {
			t.Fatalf("%s: %v", sql, e)
		}
	}
	reject := func(sql string, args ...any) {
		t.Helper()
		_, e := conn.Exec(ctx, sql, args...)
		var pe *pgconn.PgError
		if !errors.As(e, &pe) || pe.Code != "23514" {
			t.Fatalf("expected check violation for %s, got %v", sql, e)
		}
	}
	exec(`CREATE TABLE issue (id int PRIMARY KEY, workspace_id uuid NOT NULL, status text NOT NULL, executor_type text, executor_id uuid, title text NOT NULL DEFAULT 'example');
 CREATE TABLE issue_status (workspace_id uuid NOT NULL, key text NOT NULL, category text NOT NULL, name text NOT NULL DEFAULT 'custom', UNIQUE(workspace_id,key));`)
	const ws = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
	const otherWS = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
	const executor = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
	// A migration must report invalid legacy rows, never silently reassign them.
	exec(`INSERT INTO issue(id,workspace_id,status) VALUES(1,$1,'in_progress')`, ws)
	migration := readMigrationFile(t, "585_issue_active_executor_guard.up.sql")
	reject(migration)
	exec(`UPDATE issue SET status='todo' WHERE id=1`)
	applyMigrationFile(t, ctx, conn, "585_issue_active_executor_guard.up.sql")
	for _, status := range []string{"in_progress", "in_review", "blocked"} {
		t.Run(status, func(t *testing.T) {
			reject(`INSERT INTO issue(id,workspace_id,status) VALUES(2,$1,$2)`, ws, status)
			reject(`INSERT INTO issue(id,workspace_id,status,executor_id) VALUES(2,$1,$2,$3)`, ws, status, executor)
			reject(`INSERT INTO issue(id,workspace_id,status,executor_type) VALUES(2,$1,$2,'agent')`, ws, status)
			reject(`INSERT INTO issue(id,workspace_id,status,executor_type,executor_id) VALUES(2,$1,$2,'member',$3)`, ws, status, executor)
			reject(`UPDATE issue SET status=$1 WHERE id=1`, status)
			exec(`INSERT INTO issue(id,workspace_id,status,executor_type,executor_id) VALUES(2,$1,$2,'agent',$3)`, ws, status, executor)
			reject(`UPDATE issue SET executor_type=NULL,executor_id=NULL WHERE id=2`)
			// A single valid write can move the issue back and remove its executor.
			exec(`UPDATE issue SET status='todo',executor_type=NULL,executor_id=NULL WHERE id=2`)
			exec(`DELETE FROM issue WHERE id=2`)
		})
	}
	for _, status := range []string{"backlog", "todo", "done", "cancelled"} {
		exec(`INSERT INTO issue(id,workspace_id,status) VALUES(2,$1,$2)`, ws, status)
		exec(`DELETE FROM issue WHERE id=2`)
	}
	exec(`INSERT INTO issue_status(workspace_id,key,category) VALUES($1,'custom_work','in_progress'),($1,'custom_review','in_review'),($1,'custom_blocked','blocked'),($1,'custom_todo','todo'),($2,'custom_work','todo')`, ws, otherWS)
	for _, status := range []string{"custom_work", "custom_review", "custom_blocked"} {
		reject(`INSERT INTO issue(id,workspace_id,status) VALUES(2,$1,$2)`, ws, status)
		exec(`INSERT INTO issue(id,workspace_id,status,executor_type,executor_id) VALUES(2,$1,$2,'team',$3)`, ws, status, executor)
		reject(`UPDATE issue SET executor_id=NULL WHERE id=2`)
		exec(`DELETE FROM issue WHERE id=2`)
	}
	exec(`INSERT INTO issue(id,workspace_id,status) VALUES(2,$1,'custom_todo'),(3,$2,'custom_work')`, ws, otherWS)
	reject(`UPDATE issue SET workspace_id=$1 WHERE id=3`, ws)
	reject(`INSERT INTO issue(id,workspace_id,status) VALUES(4,$1,'unknown_status')`, ws)
	reject(`UPDATE issue_status SET category='in_progress' WHERE workspace_id=$1 AND key='custom_todo'`, ws)
	reject(`UPDATE issue_status SET key='renamed' WHERE workspace_id=$1 AND key='custom_work'`, ws)
	exec(`UPDATE issue_status SET name='Renamed label' WHERE workspace_id=$1 AND key='custom_work'`, ws)
	// Before/after migration and rollback keep user rows intact.
	var count int
	if err = conn.QueryRow(ctx, `SELECT count(*) FROM issue`).Scan(&count); err != nil || count != 3 {
		t.Fatalf("rows=%d err=%v", count, err)
	}
	applyMigrationFile(t, ctx, conn, "585_issue_active_executor_guard.down.sql")
	exec(`INSERT INTO issue(id,workspace_id,status) VALUES(4,$1,'in_progress')`, ws)
	exec(`DELETE FROM issue WHERE id=4`)
	applyMigrationFile(t, ctx, conn, "585_issue_active_executor_guard.up.sql")
	reject(`UPDATE issue SET status='in_progress' WHERE id=1`)
}
