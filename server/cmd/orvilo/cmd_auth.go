package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/orvilo-ai/orvilo/server/internal/auth"
	"github.com/orvilo-ai/orvilo/server/internal/cli"
)

// loginTokenPrefixes are the token prefixes `orvilo login --token` accepts.
// The CLI used to hardcode `ovy_` only, which made it impossible to log in
// with an Orvilo Cloud Node PAT (`mcn_`) even though the server happily
// authenticates both kinds. Keep this list in sync with the prefix branches
// in server/internal/middleware/auth.go.
var loginTokenPrefixes = []string{"ovy_", auth.CloudPATPrefix}

// validateLoginTokenPrefix returns nil if token starts with one of the
// CLI-recognised PAT prefixes, or an error describing the accepted set.
// Extracted so the prefix list has one obvious test surface.
func validateLoginTokenPrefix(token string) error {
	for _, p := range loginTokenPrefixes {
		if strings.HasPrefix(token, p) {
			return nil
		}
	}
	return fmt.Errorf("invalid token format: must start with %s", strings.Join(loginTokenPrefixes, " or "))
}

var authCmd = &cobra.Command{
	Use:   "auth",
	Short: "Authenticate orvilo with Orvilo",
}

var authStatusCmd = &cobra.Command{
	Use:   "status",
	Short: "Show current authentication status",
	RunE:  runAuthStatus,
}

var authLogoutCmd = &cobra.Command{
	Use:   "logout",
	Short: "Remove stored authentication token",
	RunE:  runAuthLogout,
}

// callbackHostFlag is retained so older setup scripts keep parsing cleanly.
// Device authorization no longer starts a local callback server, so this flag
// has no effect on login.
const callbackHostFlag = "callback-host"

const callbackHostFlagHelp = "Deprecated: device authorization uses a browser-entered code and does not start a local callback server."

func init() {
	authCmd.AddCommand(authStatusCmd)
	authCmd.AddCommand(authLogoutCmd)
}

func resolveToken(cmd *cobra.Command) string {
	if v := strings.TrimSpace(os.Getenv("ORVILO_TOKEN")); v != "" {
		return v
	}
	// Inside a daemon-managed task, never fall back to the user-global config
	// token: that silent fallback is how agent writes land as the wrong actor.
	// inDaemonManagedExecutionContext already covers the ORVILO_DAEMON_PORT
	// signal for subprocesses that lost ORVILO_AGENT_ID / ORVILO_TASK_ID.
	if inDaemonManagedExecutionContext() {
		return ""
	}
	profile := resolveProfile(cmd)
	cfg, _ := cli.LoadCLIConfigForProfile(profile)
	return cfg.Token
}

func openBrowser(url string) error {
	var cmd string
	var args []string
	switch runtime.GOOS {
	case "darwin":
		cmd = "open"
		args = []string{url}
	case "linux":
		cmd = "xdg-open"
		args = []string{url}
	case "windows":
		cmd = "rundll32"
		args = []string{"url.dll,FileProtocolHandler", url}
	default:
		return fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}
	return exec.Command(cmd, args...).Start()
}

func runAuthLogin(cmd *cobra.Command, args []string) error {
	if err := requireHumanLocalCommand("login"); err != nil {
		return err
	}
	if cmd.Flags().Changed("token") {
		tokenFlag, _ := cmd.Flags().GetString("token")
		// `--token ovy_xxx` (space form) is what users actually type — that's
		// the form from the docs and from #1994. NoOptDefVal prevents pflag
		// from consuming the next arg as the flag value, so it lands here as
		// a positional. Promote it to the token value.
		if tokenFlag == tokenPromptSentinel && len(args) == 1 {
			tokenFlag = args[0]
		}
		return runAuthLoginToken(cmd, tokenFlag)
	}
	return runAuthLoginDevice(cmd)
}

type deviceAuthorizationCodeResponse struct {
	DeviceCode      string `json:"device_code"`
	UserCode        string `json:"user_code"`
	VerificationURI string `json:"verification_uri"`
	ExpiresIn       int    `json:"expires_in"`
	Interval        int    `json:"interval"`
}

type deviceAuthorizationTokenResponse struct {
	AccessToken string `json:"access_token"`
	TokenType   string `json:"token_type"`
}

type deviceAuthorizationErrorResponse struct {
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description"`
}

func cliDeviceClientName() string {
	hostname, err := os.Hostname()
	if err != nil || strings.TrimSpace(hostname) == "" {
		hostname = "unknown"
	}
	return fmt.Sprintf("CLI (%s)", strings.TrimSpace(hostname))
}

func parseDeviceAuthorizationError(err error) (deviceAuthorizationErrorResponse, bool) {
	var httpErr *cli.HTTPError
	if !errors.As(err, &httpErr) {
		return deviceAuthorizationErrorResponse{}, false
	}
	var parsed deviceAuthorizationErrorResponse
	if json.Unmarshal([]byte(httpErr.Body), &parsed) != nil || strings.TrimSpace(parsed.Error) == "" {
		return deviceAuthorizationErrorResponse{}, false
	}
	return parsed, true
}

func deviceAuthorizationPollDelay(err error, current time.Duration) (time.Duration, bool) {
	parsed, ok := parseDeviceAuthorizationError(err)
	if !ok {
		return 0, false
	}
	switch parsed.Error {
	case "authorization_pending":
		return current, true
	case "slow_down":
		// RFC 8628 requires the client to add five seconds to its polling
		// interval after each slow_down response.
		return current + 5*time.Second, true
	default:
		return 0, false
	}
}

func deviceAuthorizationErrorMessage(err error) string {
	parsed, ok := parseDeviceAuthorizationError(err)
	if ok {
		switch parsed.Error {
		case "access_denied":
			return "Authorization was denied in the browser."
		case "expired_token":
			return "The device authorization expired. Run `orvilo login` again."
		case "invalid_grant":
			return "The device authorization is no longer valid. Run `orvilo login` again."
		}
		if parsed.ErrorDescription != "" {
			return parsed.ErrorDescription
		}
	}
	return "Sign-in did not complete. Run `orvilo login` again."
}

// runAuthLoginDevice performs OAuth 2.0 device authorization. The CLI only
// prints a URL and one-time code, then polls the server; it never opens a
// local callback listener or runs a browser-facing HTTP server.
func runAuthLoginDevice(cmd *cobra.Command) error {
	serverURL := resolveHumanServerURL(cmd)
	client := cli.NewAPIClient(serverURL, "", "")

	requestCtx, requestCancel := cli.APIContext(context.Background())
	var codeResp deviceAuthorizationCodeResponse
	err := client.PostJSON(requestCtx, "/api/auth/device/code", map[string]string{
		"client_name": cliDeviceClientName(),
	}, &codeResp)
	requestCancel()
	if err != nil {
		return cli.WithUserMessage("Could not start device sign-in. Check the server URL and try again.", err)
	}
	if codeResp.DeviceCode == "" || codeResp.UserCode == "" || codeResp.VerificationURI == "" || codeResp.ExpiresIn <= 0 {
		return fmt.Errorf("server returned an incomplete device authorization")
	}
	if codeResp.Interval <= 0 {
		codeResp.Interval = 5
	}

	fmt.Fprintln(os.Stderr, "\nTo sign in, open this URL on a computer with a browser:")
	fmt.Fprintf(os.Stderr, "  %s\n", codeResp.VerificationURI)
	fmt.Fprintf(os.Stderr, "Enter this one-time code: %s\n", codeResp.UserCode)
	fmt.Fprintln(os.Stderr, "Waiting for authorization...")

	deadline := time.Now().Add(time.Duration(codeResp.ExpiresIn) * time.Second)
	pollInterval := time.Duration(codeResp.Interval) * time.Second
	for {
		if !time.Now().Before(deadline) {
			return fmt.Errorf("device authorization timed out; run `orvilo login` again")
		}

		pollCtx, pollCancel := context.WithDeadline(context.Background(), deadline)
		requestCtx, requestCancel = context.WithTimeout(pollCtx, cli.APITimeout())
		var tokenResp deviceAuthorizationTokenResponse
		err = client.PostJSON(requestCtx, "/api/auth/device/token", map[string]string{
			"device_code": codeResp.DeviceCode,
		}, &tokenResp)
		requestCancel()
		pollCancel()
		if err == nil {
			if tokenResp.AccessToken == "" || !strings.EqualFold(tokenResp.TokenType, "bearer") {
				return fmt.Errorf("server returned an incomplete device token")
			}
			return saveAuthenticatedLogin(cmd, serverURL, tokenResp.AccessToken)
		}

		if next, pending := deviceAuthorizationPollDelay(err, pollInterval); pending {
			pollInterval = next
			remaining := time.Until(deadline)
			if remaining <= pollInterval {
				return fmt.Errorf("device authorization timed out; run `orvilo login` again")
			}
			timer := time.NewTimer(pollInterval)
			<-timer.C
			continue
		}
		return fmt.Errorf("%s", deviceAuthorizationErrorMessage(err))
	}
}

func saveAuthenticatedLogin(cmd *cobra.Command, serverURL, token string) error {
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	patClient := cli.NewAPIClient(serverURL, "", token)
	var me struct {
		Name  string `json:"name"`
		Email string `json:"email"`
	}
	if err := patClient.GetJSON(ctx, "/api/me", &me); err != nil {
		return cli.WithUserMessage("Sign-in did not complete: the server did not accept the new credential. Run `orvilo login` again.", err)
	}

	profile := resolveProfile(cmd)
	cfg, _ := cli.LoadCLIConfigForProfile(profile)
	cfg.WorkspaceID = ""
	cfg.Token = token
	cfg.ServerURL = serverURL
	if cfg.AppURL == "" && serverURL == defaultCloudServerURL {
		cfg.AppURL = defaultCloudAppURL
	}
	if err := cli.SaveCLIConfigForProfile(cfg, profile); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}

	fmt.Fprintf(os.Stderr, "Authenticated as %s (%s)\nToken saved to config.\n", me.Name, me.Email)
	return nil
}

func runAuthLoginToken(cmd *cobra.Command, providedToken string) error {
	// The prompt sentinel is what pflag substitutes for `--token` with no
	// value (see loginCmd init); treat it the same as an empty string so we
	// fall through to the interactive prompt.
	if providedToken == tokenPromptSentinel {
		providedToken = ""
	}
	token := strings.TrimSpace(providedToken)
	if token == "" {
		fmt.Print("Enter your personal access token: ")
		scanner := bufio.NewScanner(os.Stdin)
		if !scanner.Scan() {
			return fmt.Errorf("no input")
		}
		token = strings.TrimSpace(scanner.Text())
	}
	if token == "" {
		return fmt.Errorf("token is required")
	}
	if err := validateLoginTokenPrefix(token); err != nil {
		return err
	}

	serverURL := resolveLoginTokenServerURL(cmd)
	client := cli.NewAPIClient(serverURL, "", token)

	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	var me struct {
		Name  string `json:"name"`
		Email string `json:"email"`
	}
	if err := client.GetJSON(ctx, "/api/me", &me); err != nil {
		return cli.WithUserMessage("Could not sign in with that token — make sure it is valid and not expired, then run `orvilo login --token <token>` again.", err)
	}

	profile := resolveProfile(cmd)
	cfg, _ := cli.LoadCLIConfigForProfile(profile)
	cfg.WorkspaceID = ""
	cfg.Token = token
	cfg.ServerURL = serverURL
	if cfg.AppURL == "" && serverURL == defaultCloudServerURL {
		cfg.AppURL = defaultCloudAppURL
	}
	if err := cli.SaveCLIConfigForProfile(cfg, profile); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}

	fmt.Fprintf(os.Stderr, "Authenticated as %s (%s)\nToken saved to config.\n", me.Name, me.Email)
	return nil
}

func runAuthStatus(cmd *cobra.Command, _ []string) error {
	if err := requireTaskLocalConfigRoot(); err != nil {
		return err
	}
	taskContext := inDaemonManagedExecutionContext()
	token := resolveToken(cmd)
	if taskContext && !strings.HasPrefix(token, "mat_") {
		return fmt.Errorf("agent execution context requires ORVILO_TOKEN to be a task-scoped mat_ token")
	}
	serverURL := resolveServerURL(cmd)

	if token == "" {
		fmt.Fprintln(os.Stderr, "Not authenticated. Run 'orvilo login' to authenticate.")
		return nil
	}

	client := cli.NewAPIClient(serverURL, "", token)

	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	var me struct {
		Name  string `json:"name"`
		Email string `json:"email"`
	}
	if err := client.GetJSON(ctx, "/api/me", &me); err != nil {
		fmt.Fprintf(os.Stderr, "Token is invalid or expired: %v\nRun 'orvilo login' to re-authenticate.\n", err)
		return nil
	}

	if taskContext {
		fmt.Fprintf(os.Stderr, "Server:  %s\nUser:    %s (%s)\n", serverURL, me.Name, me.Email)
		return nil
	}

	prefix := token
	if len(prefix) > 12 {
		prefix = prefix[:12] + "..."
	}
	fmt.Fprintf(os.Stderr, "Server:  %s\nUser:    %s (%s)\nToken:   %s\n", serverURL, me.Name, me.Email, prefix)
	return nil
}

func runAuthLogout(cmd *cobra.Command, _ []string) error {
	if err := requireHumanLocalCommand("logout"); err != nil {
		return err
	}
	profile := resolveProfile(cmd)
	cfg, _ := cli.LoadCLIConfigForProfile(profile)
	if cfg.Token == "" {
		fmt.Fprintln(os.Stderr, "Not authenticated.")
		return nil
	}

	cfg.Token = ""
	if err := cli.SaveCLIConfigForProfile(cfg, profile); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}

	fmt.Fprintln(os.Stderr, "Token removed. You are now logged out.")
	return nil
}
