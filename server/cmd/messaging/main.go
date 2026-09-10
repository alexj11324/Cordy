package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/orvilo-ai/orvilo/server/internal/handler"
	"github.com/orvilo-ai/orvilo/server/internal/messagingbootstrap"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := run(ctx, os.Args[1:], os.Stdin, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, "orvilo-messaging:", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, args []string, input io.Reader, output io.Writer) error {
	if len(args) == 0 || args[0] == "--help" || args[0] == "-h" {
		_, err := fmt.Fprintln(output, "Usage: orvilo-messaging weixin-auth [--workspace-id ID] [--installer-user-id ID] [--agent-id ID]")
		return err
	}
	if args[0] != "weixin-auth" {
		return errors.New("unknown command; use orvilo-messaging --help")
	}
	flags := flag.NewFlagSet("weixin-auth", flag.ContinueOnError)
	flags.SetOutput(output)
	var scope messagingbootstrap.WeixinAuthorizationScope
	flags.StringVar(&scope.WorkspaceID, "workspace-id", os.Getenv("ORVILO_MESSAGING_WORKSPACE_ID"), "workspace ID (defaults to ORVILO_MESSAGING_WORKSPACE_ID)")
	flags.StringVar(&scope.InstallerUserID, "installer-user-id", os.Getenv("ORVILO_MESSAGING_INSTALLER_USER_ID"), "installer user ID (defaults to ORVILO_MESSAGING_INSTALLER_USER_ID)")
	flags.StringVar(&scope.AgentID, "agent-id", os.Getenv("ORVILO_MESSAGING_AGENT_ID"), "optional agent ID (defaults to ORVILO_MESSAGING_AGENT_ID)")
	flags.Usage = func() {
		fmt.Fprintln(output, "Usage: orvilo-messaging weixin-auth [options]")
		fmt.Fprintln(output, "Run in the backend environment with DATABASE_URL and ORVILO_WEIXIN_SECRET_KEY.")
		fmt.Fprintln(output, "The server_configured deployment and installer permissions are checked before QR authorization.")
		fmt.Fprintln(output, "Scan the terminal QR, confirm on your phone, and enter a verification code when prompted.")
		fmt.Fprintln(output, "Only the encrypted installation is saved. Ctrl-C cancels; rerun after an expired QR.")
		flags.PrintDefaults()
	}
	if err := flags.Parse(args[1:]); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	if flags.NArg() != 0 {
		return errors.New("weixin-auth does not accept positional arguments")
	}
	mode := handler.ResolvedMessagingModeFromEnv()
	if mode != "server_configured" {
		return errors.New("Weixin operator authorization requires a server_configured deployment")
	}
	databaseURL := os.Getenv("DATABASE_URL")
	if databaseURL == "" {
		return errors.New("DATABASE_URL must be configured")
	}
	pool, err := pgxpool.New(ctx, databaseURL)
	if err != nil {
		return errors.New("DATABASE_URL is invalid")
	}
	defer pool.Close()
	if err := pool.Ping(ctx); err != nil {
		return errors.New("could not connect to the configured server database")
	}
	_, err = messagingbootstrap.AuthorizeWeixin(ctx, pool, mode, scope, input, output)
	return err
}
