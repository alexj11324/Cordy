package handler

import (
	"encoding/json"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/lark"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/wecom"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestRevokedInstallationsExposeDisconnectedStatus(t *testing.T) {
	row := db.ChannelInstallation{Status: "revoked", Config: []byte(`{}`)}
	for name, response := range map[string]any{
		"slack":    slackInstallationToResponse(row),
		"dingtalk": dingtalkInstallationToResponse(row),
		"telegram": telegramInstallationToResponse(row),
		"weixin":   weixinInstallationToResponse(row),
		"lark":     larkInstallationToResponse(lark.Installation{Status: "revoked"}),
		"wecom":    wecomInstallationToResponse(wecom.Installation{Status: "revoked"}),
	} {
		t.Run(name, func(t *testing.T) {
			encoded, err := json.Marshal(response)
			if err != nil {
				t.Fatal(err)
			}
			var got struct {
				Runtime struct {
					State     string `json:"state"`
					ErrorCode string `json:"errorCode"`
				} `json:"runtime"`
			}
			if err := json.Unmarshal(encoded, &got); err != nil {
				t.Fatal(err)
			}
			if got.Runtime.State != "offline" || got.Runtime.ErrorCode != "installation_revoked" {
				t.Fatalf("revoked installation omitted its authoritative disconnected state: %s", encoded)
			}
		})
	}
}
