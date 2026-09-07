package protocol

import "testing"

func TestTerminalReportDigestNormalizesWireObject(t *testing.T) {
	a, err := TerminalReportDigest("complete", []byte(`{"output":"done\u0000 result","work_dir":"/tmp/run","nested":{"b":2,"a":1}}`))
	if err != nil {
		t.Fatal(err)
	}
	b, err := TerminalReportDigest("complete", []byte(`{ "nested": {"a":1,"b":2}, "work_dir":"/tmp/run", "output":"done\u0000 result", "terminal_report":{"report_id":"ignored"} }`))
	if err != nil {
		t.Fatal(err)
	}
	if a != b {
		t.Fatalf("same payload has different digests: %s %s", a, b)
	}
	other, err := TerminalReportDigest("fail", []byte(`{"output":"done\u0000 result","work_dir":"/tmp/run","nested":{"a":1,"b":2}}`))
	if err != nil {
		t.Fatal(err)
	}
	if a == other {
		t.Fatal("endpoint kind must participate in the digest")
	}
	sanitized, err := TerminalReportDigest("complete", []byte(`{"output":"done result","work_dir":"/tmp/run","nested":{"a":1,"b":2}}`))
	if err != nil {
		t.Fatal(err)
	}
	if a == sanitized {
		t.Fatal("hash must describe original bytes before NUL sanitization")
	}
}

func TestTerminalReportDigestRejectsMalformedBody(t *testing.T) {
	for _, body := range []string{`null`, `[]`, `"text"`, `{invalid`} {
		if _, err := TerminalReportDigest("complete", []byte(body)); err == nil {
			t.Fatalf("accepted %s", body)
		}
	}
	if _, err := TerminalReportDigest("cancel", []byte(`{}`)); err == nil {
		t.Fatal("accepted unknown terminal kind")
	}
}
