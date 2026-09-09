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
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/devseed"
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
	// Wake real local daemons before creating runtime-bound starter Agents.
	origin := "http://127.0.0.1:" + strings.TrimSpace(os.Getenv("PORT"))
	if os.Getenv("PORT") == "" {
		origin = "http://127.0.0.1:8080"
	}
	expectedRuntimes, err := devseed.OnlineSeedRuntimes(ctx, pool, email, "")
	if err != nil {
		return err
	}
	if len(expectedRuntimes) == 0 {
		return fmt.Errorf("fixtures saved; start Desktop so its installed Harnesses register, then rerun make seed-dev")
	}
	if err := devseed.WakeRuntimeRegistration(ctx, pool, origin, email); err != nil {
		return fmt.Errorf("fixtures saved, but runtime registration failed: %w; start the local API and Desktop, then rerun make seed-dev", err)
	}
	waitCtx, cancel := context.WithTimeout(ctx, 45*time.Second)
	defer cancel()
	ticker := time.NewTicker(250 * time.Millisecond)
	defer ticker.Stop()
	for {
		registered, err := devseed.OnlineSeedRuntimes(waitCtx, pool, email, result.WorkspaceID)
		if err != nil {
			return err
		}
		if devseed.RuntimeRegistrationComplete(expectedRuntimes, registered) {
			break
		}
		select {
		case <-waitCtx.Done():
			return fmt.Errorf("fixtures saved; not all installed Harnesses registered; check Desktop and rerun make seed-dev")
		case <-ticker.C:
		}
	}
	agents, err := devseed.SeedRuntimeAgents(ctx, pool, email)
	if err != nil {
		return err
	}
	if err := devseed.SeedTeam(ctx, pool, email); err != nil {
		return err
	}
	fmt.Printf("Added %d starter Agents using real local Harness defaults.\n", agents)
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
