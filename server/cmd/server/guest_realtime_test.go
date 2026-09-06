package main

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

func TestGuestRealtimeLifecycle(t *testing.T) {
	resp, err := http.Post(testServer.URL+"/auth/guest", "application/json", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("guest entry status: %d", resp.StatusCode)
	}
	var session struct {
		Token string `json:"token"`
		User  struct {
			ID string `json:"id"`
		} `json:"user"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		t.Fatal(err)
	}
	if session.Token == "" || session.User.ID == "" {
		t.Fatal("missing guest identity")
	}
	t.Cleanup(func() {
		for _, query := range []string{
			`DELETE FROM member WHERE user_id=$1`,
			`DELETE FROM guest_session WHERE user_id=$1`,
			`DELETE FROM "user" WHERE id=$1`,
		} {
			if _, err := testPool.Exec(context.Background(), query, session.User.ID); err != nil {
				t.Errorf("guest fixture cleanup: %v", err)
			}
		}
	})
	if _, err := testPool.Exec(context.Background(), `INSERT INTO member (workspace_id,user_id,role) VALUES ($1,$2,'member')`, testWorkspaceID, session.User.ID); err != nil {
		t.Fatal(err)
	}
	conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(testServer.URL, "http")+"/ws?workspace_id="+testWorkspaceID, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if err := conn.WriteJSON(map[string]any{"type": "auth", "payload": map[string]string{"token": session.Token}}); err != nil {
		t.Fatal(err)
	}
	readType := func(want string) {
		t.Helper()
		conn.SetReadDeadline(time.Now().Add(3 * time.Second))
		var frame struct {
			Type string `json:"type"`
		}
		if err := conn.ReadJSON(&frame); err != nil {
			t.Fatal(err)
		}
		if frame.Type != want {
			t.Fatalf("frame type %q, want %q", frame.Type, want)
		}
	}
	readType("auth_ack")
	subscribe := map[string]any{"type": "subscribe", "payload": map[string]string{"scope": "workspace", "id": testWorkspaceID}}
	if err := conn.WriteJSON(subscribe); err != nil {
		t.Fatal(err)
	}
	readType("subscribe_ack")
	req, err := http.NewRequest(http.MethodPost, testServer.URL+"/auth/logout", nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Authorization", "Bearer "+session.Token)
	logout, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	logout.Body.Close()
	if logout.StatusCode < 200 || logout.StatusCode >= 300 {
		t.Fatalf("logout status: %d", logout.StatusCode)
	}
	if err := conn.WriteJSON(subscribe); err != nil {
		t.Fatal(err)
	}
	conn.SetReadDeadline(time.Now().Add(3 * time.Second))
	if _, _, err := conn.ReadMessage(); err == nil {
		t.Fatal("revoked guest connection still receives frames")
	} else if e, ok := err.(net.Error); ok && e.Timeout() {
		t.Fatal("revoked guest connection was not closed")
	}
}
