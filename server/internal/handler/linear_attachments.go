package handler

import (
	"context"
	"encoding/json"
	"errors"

	"github.com/jackc/pgx/v5"
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

func (w *LinearWorker) deleteLinearWorkProductAttachment(ctx context.Context, b workerBinding, issueID pgtype.UUID, token string, payload []byte) error {
	api, ok := w.api.(interface {
		DeleteAttachmentByURL(context.Context, string, string, string) error
	})
	if !ok {
		return errors.New("Linear attachment API is unavailable")
	}
	var event struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal(payload, &event); err != nil {
		return err
	}
	if event.URL == "" {
		return errors.New("Linear attachment deletion omitted URL")
	}
	var remoteID string
	err := w.db.QueryRow(ctx, `SELECT linear_issue_id FROM linear_issue_link WHERE workspace_id=$1 AND binding_id=$2 AND patchbay_issue_id=$3 AND sync_status<>'deleted'`, b.WorkspaceID, b.ID, issueID).Scan(&remoteID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return err
	}
	return api.DeleteAttachmentByURL(ctx, token, remoteID, event.URL)
}
