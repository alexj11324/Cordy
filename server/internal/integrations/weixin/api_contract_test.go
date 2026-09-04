package weixin

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"testing"
)

func TestClientRequestQRCodeUsesILinkContract(t *testing.T) {
	var (
		gotPath  string
		gotQuery string
		gotBody  struct {
			LocalTokens []string `json:"local_token_list"`
		}
	)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath, gotQuery = r.URL.Path, r.URL.RawQuery
		if r.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", r.Method)
		}
		assertCommonHeaders(t, r, true)
		if err := json.NewDecoder(r.Body).Decode(&gotBody); err != nil {
			t.Errorf("decode request: %v", err)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"qrcode":"qr-token","qrcode_img_content":"data:image/png;base64,fixture"}`)
	}))
	defer server.Close()

	got, err := NewClient(server.URL, "", server.Client()).RequestQRCode(t.Context(), []string{"old-token"})
	if err != nil {
		t.Fatal(err)
	}
	if gotPath != "/ilink/bot/get_bot_qrcode" || gotQuery != "bot_type=3" {
		t.Fatalf("request target = %s?%s, want iLink QR endpoint", gotPath, gotQuery)
	}
	if len(gotBody.LocalTokens) != 1 || gotBody.LocalTokens[0] != "old-token" {
		t.Fatalf("local_token_list = %#v", gotBody.LocalTokens)
	}
	if got.QRCode != "qr-token" || got.QRCodeImageData == "" {
		t.Fatalf("QR response = %#v", got)
	}
}

func TestClientQRStatusEscapesQueryAndDoesNotAuthenticate(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/ilink/bot/get_qrcode_status" {
			t.Fatalf("request = %s %s, want GET status endpoint", r.Method, r.URL.String())
		}
		if got := r.URL.Query().Get("qrcode"); got != "qr/a?b" {
			t.Errorf("qrcode = %q", got)
		}
		if got := r.URL.Query().Get("verify_code"); got != "12 3" {
			t.Errorf("verify_code = %q", got)
		}
		assertCommonHeaders(t, r, false)
		if got := r.Header.Get("Authorization"); got != "" {
			t.Errorf("unexpected authorization header %q", got)
		}
		_, _ = io.WriteString(w, `{"status":"need_verifycode"}`)
	}))
	defer server.Close()

	got, err := NewClient(server.URL, "token", server.Client()).QRStatus(t.Context(), "qr/a?b", "12 3")
	if err != nil {
		t.Fatal(err)
	}
	if got.Status != "need_verifycode" {
		t.Fatalf("status = %q", got.Status)
	}
}

func TestClientGetUpdatesSendsOpaqueCursorAndBaseInfo(t *testing.T) {
	var gotBody struct {
		Cursor   string   `json:"get_updates_buf"`
		BaseInfo BaseInfo `json:"base_info"`
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/ilink/bot/getupdates" {
			t.Fatalf("request = %s %s", r.Method, r.URL.String())
		}
		assertCommonHeaders(t, r, true)
		if err := json.NewDecoder(r.Body).Decode(&gotBody); err != nil {
			t.Errorf("decode request: %v", err)
		}
		_, _ = io.WriteString(w, `{"ret":0,"msgs":[{"seq":7,"message_id":99,"from_user_id":"wx-user","message_type":1,"context_token":"ctx"}],"get_updates_buf":"next-cursor","longpolling_timeout_ms":15000}`)
	}))
	defer server.Close()

	got, err := NewClient(server.URL, "bot-token", server.Client()).GetUpdates(t.Context(), "opaque-cursor")
	if err != nil {
		t.Fatal(err)
	}
	if gotBody.Cursor != "opaque-cursor" {
		t.Errorf("cursor = %q", gotBody.Cursor)
	}
	if gotBody.BaseInfo.ChannelVersion != clientVersion || gotBody.BaseInfo.BotAgent != botAgent {
		t.Errorf("base_info = %#v", gotBody.BaseInfo)
	}
	if got.NextCursor != "next-cursor" || len(got.Messages) != 1 || got.Messages[0].MessageID != 99 {
		t.Fatalf("updates = %#v", got)
	}
}

func TestClientSendTextChunksUnicodeAndReturnsClientCorrelationIDs(t *testing.T) {
	var mu sync.Mutex
	var chunks []string
	var ids []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/ilink/bot/sendmessage" {
			t.Fatalf("request = %s %s", r.Method, r.URL.String())
		}
		assertCommonHeaders(t, r, true)
		var request struct {
			Msg struct {
				ClientID     string        `json:"client_id"`
				ToUserID     string        `json:"to_user_id"`
				MessageType  int           `json:"message_type"`
				MessageState int           `json:"message_state"`
				ContextToken string        `json:"context_token"`
				Items        []MessageItem `json:"item_list"`
			} `json:"msg"`
			BaseInfo BaseInfo `json:"base_info"`
		}
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Errorf("decode request: %v", err)
		}
		if request.Msg.ToUserID != "wx-user" || request.Msg.ContextToken != "ctx" || request.Msg.MessageType != 2 || request.Msg.MessageState != 2 {
			t.Errorf("message envelope = %#v", request.Msg)
		}
		if len(request.Msg.Items) != 1 || request.Msg.Items[0].Type != 1 || request.Msg.Items[0].TextItem == nil {
			t.Errorf("items = %#v", request.Msg.Items)
			return
		}
		mu.Lock()
		chunks = append(chunks, request.Msg.Items[0].TextItem.Text)
		ids = append(ids, request.Msg.ClientID)
		mu.Unlock()
		_, _ = io.WriteString(w, `{"ret":0}`)
	}))
	defer server.Close()

	input := strings.Repeat("界", maxTextChunk) + "尾"
	got, err := NewClient(server.URL, "bot-token", server.Client()).SendText(t.Context(), "wx-user", "ctx", input)
	if err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(chunks) != 2 || len(got) != 2 || len(ids) != 2 {
		t.Fatalf("chunks = %d, returned ids = %d, request ids = %d", len(chunks), len(got), len(ids))
	}
	if strings.Join(chunks, "") != input {
		t.Fatalf("reassembled text differs from input")
	}
	for i, chunk := range chunks {
		if n := len([]rune(chunk)); n > maxTextChunk {
			t.Errorf("chunk %d has %d runes", i, n)
		}
		if got[i] == "" || got[i] != ids[i] {
			t.Errorf("correlation id %d = %q/%q", i, got[i], ids[i])
		}
	}
	if got[0] == got[1] {
		t.Errorf("chunk correlation ids are not unique: %q", got)
	}
}

func assertCommonHeaders(t *testing.T, r *http.Request, authenticated bool) {
	t.Helper()
	if got := r.Header.Get("iLink-App-Id"); got != "bot" {
		t.Errorf("iLink-App-Id = %q", got)
	}
	if got := r.Header.Get("iLink-App-ClientVersion"); got != "256" {
		t.Errorf("iLink-App-ClientVersion = %q", got)
	}
	if !authenticated {
		return
	}
	if got := r.Header.Get("AuthorizationType"); got != "ilink_bot_token" {
		t.Errorf("AuthorizationType = %q", got)
	}
	if got := r.Header.Get("Authorization"); got != "Bearer bot-token" && got != "" {
		t.Errorf("Authorization = %q", got)
	}
	encoded := r.Header.Get("X-WECHAT-UIN")
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil || encoded == "" {
		t.Errorf("X-WECHAT-UIN is not base64: %q (%v)", encoded, err)
		return
	}
	if _, err := strconv.ParseUint(string(decoded), 10, 32); err != nil {
		t.Errorf("X-WECHAT-UIN payload = %q: %v", decoded, err)
	}
}
