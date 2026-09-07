// Command dev-seed installs the repository's persistent local development
// fixtures. It is intentionally separate from migrations and server startup so
// production and ordinary developer databases never receive sample content.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/patchbay-ai/patchbay/server/internal/devseed"
)

func main() {
	if err := run(context.Background()); err != nil {
		log.Fatal(err)
	}
}

func run(ctx context.Context) error {
	databaseURL := os.Getenv("DATABASE_URL")
	enabled := os.Getenv("ORVILO_ENABLE_DEV_SEED") == "1"
	if err := devseed.ValidateTarget(databaseURL, enabled); err != nil {
		return err
	}

	pool, err := pgxpool.New(ctx, databaseURL)
	if err != nil {
		return fmt.Errorf("connect to development database: %w", err)
	}
	defer pool.Close()
	if err := pool.Ping(ctx); err != nil {
		return fmt.Errorf("reach development database: %w", err)
	}

	email := strings.TrimSpace(os.Getenv("ORVILO_DEV_EMAIL"))
	result, err := devseed.Seed(ctx, pool, email)
	if err != nil {
		return err
	}
	fmt.Printf("Seeded %s (%s): %d issues, %d graph nodes, %d graph edges.\n",
		result.Workspace, result.WorkspaceID, result.Issues, result.GraphNodes, result.GraphEdges)
	// The fixtures live in their own workspace, not the one a fresh sign-in
	// lands on, so a seed that does not say where to look reads as a seed that
	// did nothing. Name the workspace first: the documented setup is
	// `make up C=desktop`, which starts no Web listener, so a Web URL is the
	// conditional extra rather than the answer.
	fmt.Printf("They are in the %q workspace — switch to it in the app.\n", devseed.WorkspaceSlug)
	if origin := strings.TrimRight(strings.TrimSpace(os.Getenv("FRONTEND_ORIGIN")), "/"); origin != "" {
		fmt.Printf("With Web running (`make up C=api,web`), %s/%s/issues opens it directly.\n",
			origin, devseed.WorkspaceSlug)
	}
	return nil
}
