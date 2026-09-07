package handler

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"

	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// Decode the original wire object before sanitization so a daemon can replay
// exactly the bytes it persisted, even when the result is rerouted to failure.
func decodeTerminalReportRequest(r *http.Request, kind string, target any) (*service.TerminalReport, error) {
	var raw json.RawMessage
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(&raw); err != nil {
		return nil, err
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		return nil, errors.New("invalid trailing request body")
	}
	if err := json.Unmarshal(raw, target); err != nil {
		return nil, err
	}
	var envelope struct {
		TerminalReport *protocol.TerminalReportIdentity `json:"terminal_report"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil {
		return nil, err
	}
	if envelope.TerminalReport == nil {
		return nil, nil
	}
	report, err := service.NewTerminalReport(*envelope.TerminalReport)
	if err != nil {
		return nil, err
	}
	digest, err := protocol.TerminalReportDigest(kind, raw)
	if err != nil {
		return nil, err
	}
	if digest != report.Identity.PayloadSHA256 {
		return nil, errors.New("terminal report payload hash mismatch")
	}
	return report, nil
}

func writeTerminalReportError(w http.ResponseWriter, err error) bool {
	if errors.Is(err, service.ErrTerminalReportConflict) || errors.Is(err, service.ErrTerminalReportStaleClaim) {
		writeError(w, http.StatusConflict, err.Error())
		return true
	}
	return false
}
