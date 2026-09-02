package handler

import (
	"context"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// The per-provider "list an issue's PRs" queries are gone: an issue's PRs are
// now Work Products, and the mirror rows are fetched one at a time by the
// product's provider_record_id. Webhook tests still want to assert on the
// mirror itself (state, provider, check counts), so these helpers walk the
// same path the handler walks — the issue's Work Product list, then the mirror
// behind each PR-backed product — and hand back the mirror rows.
//
// Going through ListWorkProductsByIssue rather than querying the mirror tables
// directly is the point: a test that asserts "this PR is hidden from the list"
// is then asserting about the filter the product actually ships with.

const workProductFixturePageSize = 100

// The workspace comes from the issue rather than from the suite fixture: the
// cross-workspace webhook tests deliberately put the same identifier in two
// workspaces, and pinning the fixture workspace would report "no products" for
// the second one — the exact collision those tests exist to catch.
func listIssueWorkProducts(ctx context.Context, issueID pgtype.UUID) ([]db.ListWorkProductsByIssueRow, error) {
	var workspaceID pgtype.UUID
	if err := testPool.QueryRow(ctx, `SELECT workspace_id FROM issue WHERE id = $1`, issueID).Scan(&workspaceID); err != nil {
		return nil, err
	}
	return testHandler.Queries.ListWorkProductsByIssue(ctx, db.ListWorkProductsByIssueParams{
		WorkspaceID: workspaceID,
		IssueID:     issueID,
		Limit:       workProductFixturePageSize,
		Offset:      0,
	})
}

func listIssueGitHubPullRequests(ctx context.Context, issueID pgtype.UUID) ([]db.GetGitHubPullRequestForWorkProductRow, error) {
	products, err := listIssueWorkProducts(ctx, issueID)
	if err != nil {
		return nil, err
	}
	rows := make([]db.GetGitHubPullRequestForWorkProductRow, 0, len(products))
	for _, product := range products {
		if product.ProviderRecordType.String != "github_pull_request" || !product.ProviderRecordID.Valid {
			continue
		}
		pr, err := testHandler.Queries.GetGitHubPullRequestForWorkProduct(ctx, product.ProviderRecordID)
		if err != nil {
			return nil, err
		}
		rows = append(rows, pr)
	}
	return rows, nil
}

func listIssueVCSPullRequests(ctx context.Context, issueID pgtype.UUID) ([]db.GetVCSPullRequestForWorkProductRow, error) {
	products, err := listIssueWorkProducts(ctx, issueID)
	if err != nil {
		return nil, err
	}
	rows := make([]db.GetVCSPullRequestForWorkProductRow, 0, len(products))
	for _, product := range products {
		if product.ProviderRecordType.String != "vcs_pull_request" || !product.ProviderRecordID.Valid {
			continue
		}
		pr, err := testHandler.Queries.GetVCSPullRequestForWorkProduct(ctx, product.ProviderRecordID)
		if err != nil {
			return nil, err
		}
		rows = append(rows, pr)
	}
	return rows, nil
}

// cleanupWorkProductForPullRequest removes the product a mirrored PR produced
// along with every relation hanging off it. Tests that seed a PR mirror
// directly need this because the product is created lazily by the link query,
// not by the mirror insert, so deleting the mirror row alone leaves the
// product behind to collide with the next test's identity.
func cleanupWorkProductForPullRequest(ctx context.Context, prID string) {
	testPool.Exec(ctx, `
		DELETE FROM work_product_relation
		WHERE work_product_id IN (
		    SELECT id FROM work_product WHERE provider_record_id = $1
		)`, prID)
	testPool.Exec(ctx, `DELETE FROM work_product WHERE provider_record_id = $1`, prID)
}
