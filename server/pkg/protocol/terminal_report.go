package protocol

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
)

// TerminalReportIdentity is stable across every delivery of one terminal result.
// ClaimFence is decimal text because claim fences exceed JavaScript's safe integer range.
type TerminalReportIdentity struct {
	ReportID      string `json:"report_id"`
	ClaimFence    string `json:"claim_fence"`
	PayloadSHA256 string `json:"payload_sha256"`
}

// TerminalReportAck is returned only after the receipt and terminal effects commit.
type TerminalReportAck struct {
	TerminalReportIdentity
	TaskID     string `json:"task_id"`
	Status     string `json:"status"`
	TaskStatus string `json:"task_status"`
}

// TerminalReportDigest hashes the original callback before server sanitization or
// completion-to-failure normalization. Object keys and whitespace are normalized;
// the terminal_report envelope is excluded and the endpoint kind is included.
func TerminalReportDigest(kind string, payload []byte) (string, error) {
	if kind != "complete" && kind != "fail" {
		return "", errors.New("invalid terminal report kind")
	}
	var body map[string]any
	decoder := json.NewDecoder(bytes.NewReader(payload))
	decoder.UseNumber()
	if err := decoder.Decode(&body); err != nil {
		return "", err
	}
	if body == nil {
		return "", errors.New("terminal report body must be an object")
	}
	delete(body, "terminal_report")
	canonical, err := json.Marshal(body)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(append([]byte(kind+"\n"), canonical...))
	return hex.EncodeToString(sum[:]), nil
}
