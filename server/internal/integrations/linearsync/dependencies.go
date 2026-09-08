// Package linearsync owns durable synchronization between local issues and
// Linear. HTTP routing and public response formats stay in the caller.
package linearsync

import (
	"context"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

// DBExecutor is the SQL boundary used by the queue and provider sync paths.
type DBExecutor interface {
	Exec(context.Context, string, ...any) (pgconn.CommandTag, error)
	Query(context.Context, string, ...any) (pgx.Rows, error)
	QueryRow(context.Context, string, ...any) pgx.Row
}

type TxStarter interface {
	Begin(context.Context) (pgx.Tx, error)
}

// EventSink receives committed domain changes. Its adapter chooses the
// transport and response DTO; sync never imports the HTTP layer.
type EventSink interface {
	IssueChanged(issue db.Issue, eventType, actorType, actorID string, projectChanged bool)
	CommentChanged(comment db.Comment, issueRevision int64, created bool)
}

func parseUUID(value string) pgtype.UUID    { return util.MustParseUUID(value) }
func uuidToString(value pgtype.UUID) string { return util.UUIDToString(value) }
