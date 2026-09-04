package service

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
)

// This raw query mirrors ListAttachmentURLsByWorkspace in
// pkg/db/queries/attachment.sql. The generated package is owned by the main
// agent, so workspace teardown uses this small typed-independent helper until
// sqlc is regenerated. The caller must keep tx open through the database
// deletion and commit before passing the returned URLs to object storage.
const listWorkspaceAttachmentURLsSQL = `
SELECT a.url
FROM attachment AS a
WHERE a.workspace_id = $1
ORDER BY a.id
`

// ListWorkspaceAttachmentURLs returns every attachment URL owned by a
// workspace, including rows without an issue/comment/chat/source-context
// relation and rows whose transient task_id is the only useful binding.
// workspaceID must be the workspace row already locked by the caller's
// DeleteWorkspace transaction; this helper intentionally does not start a
// second transaction or perform any deletion.
func ListWorkspaceAttachmentURLs(ctx context.Context, tx pgx.Tx, workspaceID pgtype.UUID) ([]string, error) {
	rows, err := tx.Query(ctx, listWorkspaceAttachmentURLsSQL, workspaceID)
	if err != nil {
		return nil, fmt.Errorf("list workspace attachment URLs: %w", err)
	}
	defer rows.Close()

	var urls []string
	for rows.Next() {
		var url string
		if err := rows.Scan(&url); err != nil {
			return nil, fmt.Errorf("scan workspace attachment URL: %w", err)
		}
		urls = append(urls, url)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate workspace attachment URLs: %w", err)
	}
	return urls, nil
}
