package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestUpdateMePersistsCompleteProfileDetails(t *testing.T) {
	id := newLanguageTestUser(t, "complete-profile@orvilo.ai")
	w := httptest.NewRecorder()
	testHandler.UpdateMe(w, newPatchMeRequest(id, `{"profile_details":{"first_name":"Alex","last_name":"Jiang","preferred_name":"Alex J","username":"alexj","role":"product-ops","phone":"+12065551243","website":"https://example.com","start_week":"monday","time_format":"24-hour"},"timezone":"America/New_York","language":"en"}`))
	if w.Code != http.StatusOK {
		t.Fatalf("update: %d %s", w.Code, w.Body.String())
	}
	var got struct {
		Name    string            `json:"name"`
		Details map[string]string `json:"profile_details"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Details["username"] != "alexj" || got.Name != "Alex J" {
		t.Fatalf("profile not persisted: %+v", got)
	}
	w = httptest.NewRecorder()
	testHandler.UpdateMe(w, newPatchMeRequest(id, `{"profile_details":{"time_format":"12-hour"}}`))
	if w.Code != http.StatusOK {
		t.Fatalf("patch: %d %s", w.Code, w.Body.String())
	}
	got.Details = nil
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Details["username"] != "alexj" || got.Details["time_format"] != "12-hour" || got.Name != "Alex J" {
		t.Fatalf("partial update lost profile: %+v", got)
	}
}

func TestUpdateMeSeedsFirstNameFromExistingDisplayName(t *testing.T) {
	id := newLanguageTestUser(t, "profile-name-seed@orvilo.ai")
	w := httptest.NewRecorder()
	testHandler.UpdateMe(w, newPatchMeRequest(id, `{"profile_details":{"last_name":"Hopper"}}`))
	if w.Code != http.StatusOK {
		t.Fatalf("update: %d %s", w.Code, w.Body.String())
	}
	var got struct {
		Name    string            `json:"name"`
		Details map[string]string `json:"profile_details"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Name != "Language Test Hopper" || got.Details["first_name"] != "Language Test" {
		t.Fatalf("display-name baseline was not persisted: %+v", got)
	}
}

func TestConcurrentProfileNamePatchesKeepCanonicalDisplayName(t *testing.T) {
	id := newLanguageTestUser(t, "concurrent-profile-name@orvilo.ai")
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if _, err := testPool.Exec(ctx,
		`UPDATE "user" SET name = 'Alice', profile_details = '{}'::jsonb WHERE id = $1`, id,
	); err != nil {
		t.Fatalf("seed account: %v", err)
	}

	holder, err := testPool.Begin(ctx)
	if err != nil {
		t.Fatalf("begin holder: %v", err)
	}
	defer holder.Rollback(context.Background())
	if _, err := holder.Exec(ctx, `
		UPDATE "user"
		SET name = 'Alex', profile_details = '{"first_name":"Alex"}'::jsonb
		WHERE id = $1`, id,
	); err != nil {
		t.Fatalf("lock and update account: %v", err)
	}

	waiter, err := testPool.Acquire(ctx)
	if err != nil {
		t.Fatalf("acquire waiter connection: %v", err)
	}
	defer func() {
		_, _ = waiter.Exec(context.Background(), `RESET application_name`)
		waiter.Release()
	}()
	const waiterName = "profile_name_patch_waiter"
	if _, err := waiter.Exec(ctx, `SET application_name = 'profile_name_patch_waiter'`); err != nil {
		t.Fatalf("name waiter connection: %v", err)
	}
	w := httptest.NewRecorder()
	requestDone := make(chan struct{})
	localHandler := *testHandler
	localHandler.Queries = db.New(waiter)
	go func() {
		localHandler.UpdateMe(w, newPatchMeRequest(id, `{"profile_details":{"last_name":"Jiang"}}`))
		close(requestDone)
	}()

	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		var waiting bool
		if err := holder.QueryRow(ctx, `
			SELECT EXISTS (
				SELECT 1 FROM pg_stat_activity
				WHERE application_name = $1 AND wait_event_type = 'Lock'
			)`, waiterName,
		).Scan(&waiting); err != nil {
			t.Fatalf("inspect blocked request: %v", err)
		}
		if waiting {
			break
		}
		select {
		case <-requestDone:
			t.Fatalf("profile patch completed before the holder committed: %d %s", w.Code, w.Body.String())
		case <-ticker.C:
		case <-ctx.Done():
			t.Fatal("profile patch never blocked on the locked user row")
		}
	}
	if err := holder.Commit(ctx); err != nil {
		t.Fatalf("commit holder: %v", err)
	}
	select {
	case <-requestDone:
	case <-ctx.Done():
		t.Fatal("profile patch did not finish after the holder committed")
	}
	if w.Code != http.StatusOK {
		t.Fatalf("profile patch: %d %s", w.Code, w.Body.String())
	}

	var name string
	var details []byte
	if err := testPool.QueryRow(context.Background(),
		`SELECT name, profile_details FROM "user" WHERE id = $1`, id,
	).Scan(&name, &details); err != nil {
		t.Fatalf("load final profile: %v", err)
	}
	var profile map[string]string
	if err := json.Unmarshal(details, &profile); err != nil {
		t.Fatalf("decode final profile: %v", err)
	}
	if profile["first_name"] != "Alex" || profile["last_name"] != "Jiang" || name != "Alex Jiang" {
		t.Fatalf("concurrent patches diverged: name=%q details=%v", name, profile)
	}
}

func TestUpdateMeRejectsInvalidProfileDetailsAtomically(t *testing.T) {
	id := newLanguageTestUser(t, "invalid-profile@orvilo.ai")
	for _, body := range []string{
		`{"name":"Do not save","profile_details":{"phone":"not-a-phone"}}`,
		`{"profile_details":{"website":"javascript:alert(1)"}}`,
		`{"profile_details":{"start_week":"someday"}}`,
		`{"profile_details":{"time_format":"25-hour"}}`,
		`{"profile_details":{"username":"contains spaces"}}`,
		`{"profile_details":{"role":"owner"}}`,
		`{"profile_details":{"first_name":"line\nfeed"}}`,
		`{"profile_details":{"email":"other@example.com"}}`,
		`{"profile_details":{"username":42}}`,
	} {
		w := httptest.NewRecorder()
		testHandler.UpdateMe(w, newPatchMeRequest(id, body))
		if w.Code != http.StatusBadRequest {
			t.Errorf("body %s: %d %s", body, w.Code, w.Body.String())
		}
	}
	w := httptest.NewRecorder()
	testHandler.UpdateMe(w, newPatchMeRequest(id, `{}`))
	var got struct {
		Name string `json:"name"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.Name != "Language Test" {
		t.Fatalf("invalid patch changed name: %q", got.Name)
	}
}
