package handler

import (
	"context"
	"errors"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func (w *LinearWorker) publishLinearWorkProducts(ctx context.Context, b workerBinding, issueID pgtype.UUID, remoteID, token string) error {
	q := db.New(w.db)
	for offset := int32(0); offset < 10000; offset += 100 {
		products, err := q.ListWorkProductsByIssue(ctx, db.ListWorkProductsByIssueParams{WorkspaceID: b.WorkspaceID, IssueID: issueID, Limit: 100, Offset: offset})
		if err != nil {
			return err
		}
		for _, product := range products {
			if product.Kind != "pull_request" || !product.ExternalUrl.Valid || product.ExternalUrl.String == "" {
				continue
			}
			api, ok := w.api.(interface {
				UpsertAttachment(context.Context, string, string, string, string) error
			})
			if !ok {
				return errors.New("Linear attachment API is unavailable")
			}
			if err = api.UpsertAttachment(ctx, token, remoteID, product.ExternalIdentity, product.ExternalUrl.String); err != nil {
				return err
			}
		}
		if len(products) < 100 {
			return nil
		}
	}
	return errors.New("Linear work product pagination limit reached")
}
