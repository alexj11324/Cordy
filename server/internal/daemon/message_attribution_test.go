package daemon

import (
	"context"
	"log/slog"
	"sync/atomic"
	"testing"

	"github.com/orvilo-ai/orvilo/server/pkg/agent"
)

type attributedTranscriptBackend struct{}

func (attributedTranscriptBackend) Execute(context.Context, string, agent.ExecOptions) (*agent.Session, error) {
	messages := make(chan agent.Message, 3)
	messages <- agent.Message{
		Type:      agent.MessageText,
		Content:   "alpha",
		Sources:   []agent.MessageSource{{ID: "a", URL: "https://example.test/a", Title: "A"}},
		Citations: []agent.MessageCitation{{SourceID: "a", Start: 0, End: 5}},
	}
	messages <- agent.Message{
		Type:      agent.MessageText,
		Content:   " beta",
		Sources:   []agent.MessageSource{{ID: "a", URL: "https://example.test/b"}},
		Citations: []agent.MessageCitation{{SourceID: "a", Start: 1, End: 5}},
	}
	// A Markdown link without provider metadata must stay ordinary text.
	messages <- agent.Message{Type: agent.MessageText, Content: " [link](https://example.test/plain)"}
	close(messages)
	result := make(chan agent.Result, 1)
	result <- agent.Result{Status: "completed", Output: "alpha beta [link](https://example.test/plain)"}
	return &agent.Session{Messages: messages, Result: result}, nil
}

func TestExecuteAndDrainCarriesOnlyProviderAuthoredAttribution(t *testing.T) {
	t.Parallel()
	d, recorder := newTranscriptRecorder(t)
	result, _, err := d.executeAndDrain(
		context.Background(), attributedTranscriptBackend{}, "prompt", agent.ExecOptions{},
		slog.Default(), "task-attribution", "", new(atomic.Int32),
	)
	if err != nil || result.Status != "completed" {
		t.Fatalf("executeAndDrain result=%+v err=%v", result, err)
	}
	messages := recorder.snapshot()
	if len(messages) != 1 {
		t.Fatalf("reported messages = %#v, want one coalesced text message", messages)
	}
	got := messages[0]
	if got.Content != "alpha beta [link](https://example.test/plain)" || len(got.Sources) != 2 {
		t.Fatalf("reported attribution = %#v", got)
	}
	want := []agent.MessageCitation{
		{SourceID: "a", Start: 0, End: 5},
		{SourceID: "a#2", Start: 6, End: 10},
	}
	if got.Sources[1].ID != "a#2" {
		t.Fatalf("conflicting chunk-local source id was not remapped: %#v", got.Sources)
	}
	if len(got.Citations) != len(want) {
		t.Fatalf("citations = %#v, want %#v", got.Citations, want)
	}
	for i := range want {
		if got.Citations[i] != want[i] {
			t.Fatalf("citations = %#v, want %#v", got.Citations, want)
		}
	}
}
