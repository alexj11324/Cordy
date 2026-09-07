package migrations

import (
	"context"
	"os"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"
)

func TestOrviloMediaMigrationRoundTrip(t *testing.T) {
	url := os.Getenv("DATABASE_URL")
	if url == "" {
		t.Skip("requires DATABASE_URL")
	}
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, url)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close(ctx)
	tx, err := conn.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Exec(ctx, `CREATE TEMP TABLE issue (description text);
CREATE TEMP TABLE chat_message (content text);
INSERT INTO issue VALUES ('text <!-- patchbay:channel-media:11111111-1111-1111-1111-111111111111 -->'), (NULL);
INSERT INTO chat_message SELECT description FROM issue;`); err != nil {
		t.Fatal(err)
	}
	for _, direction := range []string{"up", "down"} {
		data, err := os.ReadFile("../../migrations/597_orvilo_identity." + direction + ".sql")
		if err != nil {
			t.Fatal(err)
		}
		// Exercise the actual persisted-media statements without unrelated schema changes.
		start := strings.Index(string(data), "UPDATE issue SET description")
		if start < 0 {
			t.Fatal("media migration statements missing")
		}
		mediaSQL := strings.SplitN(string(data[start:]), "ALTER TABLE", 2)[0]
		if _, err := tx.Exec(ctx, mediaSQL); err != nil {
			t.Fatal(err)
		}
		prefix := "orvilo"
		if direction == "down" {
			prefix = "patchbay"
		}
		var count int
		if err := tx.QueryRow(ctx, `SELECT count(*) FROM
(SELECT description AS body FROM issue UNION ALL SELECT content FROM chat_message) media
WHERE body = $1`, "text <!-- "+prefix+":channel-media:11111111-1111-1111-1111-111111111111 -->").Scan(&count); err != nil || count != 2 {
			t.Fatalf("%s media conversion: count=%d err=%v", direction, count, err)
		}
	}
}
