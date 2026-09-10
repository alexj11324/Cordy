package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/spf13/cobra"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
)

func TestMain(m *testing.M) {
	for _, key := range []string{
		"ORVILO_AGENT_ID",
		"ORVILO_TASK_ID",
		"ORVILO_TOKEN",
		"ORVILO_DAEMON_PORT",
		"ORVILO_WORKSPACE_ID",
		"ORVILO_SERVER_URL",
		"ORVILO_TASK_CONFIG_ROOT",
	} {
		os.Unsetenv(key)
	}
	os.Exit(m.Run())
}

// testCmd returns a minimal cobra.Command with the --profile persistent flag
// registered, matching the rootCmd setup used in production.
func testCmd() *cobra.Command {
	cmd := &cobra.Command{}
	cmd.PersistentFlags().String("profile", "", "")
	return cmd
}

// TestLoginTokenFlagWiring asserts the production loginCmd flag is registered
// the way #1994 needs it to be: a String flag (not Bool) with a NoOptDefVal
// so `--token` (no value) keeps its legacy prompt-mode behavior. This is the
// load-bearing regression guard — without these asserts a future change that
// reverts the flag to Bool could pass while a synthetic stand-in test happily
// keeps testing string-flag parsing.
func TestLoginTokenFlagWiring(t *testing.T) {
	tokenFlag := loginCmd.Flags().Lookup("token")
	if tokenFlag == nil {
		t.Fatal("loginCmd is missing the --token flag")
	}
	if got := tokenFlag.Value.Type(); got != "string" {
		t.Fatalf("loginCmd --token type = %q, want %q (regressed to bool?)", got, "string")
	}
	if tokenFlag.NoOptDefVal != tokenPromptSentinel {
		t.Fatalf("loginCmd --token NoOptDefVal = %q, want %q (legacy `orvilo login --token` prompt mode would break)", tokenFlag.NoOptDefVal, tokenPromptSentinel)
	}
}

// TestLoginTokenHelpOutputRendersCleanly renders loginCmd's flag help through
// the same pflag path `orvilo login -h` uses (FlagUsagesWrapped) and locks the
// user-visible help contract that regressed. The original bug had two causes,
// both invisible to the flag-wiring/parsing tests above:
//   - The NoOptDefVal sentinel was "\x00prompt". pflag renders NoOptDefVal
//     verbatim into the flag column AND uses "\x00" as its own column-alignment
//     marker, so the split-at-first-NUL logic mispadded the line and printed a
//     raw NUL to the terminal.
//   - The usage string wrapped the PAT example in backticks, so pflag's
//     UnquoteUsage hijacked it as the flag's value placeholder and stripped a
//     backtick pair from the description.
//
// A comment can't prevent either from recurring; only rendering the real output
// and asserting on it can. This is the regression guard for that output.
func TestLoginTokenHelpOutputRendersCleanly(t *testing.T) {
	help := loginCmd.Flags().FlagUsages()

	// No control byte may reach the terminal. pflag emits only spaces and
	// newlines for layout, so any other sub-0x20 rune (notably the NUL the old
	// "\x00prompt" sentinel leaked) means the help rendering is corrupted.
	for _, r := range help {
		if r < 0x20 && r != '\n' && r != '\t' {
			t.Fatalf("login help contains control byte %#x; rendered flag usage:\n%q", r, help)
		}
	}

	// The --token line must show the standard optional-value form. This single
	// assertion pins both root causes: the placeholder is `string` (backticks
	// removed, so UnquoteUsage no longer hijacks the PAT example) and the
	// optional value is the printable `prompt` (no NUL-prefixed sentinel).
	if want := `--token string[="prompt"]`; !strings.Contains(help, want) {
		t.Fatalf("login help missing %q; rendered flag usage:\n%q", want, help)
	}

	// The description must survive intact — a swallowed backtick pair used to
	// truncate it, so assert the tail of the sentence is still present.
	if want := "to be prompted interactively."; !strings.Contains(help, want) {
		t.Fatalf("login help missing description tail %q; rendered flag usage:\n%q", want, help)
	}
}

// TestLoginTokenFlagParsing exercises every documented invocation form
// against a cobra command wired up exactly the same way as the production
// loginCmd, then runs runAuthLogin's flag-resolution logic to confirm the
// right downstream branch is taken: `--token ovy_xxx` and `--token=ovy_xxx`
// both consume the value (the bug from #1994), `--token` alone falls
// through to the prompt sentinel (preserves the legacy headless form), and
// no flag at all starts the device authorization flow.
func TestLoginTokenFlagParsing(t *testing.T) {
	type want struct {
		changed         bool
		resolvedToken   string // empty == "fall through to prompt"
		expectsPrompted bool
	}

	cases := []struct {
		name string
		argv []string
		want want
	}{
		{
			name: "space-separated value (the form from #1994)",
			argv: []string{"--token", "ovy_xxx"},
			want: want{changed: true, resolvedToken: "ovy_xxx"},
		},
		{
			name: "equals-separated value",
			argv: []string{"--token=ovy_yyy"},
			want: want{changed: true, resolvedToken: "ovy_yyy"},
		},
		{
			name: "no value falls through to prompt (legacy CLI_INSTALL.md form)",
			argv: []string{"--token"},
			want: want{changed: true, expectsPrompted: true},
		},
		{
			name: "explicit empty value also falls through to prompt",
			argv: []string{"--token="},
			want: want{changed: true, expectsPrompted: true},
		},
		{
			name: "no flag at all → device authorization flow",
			argv: []string{},
			want: want{changed: false},
		},
	}

	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			cmd := &cobra.Command{Use: "login"}
			// Mirror loginCmd's exact flag wiring. If init() in cmd_login.go
			// regresses, TestLoginTokenFlagWiring catches that; here we test
			// the parsing behavior given the documented wiring.
			cmd.Flags().String("token", "", "")
			cmd.Flags().Lookup("token").NoOptDefVal = tokenPromptSentinel

			if err := cmd.ParseFlags(tc.argv); err != nil {
				t.Fatalf("ParseFlags(%v) error: %v", tc.argv, err)
			}
			if cmd.Flags().Changed("token") != tc.want.changed {
				t.Fatalf("Changed(token) = %v, want %v for argv=%v", cmd.Flags().Changed("token"), tc.want.changed, tc.argv)
			}
			if !tc.want.changed {
				return
			}

			// Replay runAuthLogin's resolution logic so the test fails if
			// either the flag wiring OR the space-form recovery breaks.
			tokenFlag, _ := cmd.Flags().GetString("token")
			positional := cmd.Flags().Args()
			if tokenFlag == tokenPromptSentinel && len(positional) == 1 {
				tokenFlag = positional[0]
			}

			if tc.want.expectsPrompted {
				if tokenFlag != tokenPromptSentinel && tokenFlag != "" {
					t.Fatalf("expected prompt fall-through, got resolved token %q", tokenFlag)
				}
			} else {
				if tokenFlag != tc.want.resolvedToken {
					t.Fatalf("resolved token = %q, want %q", tokenFlag, tc.want.resolvedToken)
				}
			}
		})
	}
}

func TestDeviceAuthorizationPollDelay(t *testing.T) {
	pending := &cli.HTTPError{StatusCode: 400, Body: `{"error":"authorization_pending"}`}
	if got, ok := deviceAuthorizationPollDelay(pending, 5*time.Second); !ok || got != 5*time.Second {
		t.Fatalf("pending delay = (%s, %v), want (5s, true)", got, ok)
	}
	slowDown := &cli.HTTPError{StatusCode: 400, Body: `{"error":"slow_down"}`}
	if got, ok := deviceAuthorizationPollDelay(slowDown, 5*time.Second); !ok || got != 10*time.Second {
		t.Fatalf("slow_down delay = (%s, %v), want (10s, true)", got, ok)
	}
	denied := &cli.HTTPError{StatusCode: 400, Body: `{"error":"access_denied"}`}
	if got, ok := deviceAuthorizationPollDelay(denied, 5*time.Second); ok || got != 0 {
		t.Fatalf("denied delay = (%s, %v), want (0, false)", got, ok)
	}
}

func TestRunAuthLoginDeviceUsesServerPollingAndSavesPAT(t *testing.T) {
	t.Chdir(t.TempDir())
	t.Setenv("HOME", t.TempDir())
	t.Setenv("ORVILO_TOKEN", "")
	t.Setenv("ORVILO_AGENT_ID", "")
	t.Setenv("ORVILO_TASK_ID", "")
	t.Setenv("ORVILO_DAEMON_PORT", "")
	t.Setenv("ORVILO_TASK_CONFIG_ROOT", "")

	const rawPAT = "ovy_device_test_token"
	var paths []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		if got := r.Header.Get("Authorization"); got != "" && r.URL.Path != "/api/me" {
			t.Fatalf("device request carried unexpected authorization header %q", got)
		}
		switch r.URL.Path {
		case "/api/auth/device/code":
			if r.Method != http.MethodPost {
				t.Fatalf("device code method = %s, want POST", r.Method)
			}
			var body map[string]string
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatalf("decode device code body: %v", err)
			}
			if !strings.HasPrefix(body["client_name"], "CLI (") {
				t.Fatalf("client_name = %q, want CLI label", body["client_name"])
			}
			_ = json.NewEncoder(w).Encode(map[string]any{
				"device_code":      "odc_test_device_code",
				"user_code":        "ABCD-EFGH",
				"verification_uri": "https://orvilo.example/device",
				"expires_in":       600,
				"interval":         1,
			})
		case "/api/auth/device/token":
			if r.Method != http.MethodPost {
				t.Fatalf("device token method = %s, want POST", r.Method)
			}
			var body map[string]string
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatalf("decode device token body: %v", err)
			}
			if body["device_code"] != "odc_test_device_code" {
				t.Fatalf("device_code = %q", body["device_code"])
			}
			_ = json.NewEncoder(w).Encode(map[string]string{"access_token": rawPAT, "token_type": "Bearer"})
		case "/api/me":
			if got := r.Header.Get("Authorization"); got != "Bearer "+rawPAT {
				t.Fatalf("me authorization = %q, want PAT", got)
			}
			_ = json.NewEncoder(w).Encode(map[string]string{"name": "Ada", "email": "ada@example.test"})
		default:
			t.Fatalf("unexpected request: %s %s", r.Method, r.URL.Path)
		}
	}))
	defer srv.Close()
	t.Setenv("ORVILO_SERVER_URL", srv.URL)

	stderr := captureStderr(t)
	err := runAuthLoginDevice(testCmd())
	out := stderr.read()
	if err != nil {
		t.Fatalf("runAuthLoginDevice: %v", err)
	}
	if !strings.Contains(out, "https://orvilo.example/device") || !strings.Contains(out, "ABCD-EFGH") {
		t.Fatalf("device instructions missing from stderr: %q", out)
	}
	if strings.Contains(out, "callback") || strings.Contains(out, "Opening browser") {
		t.Fatalf("device login still advertised callback/browser flow: %q", out)
	}
	if !strings.Contains(strings.Join(paths, ","), "/api/auth/device/code") || !strings.Contains(strings.Join(paths, ","), "/api/auth/device/token") {
		t.Fatalf("device endpoints were not called: %v", paths)
	}
	cfg, err := cli.LoadCLIConfigForProfile("")
	if err != nil {
		t.Fatalf("LoadCLIConfig: %v", err)
	}
	if cfg.Token != rawPAT || cfg.ServerURL != srv.URL {
		t.Fatalf("config = %#v, want saved device PAT and server", cfg)
	}
}

func TestRunAuthStatusTaskContextDoesNotPrintCredential(t *testing.T) {
	const fakeTaskToken = "mat_task_status_sentinel"
	t.Setenv("HOME", t.TempDir())
	t.Setenv("ORVILO_AGENT_ID", "agent-test")
	t.Setenv("ORVILO_TASK_ID", "task-test")
	t.Setenv("ORVILO_TOKEN", fakeTaskToken)
	t.Setenv("ORVILO_TASK_CONFIG_ROOT", filepath.Join(t.TempDir(), "task-orvilo"))

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/me" {
			http.NotFound(w, r)
			return
		}
		if got := r.Header.Get("Authorization"); got != "Bearer "+fakeTaskToken {
			t.Errorf("Authorization = %q, want fake task token", got)
		}
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "Task Agent", "email": "task@example.test"})
	}))
	defer srv.Close()
	t.Setenv("ORVILO_SERVER_URL", srv.URL)

	stderr := captureStderr(t)
	if err := runAuthStatus(testCmd(), nil); err != nil {
		stderr.restore()
		t.Fatalf("runAuthStatus: %v", err)
	}
	out := stderr.read()
	for _, forbidden := range []string{fakeTaskToken, fakeTaskToken[:12], "Token:"} {
		if strings.Contains(out, forbidden) {
			t.Fatalf("auth status exposed credential material %q:\n%s", forbidden, out)
		}
	}
	for _, want := range []string{"Server:", "Task Agent", "task@example.test"} {
		if !strings.Contains(out, want) {
			t.Fatalf("auth status output missing %q:\n%s", want, out)
		}
	}
}

func TestRunAuthStatusTaskContextRequiresTaskToken(t *testing.T) {
	ownerHome := t.TempDir()
	t.Setenv("HOME", ownerHome)
	t.Setenv("ORVILO_AGENT_ID", "agent-test")
	t.Setenv("ORVILO_TASK_ID", "task-test")
	t.Setenv("ORVILO_TASK_CONFIG_ROOT", filepath.Join(t.TempDir(), "task-orvilo"))

	ownerPath := filepath.Join(ownerHome, ".orvilo", "config.json")
	if err := os.MkdirAll(filepath.Dir(ownerPath), 0o755); err != nil {
		t.Fatal(err)
	}
	ownerBytes := []byte("{\n  \"server_url\": \"https://owner.invalid\",\n  \"token\": \"ovy_owner_sentinel\"\n}\n")
	if err := os.WriteFile(ownerPath, ownerBytes, 0o600); err != nil {
		t.Fatal(err)
	}

	requestCount := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		requestCount++
		_ = json.NewEncoder(w).Encode(map[string]string{"name": "Unexpected", "email": "unexpected@example.test"})
	}))
	defer srv.Close()
	t.Setenv("ORVILO_SERVER_URL", srv.URL)

	for _, tc := range []struct {
		name  string
		token string
	}{
		{name: "missing token", token: ""},
		{name: "human token", token: "ovy_owner_sentinel"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("ORVILO_TOKEN", tc.token)
			requestCount = 0
			stderr := captureStderr(t)
			err := runAuthStatus(testCmd(), nil)
			stderr.restore()
			out := stderr.read()
			if err == nil || !strings.Contains(err.Error(), "task-scoped mat_ token") {
				t.Fatalf("runAuthStatus error = %v, want task token requirement", err)
			}
			if requestCount != 0 {
				t.Fatalf("auth status made %d request(s) without a task token", requestCount)
			}
			for _, forbidden := range []string{tc.token, "Token:"} {
				if forbidden != "" && strings.Contains(out, forbidden) {
					t.Fatalf("auth status exposed credential material %q: %s", forbidden, out)
				}
			}
		})
	}

	after, err := os.ReadFile(ownerPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(after) != string(ownerBytes) {
		t.Fatalf("owner config content changed: got %q", after)
	}
}

func TestHumanAuthCommandsFailClosedInTaskContext(t *testing.T) {
	ownerHome := t.TempDir()
	t.Setenv("HOME", ownerHome)
	t.Setenv("ORVILO_AGENT_ID", "agent-test")
	t.Setenv("ORVILO_TASK_ID", "task-test")
	t.Setenv("ORVILO_TOKEN", "mat_task_sentinel")
	t.Setenv("ORVILO_SERVER_URL", "https://task.invalid")
	t.Setenv("ORVILO_TASK_CONFIG_ROOT", filepath.Join(t.TempDir(), "task-orvilo"))

	ownerPath := filepath.Join(ownerHome, ".orvilo", "config.json")
	if err := os.MkdirAll(filepath.Dir(ownerPath), 0o755); err != nil {
		t.Fatal(err)
	}
	ownerBytes := []byte("{\n  \"server_url\": \"https://owner.invalid\",\n  \"token\": \"ovy_owner_sentinel\"\n}\n")
	if err := os.WriteFile(ownerPath, ownerBytes, 0o600); err != nil {
		t.Fatal(err)
	}

	loginCmd := testCmd()
	loginCmd.Flags().String("token", "ovy_fake_login", "")
	_ = loginCmd.Flags().Set("token", "ovy_fake_login")
	for name, run := range map[string]func() error{
		"login":  func() error { return runAuthLogin(loginCmd, nil) },
		"logout": func() error { return runAuthLogout(testCmd(), nil) },
	} {
		err := run()
		if err == nil || !strings.Contains(err.Error(), "not available inside a daemon-managed task") {
			t.Fatalf("%s error = %v, want task-context guard", name, err)
		}
	}
	after, err := os.ReadFile(ownerPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(after) != string(ownerBytes) {
		t.Fatalf("owner config content changed: got %q", after)
	}
}

func TestNormalizeAPIBaseURL(t *testing.T) {
	t.Run("converts websocket base URL", func(t *testing.T) {
		if got := normalizeAPIBaseURL("ws://localhost:18106/ws"); got != "http://localhost:18106" {
			t.Fatalf("normalizeAPIBaseURL() = %q, want %q", got, "http://localhost:18106")
		}
	})

	t.Run("keeps http base URL", func(t *testing.T) {
		if got := normalizeAPIBaseURL("http://localhost:8080"); got != "http://localhost:8080" {
			t.Fatalf("normalizeAPIBaseURL() = %q, want %q", got, "http://localhost:8080")
		}
	})

	t.Run("falls back to raw value for invalid URL", func(t *testing.T) {
		if got := normalizeAPIBaseURL("://bad-url"); got != "://bad-url" {
			t.Fatalf("normalizeAPIBaseURL() = %q, want %q", got, "://bad-url")
		}
	})
}

// TestValidateLoginTokenPrefix pins the accepted PAT prefix set for
// `orvilo login --token`. The original implementation hardcoded `ovy_`
// only, which rejected legitimate Orvilo Cloud Node PATs (`mcn_`) at
// the CLI even though the server's middleware would have accepted them.
// If a future change drops `mcn_` from the list (or accidentally
// broadens the set to anything-goes), this test fails.
func TestValidateLoginTokenPrefix(t *testing.T) {
	cases := []struct {
		name    string
		token   string
		wantErr bool
	}{
		{name: "ovy_ PAT", token: "ovy_abc123", wantErr: false},
		{name: "mcn_ Cloud Node PAT", token: "mcn_abc123", wantErr: false},
		{name: "empty token", token: "", wantErr: true},
		{name: "no prefix", token: "abc123", wantErr: true},
		{name: "wrong prefix mdt_", token: "mdt_abc123", wantErr: true},
		{name: "wrong prefix mat_", token: "mat_abc123", wantErr: true},
		{name: "case-sensitive: MUL_ rejected", token: "MUL_abc123", wantErr: true},
		{name: "leading whitespace not allowed (callers TrimSpace first)", token: " ovy_abc", wantErr: true},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			err := validateLoginTokenPrefix(tc.token)
			if tc.wantErr && err == nil {
				t.Fatalf("validateLoginTokenPrefix(%q) = nil, want error", tc.token)
			}
			if !tc.wantErr && err != nil {
				t.Fatalf("validateLoginTokenPrefix(%q) = %v, want nil", tc.token, err)
			}
		})
	}

	// The error string is user-facing; make sure it lists every accepted
	// prefix so users hitting it can self-serve. Hardcoding the exact
	// prefixes here is deliberate — if someone adds a new prefix to
	// loginTokenPrefixes they should also update the docs / this test.
	err := validateLoginTokenPrefix("nope_xxx")
	if err == nil {
		t.Fatal("expected error for unknown prefix")
	}
	for _, p := range []string{"ovy_", "mcn_"} {
		if !strings.Contains(err.Error(), p) {
			t.Errorf("error %q does not mention prefix %q", err.Error(), p)
		}
	}
}
