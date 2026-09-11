package messagingbootstrap

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"net/url"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	qrcode "github.com/skip2/go-qrcode"

	"github.com/orvilo-ai/orvilo/server/internal/integrations/weixin"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	"github.com/orvilo-ai/orvilo/server/internal/util/secretbox"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type WeixinAuthorizationScope struct {
	WorkspaceID     string
	InstallerUserID string
	AgentID         string
}

// AuthorizeWeixin runs the server operator's QR session and leaves provider
// credentials inside the existing installation service and its encrypted row.
func AuthorizeWeixin(ctx context.Context, pool *pgxpool.Pool, mode string, values WeixinAuthorizationScope, input io.Reader, output io.Writer) (pgtype.UUID, error) {
	if mode != serverConfigured {
		return pgtype.UUID{}, errors.New("Weixin operator authorization requires a server_configured deployment")
	}
	installScope, err := scopeFromValues(values.WorkspaceID, values.InstallerUserID, values.AgentID)
	if err != nil {
		return pgtype.UUID{}, err
	}
	if pool == nil {
		return pgtype.UUID{}, errors.New("Weixin operator authorization requires the server database")
	}
	key, err := secretbox.LoadKey("ORVILO_WEIXIN_SECRET_KEY")
	if err != nil {
		return pgtype.UUID{}, err
	}
	box, err := secretbox.New(key)
	if err != nil {
		return pgtype.UUID{}, err
	}
	queries := db.New(pool)
	if err := validateWeixinOperatorScope(ctx, queries, installScope); err != nil {
		return pgtype.UUID{}, err
	}
	sessions := weixin.NewMemorySessionStore()
	service, err := weixin.NewInstallationService(queries, pool, box, sessions, nil)
	if err != nil {
		return pgtype.UUID{}, err
	}
	begin, err := service.Begin(ctx, weixin.BeginParams{
		WorkspaceID: installScope.workspaceID, AgentID: installScope.agentID, InitiatorID: installScope.installerUserID,
	})
	if err != nil {
		return pgtype.UUID{}, weixinOperatorError("request Weixin QR code", err)
	}
	defer func() { _ = sessions.Delete(context.Background(), begin.SessionID) }()
	ctx, cancel := context.WithDeadline(ctx, begin.ExpiresAt)
	defer cancel()
	// qrcode is an opaque polling token; only qrcode_img_content is scannable.
	scanURL := strings.TrimSpace(begin.QRCodeImageData)
	parsed, err := url.Parse(scanURL)
	if err != nil || parsed.Scheme != "https" || parsed.User != nil ||
		(parsed.Hostname() != "weixin.qq.com" && !strings.HasSuffix(parsed.Hostname(), ".weixin.qq.com")) {
		return pgtype.UUID{}, errors.New("Weixin returned an invalid QR scan URL")
	}
	qr, err := qrcode.New(scanURL, qrcode.Medium)
	if err != nil {
		return pgtype.UUID{}, errors.New("could not render the Weixin QR code")
	}
	if _, err := fmt.Fprintf(output, "Scan with Weixin and confirm on your phone. Press Ctrl-C to cancel.\n%s\n%s\n", qr.ToSmallString(false), scanURL); err != nil {
		return pgtype.UUID{}, err
	}
	scanner := bufio.NewScanner(input)
	scanner.Buffer(make([]byte, 64), 256)
	verifyCode, previousStatus := "", ""
	for {
		if err := ctx.Err(); err != nil {
			return pgtype.UUID{}, err
		}
		status, err := service.Status(ctx, begin.SessionID, installScope.workspaceID, installScope.installerUserID, verifyCode)
		verifyCode = ""
		if err != nil {
			return pgtype.UUID{}, weixinOperatorError("complete Weixin authorization", err)
		}
		switch status.Status {
		case weixin.InstallStatusSuccess:
			if _, err := fmt.Fprintf(output, "Installation ID: %s\n", util.UUIDToString(status.InstallationID)); err != nil {
				return status.InstallationID, err
			}
			return status.InstallationID, nil
		case weixin.InstallStatusExpired:
			return pgtype.UUID{}, errors.New("Weixin QR authorization expired; run weixin-auth again for a new code")
		case weixin.InstallStatusAlreadyConnected:
			return pgtype.UUID{}, errors.New("this Weixin account is already connected; check the existing installation")
		case weixin.InstallStatusNeedVerifyCode:
			verifyCode, err = readWeixinVerificationCode(ctx, scanner, output)
			if err != nil {
				return pgtype.UUID{}, err
			}
			continue
		case weixin.InstallStatusScanned:
			if previousStatus != status.Status {
				if _, err := fmt.Fprintln(output, "Scanned. Confirm the authorization on your phone."); err != nil {
					return pgtype.UUID{}, err
				}
			}
		}
		previousStatus = status.Status
		timer := time.NewTimer(time.Duration(begin.PollIntervalSeconds) * time.Second)
		select {
		case <-ctx.Done():
			timer.Stop()
			return pgtype.UUID{}, ctx.Err()
		case <-timer.C:
		}
	}
}

func validateWeixinOperatorScope(ctx context.Context, queries *db.Queries, s scope) error {
	member, err := queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID: s.installerUserID, WorkspaceID: s.workspaceID,
	})
	if err != nil || !member.ID.Valid {
		return errors.New("installer must be a member of the requested workspace")
	}
	admin := member.Role == "owner" || member.Role == "admin"
	if !s.agentID.Valid {
		if !admin {
			return errors.New("workspace installation requires an owner or admin installer")
		}
		return nil
	}
	agent, err := queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: s.agentID, WorkspaceID: s.workspaceID})
	if err != nil || !agent.ID.Valid || agent.ArchivedAt.Valid {
		return errors.New("agent must be active in the requested workspace")
	}
	if !admin && agent.OwnerID != s.installerUserID {
		return errors.New("agent installation requires its owner or a workspace admin")
	}
	return nil
}

func readWeixinVerificationCode(ctx context.Context, scanner *bufio.Scanner, output io.Writer) (string, error) {
	for {
		if _, err := fmt.Fprint(output, "Verification code shown on your phone: "); err != nil {
			return "", err
		}
		read := make(chan bool, 1)
		go func() { read <- scanner.Scan() }()
		select {
		case <-ctx.Done():
			return "", ctx.Err()
		case ok := <-read:
			if !ok {
				if scanner.Err() != nil {
					return "", errors.New("could not read Weixin verification code")
				}
				return "", errors.New("Weixin authorization cancelled: verification input closed")
			}
		}
		value := strings.TrimSpace(scanner.Text())
		if value != "" && strings.IndexFunc(value, func(r rune) bool { return r < '0' || r > '9' }) == -1 {
			return value, nil
		}
		if _, err := fmt.Fprintln(output, "Enter the digits from your phone."); err != nil {
			return "", err
		}
	}
}

func weixinOperatorError(action string, err error) error {
	for _, known := range []error{
		context.Canceled, context.DeadlineExceeded,
		weixin.ErrInstallSessionNotFound, weixin.ErrInstallSessionExpired, weixin.ErrInstallSessionForbidden,
		weixin.ErrInstallAuthorizationChanged, weixin.ErrUnsafeProviderURL, weixin.ErrConfirmationIncomplete,
		weixin.ErrBotOwnedByAnotherWorkspace, weixin.ErrBotOwnedBySameWorkspace, weixin.ErrBotOwnedByArchivedAgent,
		weixin.ErrBindingAlreadyAssigned,
	} {
		if errors.Is(err, known) {
			return fmt.Errorf("%s: %w", action, known)
		}
	}
	// Provider messages and database detail can contain returned credentials.
	var providerError *weixin.APIError
	if errors.As(err, &providerError) {
		return fmt.Errorf("%s: provider returned code %d", action, providerError.Ret)
	}
	var databaseError *pgconn.PgError
	if errors.As(err, &databaseError) {
		return fmt.Errorf("%s: database returned SQLSTATE %s", action, databaseError.Code)
	}
	return fmt.Errorf("%s failed; check provider connectivity and server database access", action)
}
