package weixin

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

const (
	clientVersion = "0.1.0"
	botAgent      = "Patchbay/0.1.0"
	maxTextChunk  = 2000
)

// APIError is returned for an iLink JSON response with a non-zero ret or
// errcode. It keeps the provider's operation and code available to logs/tests
// without pretending a provider message was delivered.
type APIError struct {
	Operation string
	Ret       int
	Message   string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("weixin API %s failed: ret=%d %s", e.Operation, e.Ret, e.Message)
}

type QRCodeResponse struct {
	QRCode          string `json:"qrcode"`
	QRCodeImageData string `json:"qrcode_img_content"`
}

type QRStatusResponse struct {
	Status       string `json:"status"`
	BotToken     string `json:"bot_token"`
	ILinkBotID   string `json:"ilink_bot_id"`
	ILinkUserID  string `json:"ilink_user_id"`
	BaseURL      string `json:"baseurl"`
	RedirectHost string `json:"redirect_host"`
}

type BaseInfo struct {
	ChannelVersion string `json:"channel_version"`
	BotAgent       string `json:"bot_agent"`
}

type TextItem struct {
	Text string `json:"text"`
}

type MessageItem struct {
	Type     int       `json:"type"`
	MsgID    string    `json:"msg_id"`
	TextItem *TextItem `json:"text_item"`
}

// WeixinMessage mirrors the fields used by the Rust iLink adapter. Unknown
// provider fields are intentionally ignored for forward compatibility.
type WeixinMessage struct {
	Seq          int64         `json:"seq"`
	MessageID    int64         `json:"message_id"`
	FromUserID   string        `json:"from_user_id"`
	ToUserID     string        `json:"to_user_id"`
	ClientID     string        `json:"client_id"`
	SessionID    string        `json:"session_id"`
	GroupID      string        `json:"group_id"`
	MessageType  int           `json:"message_type"`
	MessageState int           `json:"message_state"`
	ItemList     []MessageItem `json:"item_list"`
	ContextToken string        `json:"context_token"`
}

type GetUpdatesResponse struct {
	Ret                int             `json:"ret"`
	ErrCode            int             `json:"errcode"`
	ErrMsg             string          `json:"errmsg"`
	Messages           []WeixinMessage `json:"msgs"`
	NextCursor         string          `json:"get_updates_buf"`
	LongPollingTimeout int64           `json:"longpolling_timeout_ms"`
}

type apiResponse struct {
	Ret     int    `json:"ret"`
	ErrCode int    `json:"errcode"`
	ErrMsg  string `json:"errmsg"`
}

// Client is a small net/http implementation of the documented iLink
// endpoints. It deliberately does not expose arbitrary path/request methods;
// adding an endpoint requires a reviewed provider contract.
type Client struct {
	httpClient *http.Client
	baseURL    string
	token      string
}

// NewClient constructs a client. Redirects are disabled because a provider
// redirect could otherwise move an Authorization header to an untrusted host.
func NewClient(baseURL, token string, httpClient *http.Client) *Client {
	if httpClient == nil {
		httpClient = &http.Client{Timeout: 40 * time.Second}
	}
	copyClient := *httpClient
	copyClient.CheckRedirect = func(_ *http.Request, _ []*http.Request) error {
		return http.ErrUseLastResponse
	}
	return &Client{
		httpClient: &copyClient,
		baseURL:    normalizeBaseURL(baseURL),
		token:      strings.TrimSpace(token),
	}
}

// RequestQRCode calls the real QR endpoint from the Rust mainline adapter.
func (c *Client) RequestQRCode(ctx context.Context, localTokens []string) (QRCodeResponse, error) {
	var out QRCodeResponse
	err := c.doJSON(ctx, http.MethodPost, "ilink/bot/get_bot_qrcode?bot_type=3", map[string]any{
		"local_token_list": localTokens,
	}, &out, true)
	if err != nil {
		return QRCodeResponse{}, err
	}
	if strings.TrimSpace(out.QRCode) == "" {
		return QRCodeResponse{}, errors.New("weixin: QR response was incomplete")
	}
	return out, nil
}

// QRStatus calls the real QR polling endpoint. verifyCode is omitted when
// empty, matching the Rust adapter's query construction.
func (c *Client) QRStatus(ctx context.Context, qrCode, verifyCode string) (QRStatusResponse, error) {
	path := "ilink/bot/get_qrcode_status?qrcode=" + url.QueryEscape(qrCode)
	if strings.TrimSpace(verifyCode) != "" {
		path += "&verify_code=" + url.QueryEscape(strings.TrimSpace(verifyCode))
	}
	var out QRStatusResponse
	if err := c.doJSON(ctx, http.MethodGet, path, nil, &out, false); err != nil {
		return QRStatusResponse{}, err
	}
	return out, nil
}

// GetUpdates performs the documented HTTP long-poll call and returns the
// opaque cursor exactly as supplied by iLink.
func (c *Client) GetUpdates(ctx context.Context, cursor string) (GetUpdatesResponse, error) {
	var out GetUpdatesResponse
	err := c.doJSON(ctx, http.MethodPost, "ilink/bot/getupdates", map[string]any{
		"get_updates_buf": cursor,
		"base_info":       baseInfo(),
	}, &out, true)
	if err != nil {
		return GetUpdatesResponse{}, err
	}
	if out.Ret != 0 || out.ErrCode != 0 {
		code := out.Ret
		if out.ErrCode != 0 {
			code = out.ErrCode
		}
		return GetUpdatesResponse{}, &APIError{Operation: "getupdates", Ret: code, Message: out.ErrMsg}
	}
	return out, nil
}

// SendText sends direct text using iLink's documented sendmessage shape. The
// return values are the client_id values submitted for each successful chunk.
// iLink's response does not contain a provider message id, so callers must
// label these as client correlation ids rather than inventing provider ids.
func (c *Client) SendText(ctx context.Context, toUserID, contextToken, text string) ([]string, error) {
	if strings.TrimSpace(toUserID) == "" || strings.TrimSpace(contextToken) == "" {
		return nil, errors.New("weixin: outbound target requires user id and context token")
	}
	chunks := chunkText(text, maxTextChunk)
	ids := make([]string, 0, len(chunks))
	for _, chunk := range chunks {
		clientID := uuid.NewString()
		var out apiResponse
		err := c.doJSON(ctx, http.MethodPost, "ilink/bot/sendmessage", map[string]any{
			"msg": map[string]any{
				"client_id":     clientID,
				"from_user_id":  "",
				"to_user_id":    toUserID,
				"message_type":  2,
				"message_state": 2,
				"context_token": contextToken,
				"item_list": []any{map[string]any{
					"type":      1,
					"text_item": map[string]string{"text": chunk},
				}},
			},
			"base_info": baseInfo(),
		}, &out, true)
		if err != nil {
			return ids, err
		}
		if out.Ret != 0 || out.ErrCode != 0 {
			code := out.Ret
			if out.ErrCode != 0 {
				code = out.ErrCode
			}
			return ids, &APIError{Operation: "sendmessage", Ret: code, Message: out.ErrMsg}
		}
		ids = append(ids, clientID)
	}
	return ids, nil
}

func (c *Client) doJSON(ctx context.Context, method, path string, payload any, dst any, authenticated bool) error {
	var body io.Reader
	if payload != nil {
		encoded, err := json.Marshal(payload)
		if err != nil {
			return err
		}
		body = bytes.NewReader(encoded)
	}
	request, err := http.NewRequestWithContext(ctx, method, c.baseURL+"/"+path, body)
	if err != nil {
		return fmt.Errorf("weixin: build %s request: %w", method, err)
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("iLink-App-Id", "bot")
	request.Header.Set("iLink-App-ClientVersion", strconv.FormatInt(encodedClientVersion(), 10))
	if authenticated {
		request.Header.Set("AuthorizationType", "ilink_bot_token")
		request.Header.Set("X-WECHAT-UIN", randomWeixinUIN())
		if c.token != "" {
			request.Header.Set("Authorization", "Bearer "+c.token)
		}
	}
	response, err := c.httpClient.Do(request)
	if err != nil {
		return fmt.Errorf("weixin: %s request: %w", method, err)
	}
	defer response.Body.Close()
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return fmt.Errorf("weixin: %s returned HTTP %d", method, response.StatusCode)
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 4<<20)).Decode(dst); err != nil {
		return fmt.Errorf("weixin: decode %s response: %w", method, err)
	}
	return nil
}

func baseInfo() BaseInfo {
	return BaseInfo{ChannelVersion: clientVersion, BotAgent: botAgent}
}

func encodedClientVersion() int64 {
	parts := strings.Split(clientVersion, ".")
	values := [3]int64{}
	for i := range values {
		if i < len(parts) {
			values[i], _ = strconv.ParseInt(parts[i], 10, 64)
		}
	}
	// iLink encodes semantic-version components in three successive bytes;
	// the Rust 0.1.0 mainline client therefore sends 0x000100 (256).
	return ((values[0] & 0xff) << 16) | ((values[1] & 0xff) << 8) | (values[2] & 0xff)
}

func randomWeixinUIN() string {
	var raw [4]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return base64.StdEncoding.EncodeToString([]byte("0"))
	}
	value := uint32(raw[0])<<24 | uint32(raw[1])<<16 | uint32(raw[2])<<8 | uint32(raw[3])
	return base64.StdEncoding.EncodeToString([]byte(strconv.FormatUint(uint64(value), 10)))
}

func chunkText(text string, limit int) []string {
	if text == "" || limit <= 0 {
		return nil
	}
	runes := []rune(text)
	chunks := make([]string, 0, int(math.Ceil(float64(len(runes))/float64(limit))))
	for len(runes) > 0 {
		n := limit
		if len(runes) < n {
			n = len(runes)
		}
		chunks = append(chunks, string(runes[:n]))
		runes = runes[n:]
	}
	return chunks
}
