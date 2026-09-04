package weixin

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
)

func TestUnsupportedDirectMessageGetsContractNotice(t *testing.T) {
	var got struct {
		ToUserID string `json:"to_user_id"`
		Text     string `json:"-"`
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/ilink/bot/sendmessage" {
			t.Fatalf("path = %q", r.URL.Path)
		}
		var request struct {
			Msg struct {
				ToUserID string        `json:"to_user_id"`
				Items    []MessageItem `json:"item_list"`
			} `json:"msg"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Fatal(err)
		}
		got.ToUserID = request.Msg.ToUserID
		if len(request.Msg.Items) != 1 || request.Msg.Items[0].TextItem == nil {
			t.Fatalf("notice items = %#v", request.Msg.Items)
		}
		got.Text = request.Msg.Items[0].TextItem.Text
		_, _ = io.WriteString(w, `{"ret":0}`)
	}))
	defer server.Close()

	client := NewClient(server.URL, "token", server.Client())
	adapter := &weixinChannel{client: client, botID: "bot-id", contextByChat: make(map[string]string)}
	message := WeixinMessage{
		FromUserID: "wx-user", MessageType: 1, ContextToken: "ctx",
		ItemList: []MessageItem{{Type: 2, TextItem: &TextItem{Text: "image"}}},
	}
	if err := adapter.dispatch(t.Context(), message); err != nil {
		t.Fatal(err)
	}
	if got.ToUserID != "wx-user" || got.Text != unsupportedMessageText {
		t.Fatalf("unsupported notice = %#v", got)
	}
}

func TestUnsupportedMessageNoticeFailsClosedForSelfAndGroup(t *testing.T) {
	base := WeixinMessage{
		FromUserID: "wx-user", MessageType: 1, ContextToken: "ctx",
		ItemList: []MessageItem{{Type: 2, TextItem: &TextItem{Text: "image"}}},
	}
	if !shouldNotifyUnsupported(base, "bot-id") {
		t.Fatal("valid unsupported direct message was not classified")
	}
	for _, edit := range []func(*WeixinMessage){
		func(m *WeixinMessage) { m.FromUserID = "bot-id" },
		func(m *WeixinMessage) { m.GroupID = "group-id" },
		func(m *WeixinMessage) { m.ContextToken = "" },
		func(m *WeixinMessage) { m.MessageType = 2 },
		func(m *WeixinMessage) { m.ItemList = nil },
	} {
		message := base
		edit(&message)
		if shouldNotifyUnsupported(message, "bot-id") {
			t.Fatalf("unsafe message was classified for notice: %#v", message)
		}
	}
}

func TestWeixinChannelDeclaresOnlyText(t *testing.T) {
	adapter := &weixinChannel{}
	if adapter.Type() != TypeWeixin || !adapter.Capabilities().Has(channel.CapText) || adapter.Capabilities() != channel.CapText {
		t.Fatalf("channel contract = type %q capabilities %s", adapter.Type(), adapter.Capabilities())
	}
}
