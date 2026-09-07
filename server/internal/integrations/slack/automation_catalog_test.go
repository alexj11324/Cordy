package slack

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestAutomationCatalogUsesInstalledProviderIdentities(t *testing.T) {
	box := testBox(t)
	sealed, err := box.Seal([]byte("xoxb-catalog"))
	if err != nil {
		t.Fatal(err)
	}
	config, _ := json.Marshal(installConfig{
		AppID: "A1", TeamID: "T1", BotUserID: "UBOT",
		BotTokenEncrypted: base64.StdEncoding.EncodeToString(sealed),
	})
	installationID := mustUUID(t, "11111111-1111-1111-1111-111111111111")
	q := &fakeInstallQueries{installations: []db.ChannelInstallation{{
		ID: installationID, ChannelType: "slack", Status: "installed", Config: config,
	}}}
	svc := newTestInstallService(t, q)
	provider := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/conversations.list":
			_, _ = w.Write([]byte(`{"ok":true,"channels":[{"id":"C1","name":"incidents","is_channel":true,"is_member":true},{"id":"C2","name":"unjoined","is_channel":true,"is_member":false}],"response_metadata":{"next_cursor":""}}`))
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(provider.Close)
	svc.apiURL = provider.URL

	catalog, err := svc.AutomationCatalog(context.Background(), mustUUID(t, "22222222-2222-2222-2222-222222222222"))
	if err != nil {
		t.Fatalf("AutomationCatalog: %v", err)
	}
	if len(catalog.Channels) != 1 || catalog.Channels[0].ID != "C1" || catalog.Channels[0].InstallationID != "11111111-1111-1111-1111-111111111111" {
		t.Fatalf("channels = %+v", catalog.Channels)
	}
}

func TestAutomationCatalogExcludesRevokedInstallations(t *testing.T) {
	q := &fakeInstallQueries{installations: []db.ChannelInstallation{{Status: "revoked"}}}
	svc := newTestInstallService(t, q)
	catalog, err := svc.AutomationCatalog(context.Background(), mustUUID(t, "22222222-2222-2222-2222-222222222222"))
	if err != nil {
		t.Fatal(err)
	}
	if len(catalog.Channels) != 0 {
		t.Fatalf("revoked installation leaked into catalog: %+v", catalog)
	}
}
