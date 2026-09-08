package linearsync

import (
	"context"
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

var (
	testPool        *pgxpool.Pool
	testUserID      string
	testWorkspaceID string
	dbfx            *testutil.Fixture
)

func TestMain(m *testing.M) { os.Exit(runLinearTests(m)) }

func runLinearTests(m *testing.M) (code int) {
	dsn, configured := os.LookupEnv("DATABASE_URL")
	if dsn == "" {
		dsn = "postgres://patchbay:patchbay@localhost:5432/patchbay?sslmode=disable"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	admin, err := pgxpool.New(ctx, dsn)
	if err == nil {
		err = admin.Ping(ctx)
	}
	if err != nil {
		if admin != nil {
			admin.Close()
		}
		if configured {
			fmt.Fprintf(os.Stderr, "Linear test database unavailable: %v\n", err)
			return 1
		}
		// Preserve the offline unit-test path, while configured CI/local DB
		// failures are fatal and cannot masquerade as a successful DB run.
		return m.Run()
	}
	defer admin.Close()
	schema := "linearsync_test_" + strings.ReplaceAll(uuid.NewString(), "-", "")
	quotedSchema := pgx.Identifier{schema}.Sanitize()
	if _, err = admin.Exec(ctx, "CREATE SCHEMA "+quotedSchema); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cleanupCancel()
		if _, err := admin.Exec(cleanupCtx, "DROP SCHEMA "+quotedSchema+" CASCADE"); err != nil {
			fmt.Fprintf(os.Stderr, "Linear fixture schema cleanup: %v\n", err)
			code = 1
		}
	}()
	// Only integration-owned tables need isolation: global queue claiming and
	// polling must not see another package's fixtures. The real public issue,
	// comment and work-product tables keep their production triggers, whose
	// unqualified Linear table references follow this pool's search_path.
	for _, table := range []string{"linear_connection", "linear_oauth_state", "linear_project_binding", "linear_issue_link", "linear_sync_inbox", "linear_sync_outbox", "linear_member_binding", "linear_sync_conflict", "linear_comment_link"} {
		statement := "CREATE TABLE " + pgx.Identifier{schema, table}.Sanitize() + " (LIKE " + pgx.Identifier{"public", table}.Sanitize() + " INCLUDING ALL)"
		if _, err = admin.Exec(ctx, statement); err != nil {
			fmt.Fprintf(os.Stderr, "Linear fixture table %s: %v\n", table, err)
			return 1
		}
	}
	config, err := pgxpool.ParseConfig(dsn)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	config.ConnConfig.RuntimeParams["search_path"] = quotedSchema + ", public"
	testPool, err = pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer testPool.Close()
	suffix := uuid.NewString()
	if err = testPool.QueryRow(ctx, `INSERT INTO "user"(name,email) VALUES('Linear sync fixture',$1) RETURNING id`, suffix+"@linearsync.example.test").Scan(&testUserID); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer func() {
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cleanupCancel()
		if testWorkspaceID != "" {
			for _, table := range []string{"activity_log", "comment", "work_product_relation", "work_product", "issue", "project", "issue_status", "member"} {
				if _, err := testPool.Exec(cleanupCtx, "DELETE FROM "+pgx.Identifier{table}.Sanitize()+" WHERE workspace_id=$1", testWorkspaceID); err != nil {
					fmt.Fprintf(os.Stderr, "Linear fixture cleanup %s: %v\n", table, err)
					code = 1
				}
			}
			if _, err := testPool.Exec(cleanupCtx, `DELETE FROM workspace WHERE id=$1`, testWorkspaceID); err != nil {
				fmt.Fprintln(os.Stderr, err)
				code = 1
			}
		}
		if _, err := testPool.Exec(cleanupCtx, `DELETE FROM "user" WHERE id=$1`, testUserID); err != nil {
			fmt.Fprintln(os.Stderr, err)
			code = 1
		}
	}()
	if err = testPool.QueryRow(ctx, `INSERT INTO workspace(name,slug,issue_prefix) VALUES('Linear sync fixture',$1,'LIN') RETURNING id`, "linearsync-"+suffix).Scan(&testWorkspaceID); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	if _, err = testPool.Exec(ctx, `INSERT INTO member(workspace_id,user_id,role) VALUES($1,$2,'owner')`, testWorkspaceID, testUserID); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	dbfx = testutil.New(testPool, testWorkspaceID, testUserID)
	return m.Run()
}

func TestLinearFixtureRoutesRealTriggersToPrivateQueues(t *testing.T) {
	f := setupWorker(t, "publish", &fakeLinearAPI{})
	issueID := dbfx.Issue(t, "Real trigger routing", testutil.Cols{"project_id": f.projectID})
	if _, err := db.New(testPool).CreateComment(context.Background(), db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Trigger isolation", Type: "comment"}); err != nil {
		t.Fatal(err)
	}
	var own, public int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND event_type IN ('issue_created','comment_created')`, issueID).Scan(&own); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM public.linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&public); err != nil {
		t.Fatal(err)
	}
	if own != 2 || public != 0 {
		t.Fatalf("real trigger queues: private=%d public=%d, want 2/0", own, public)
	}
}
