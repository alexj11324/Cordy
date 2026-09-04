package weixin

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestValidateProviderBaseURLEnforcesRedirectBoundary(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		want    string
		wantErr bool
	}{
		{name: "default", input: "", want: DefaultBaseURL},
		{name: "documented host", input: "https://ilinkai.weixin.qq.com/path?q=ignored", want: DefaultBaseURL},
		{name: "regional host", input: "https://region.weixin.qq.com:443/v1", want: "https://region.weixin.qq.com"},
		{name: "provider host without scheme", input: "region.weixin.qq.com/path", want: "https://region.weixin.qq.com"},
		{name: "wrong scheme", input: "http://ilinkai.weixin.qq.com", wantErr: true},
		{name: "wrong host", input: "https://example.com", wantErr: true},
		{name: "host suffix confusion", input: "https://weixin.qq.com.example.com", wantErr: true},
		{name: "unsafe port", input: "https://region.weixin.qq.com:8443", wantErr: true},
		{name: "userinfo", input: "https://user:pass@region.weixin.qq.com", wantErr: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ValidateProviderBaseURL(tt.input)
			if tt.wantErr {
				if err == nil {
					t.Fatalf("expected error, got %q", got)
				}
				return
			}
			if err != nil || got != tt.want {
				t.Fatalf("ValidateProviderBaseURL(%q) = %q, %v; want %q", tt.input, got, err, tt.want)
			}
		})
	}
}

func TestInstallSessionTTLMatchesProviderContract(t *testing.T) {
	if InstallSessionTTLSeconds != 300 {
		t.Fatalf("InstallSessionTTLSeconds = %d, want 300", InstallSessionTTLSeconds)
	}
}

func TestDecodeCredentialsDecryptsOnlyOpaqueBotToken(t *testing.T) {
	ciphertext := []byte("sealed-token")
	raw, err := json.Marshal(installConfig{
		AppID:             "bot-id",
		ILinkUserID:       "wx-user",
		BaseURL:           "https://region.weixin.qq.com/ignored",
		BotTokenEncrypted: base64.StdEncoding.EncodeToString(ciphertext),
	})
	if err != nil {
		t.Fatal(err)
	}
	var opened []byte
	credentials, err := DecodeCredentials(raw, func(value []byte) ([]byte, error) {
		opened = append([]byte(nil), value...)
		return []byte("opaque-provider-token"), nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if string(opened) != string(ciphertext) || credentials.BotToken != "opaque-provider-token" {
		t.Fatalf("opened = %q, credentials = %#v", opened, credentials)
	}
	if credentials.BaseURL != "https://region.weixin.qq.com" {
		t.Fatalf("base URL = %q", credentials.BaseURL)
	}
	public, publicErr := json.Marshal(DecodePublicConfig(raw))
	if publicErr != nil {
		t.Fatal(publicErr)
	}
	if strings.Contains(string(public), "opaque-provider-token") || strings.Contains(string(public), "sealed-token") {
		t.Fatalf("public config leaked token: %s", public)
	}
}

func TestWeixinMigrationAndQueryContract(t *testing.T) {
	up := readWeixinSQLFile(t, "529_channel_receive_state.up.sql")
	indexUp := readWeixinSQLFile(t, "530_channel_receive_state_unique_index.up.sql")
	indexDown := readWeixinSQLFile(t, "530_channel_receive_state_unique_index.down.sql")
	tableDown := readWeixinSQLFile(t, "529_channel_receive_state.down.sql")
	query := readWeixinQueryFile(t)

	sql := withoutSQLComments(up)
	for _, forbidden := range []string{"FOREIGN KEY", "REFERENCES", "CASCADE"} {
		if strings.Contains(strings.ToUpper(sql), forbidden) {
			t.Errorf("table migration contains forbidden %q", forbidden)
		}
	}
	for _, required := range []string{"CREATE TABLE channel_receive_state", "installation_id UUID NOT NULL", "channel_type TEXT NOT NULL", "cursor TEXT NOT NULL"} {
		if !strings.Contains(sql, required) {
			t.Errorf("table migration missing %q", required)
		}
	}
	if got := strings.Count(withoutSQLComments(indexUp), ";"); got != 1 || !strings.Contains(strings.ToUpper(indexUp), "CREATE UNIQUE INDEX CONCURRENTLY") {
		t.Errorf("index-up is not one concurrent statement: %s", indexUp)
	}
	if !strings.Contains(strings.ToUpper(indexDown), "DROP INDEX CONCURRENTLY") {
		t.Errorf("index-down is not concurrent: %s", indexDown)
	}
	if !strings.Contains(strings.ToUpper(tableDown), "DROP TABLE IF EXISTS CHANNEL_RECEIVE_STATE") {
		t.Errorf("table-down does not drop receive state: %s", tableDown)
	}
	if strings.Contains(query, "$4::jsonb") || !strings.Contains(query, "$3::jsonb") {
		t.Fatalf("Weixin binding merge query has wrong parameter contract")
	}
}

func readWeixinSQLFile(t *testing.T, name string) string {
	t.Helper()
	return readWeixinFile(t, filepath.Join("server", "migrations", name))
}

func readWeixinQueryFile(t *testing.T) string {
	t.Helper()
	return readWeixinFile(t, filepath.Join("server", "pkg", "db", "queries", "weixin.sql"))
}

func readWeixinFile(t *testing.T, relative string) string {
	t.Helper()
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	repoRoot := filepath.Clean(filepath.Join(filepath.Dir(source), "../../../.."))
	path := filepath.Join(repoRoot, relative)
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	return string(contents)
}

func withoutSQLComments(sql string) string {
	lines := strings.Split(sql, "\n")
	filtered := lines[:0]
	for _, line := range lines {
		if strings.HasPrefix(strings.TrimSpace(line), "--") {
			continue
		}
		filtered = append(filtered, line)
	}
	return strings.Join(filtered, "\n")
}

func TestDecodeCredentialsRejectsMissingToken(t *testing.T) {
	raw := []byte(fmt.Sprintf(`{"app_id":"bot-id","bot_token_encrypted":%q}`, base64.StdEncoding.EncodeToString(nil)))
	if _, err := DecodeCredentials(raw, nil); !errors.Is(err, errMissingCredentials) {
		t.Fatalf("error = %v, want missing credentials", err)
	}
}
