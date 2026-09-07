package agent

import (
	"bytes"
	"encoding/json"
)

// acpJSONValue accepts the ACP currentValue shapes we have to live with:
// a string (select), a boolean (type:boolean), or null. Unknown shapes
// become empty rather than failing the parent option list — a sibling
// boolean Fast toggle must not hide the model catalog or the effort dial.
type acpJSONValue string

func (v *acpJSONValue) UnmarshalJSON(data []byte) error {
	data = bytes.TrimSpace(data)
	if len(data) == 0 || bytes.Equal(data, []byte("null")) {
		*v = ""
		return nil
	}
	var s string
	if err := json.Unmarshal(data, &s); err == nil {
		*v = acpJSONValue(s)
		return nil
	}
	var b bool
	if err := json.Unmarshal(data, &b); err == nil {
		if b {
			*v = "true"
		} else {
			*v = "false"
		}
		return nil
	}
	*v = ""
	return nil
}

func (v acpJSONValue) String() string {
	return string(v)
}

// acpClientCapabilities is the initialize handshake Patchbay sends to ACP
// runtimes. Extra keys (Kimi's `terminal: true`) merge in at the top level.
//
// `session.configOptions.boolean: {}` is required before a v1 agent may
// include type:boolean options such as Fast mode. Omitting it is why
// discovery previously could not tell "this runtime has no speed toggle"
// from "this runtime has a boolean Fast toggle we never asked to see".
func acpClientCapabilities(extra map[string]any) map[string]any {
	caps := map[string]any{
		"session": map[string]any{
			"configOptions": map[string]any{
				"boolean": map[string]any{},
			},
		},
	}
	for k, v := range extra {
		if k == "session" {
			continue
		}
		caps[k] = v
	}
	return caps
}
