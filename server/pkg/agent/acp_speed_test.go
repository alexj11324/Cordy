package agent

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
)

// codexACPFastModeSessionResult is the shape codex-acp advertises for Fast
// mode: select on/off, id `fast-mode`. Category is the option id, not
// `model_config`. Matching must therefore not require that category.
const codexACPFastModeSessionResult = `{"sessionId":"ses-codex-acp",` +
	`"models":{"currentModelId":"gpt-5.4","availableModels":[` +
	`{"modelId":"gpt-5.4","name":"GPT-5.4"},{"modelId":"gpt-5.5","name":"GPT-5.5"}]},` +
	`"configOptions":[{"id":"fast-mode","name":"Fast mode","description":"1.5x speed, increased usage",` +
	`"category":"fast-mode","type":"select","currentValue":"off","options":[` +
	`{"value":"off","name":"Off","description":"Default speed, normal usage"},` +
	`{"value":"on","name":"On","description":"1.5x speed, increased usage"}]}]}`

const acpFastModeBooleanSessionResult = `{"sessionId":"ses-bool",` +
	`"models":{"currentModelId":"sonnet","availableModels":[{"modelId":"sonnet","name":"Sonnet"}]},` +
	`"configOptions":[` +
	`{"id":"model","name":"Model","category":"model","type":"select","currentValue":"sonnet",` +
	`"options":[{"value":"sonnet","name":"Sonnet"}]},` +
	`{"id":"context_size","name":"Context Size","category":"model_config","type":"select","currentValue":"200k",` +
	`"options":[{"value":"200k","name":"200K"},{"value":"1m","name":"1M"}]},` +
	`{"id":"fast_mode","name":"Fast Mode","category":"model_config","type":"boolean","currentValue":false}` +
	`]}`

func TestParseACPSpeedOptionCodexACPFastMode(t *testing.T) {
	t.Parallel()
	option, ok := parseACPSpeedOption(json.RawMessage(codexACPFastModeSessionResult))
	if !ok {
		t.Fatal("parseACPSpeedOption found no speed option in the codex-acp capture")
	}
	if option.ConfigID != "fast-mode" {
		t.Errorf("ConfigID = %q, want fast-mode", option.ConfigID)
	}
	if got := strings.Join(option.values(), ","); got != "off,on" {
		t.Errorf("values = %q, want off,on", got)
	}
	if option.CurrentValue != "off" {
		t.Errorf("CurrentValue = %q, want off", option.CurrentValue)
	}
	if option.Choices[1].Name != "On" {
		t.Errorf("Choices[1].Name = %q, want On", option.Choices[1].Name)
	}
}

func TestParseACPSpeedOptionBooleanFastModeIgnoresContextSize(t *testing.T) {
	t.Parallel()
	option, ok := parseACPSpeedOption(json.RawMessage(acpFastModeBooleanSessionResult))
	if !ok {
		t.Fatal("boolean fast_mode under model_config was not recognised")
	}
	if option.ConfigID != "fast_mode" || !option.boolean() {
		t.Errorf("option = %+v, want fast_mode boolean", option)
	}
	if got := strings.Join(option.values(), ","); got != "false,true" {
		t.Errorf("values = %q, want false,true", got)
	}
	if option.CurrentValue != "false" {
		t.Errorf("CurrentValue = %q, want false (JSON boolean)", option.CurrentValue)
	}
	if option.Choices[1].Name != "Fast Mode" {
		t.Errorf("on label = %q, want the option's own name", option.Choices[1].Name)
	}
}

func TestParseACPSpeedOptionAbsent(t *testing.T) {
	t.Parallel()
	for _, raw := range []string{
		reasonixEffortSessionResult,
		`{"sessionId":"s","models":{"currentModelId":"m","availableModels":[{"modelId":"m"}]},"modes":{}}`,
		`{"configOptions":[{"id":"context_size","name":"Context Size","category":"model_config","options":[{"value":"200k"}]}]}`,
		`{"configOptions":[{"id":"temperature","name":"Temperature","category":"model_config","options":[{"value":"1"}]}]}`,
		`{"configOptions":[{"id":"model","category":"model","currentValue":"m","options":[{"value":"m","name":"M"}]}]}`,
		`not json at all`,
	} {
		if _, ok := parseACPSpeedOption(json.RawMessage(raw)); ok {
			t.Errorf("parseACPSpeedOption(%s) reported a speed option where there is none", raw)
		}
	}
}

func TestParseACPSpeedOptionSnakeCase(t *testing.T) {
	t.Parallel()
	raw := json.RawMessage(`{"config_options":[{"id":"service_tier","current_value":"priority",` +
		`"options":[{"value":"flex","name":"Flex"},{"value":"priority","name":"Fast"}]}]}`)
	option, ok := parseACPSpeedOption(raw)
	if !ok {
		t.Fatal("snake_case config_options was not recognised")
	}
	if option.ConfigID != "service_tier" || option.CurrentValue != "priority" {
		t.Errorf("option = %+v, want service_tier/priority", option)
	}
}

func TestAnnotateACPSpeedForSessionModelOnlyDefault(t *testing.T) {
	t.Parallel()
	models := []Model{
		{ID: "gpt-5.4", Label: "GPT-5.4", Default: true},
		{ID: "gpt-5.5", Label: "GPT-5.5"},
	}
	annotateACPSpeedForSessionModel(models, json.RawMessage(codexACPFastModeSessionResult))
	if len(models[0].ServiceTiers) != 2 || models[0].ServiceTiers[0].ID != "off" {
		t.Errorf("default model tiers = %+v, want off/on", models[0].ServiceTiers)
	}
	if models[1].ServiceTiers != nil {
		t.Errorf("sibling model inherited speed catalog: %+v", models[1].ServiceTiers)
	}
}

func TestAnnotateACPSessionConfigLeavesHermesAgentUntouched(t *testing.T) {
	t.Parallel()
	models := []Model{{ID: "m", Default: true}}
	annotateACPSessionConfig(models, json.RawMessage(`{"sessionId":"s","models":{"currentModelId":"m","availableModels":[{"modelId":"m"}]},"modes":{}}`))
	if models[0].Thinking != nil {
		t.Errorf("Thinking = %+v, want nil", models[0].Thinking)
	}
	if models[0].ServiceTiers != nil {
		t.Errorf("ServiceTiers = %+v, want nil", models[0].ServiceTiers)
	}
}

func TestApplyACPSpeedOptionSendsAdvertisedID(t *testing.T) {
	t.Parallel()
	echo := `{"configOptions":[{"id":"fast-mode","currentValue":"on","options":[` +
		`{"value":"off"},{"value":"on"}]}]}`
	request, calls := recordingACPRequest(echo, nil)

	applyACPSpeedOption(context.Background(), request, "hermes", discardLogger(),
		"ses-1", json.RawMessage(codexACPFastModeSessionResult), "on", true)

	if len(*calls) != 1 {
		t.Fatalf("calls = %+v, want exactly one set_config_option", *calls)
	}
	call := (*calls)[0]
	if call.method != "session/set_config_option" {
		t.Errorf("method = %q", call.method)
	}
	if call.params["configId"] != "fast-mode" {
		t.Errorf("configId = %v, want fast-mode", call.params["configId"])
	}
	if call.params["value"] != "on" || call.params["sessionId"] != "ses-1" {
		t.Errorf("params = %+v", call.params)
	}
	if _, ok := call.params["type"]; ok {
		t.Errorf("select options must not send a type discriminator: %+v", call.params)
	}
}

func TestApplyACPSpeedOptionSendsBooleanWireValue(t *testing.T) {
	t.Parallel()
	echo := `{"configOptions":[{"id":"fast_mode","type":"boolean","currentValue":true}]}`
	request, calls := recordingACPRequest(echo, nil)

	applyACPSpeedOption(context.Background(), request, "reasonix", discardLogger(),
		"ses-1", json.RawMessage(acpFastModeBooleanSessionResult), "true", true)

	if len(*calls) != 1 {
		t.Fatalf("calls = %+v", *calls)
	}
	call := (*calls)[0]
	if call.params["configId"] != "fast_mode" {
		t.Errorf("configId = %v", call.params["configId"])
	}
	if call.params["type"] != "boolean" {
		t.Errorf("type = %v, want boolean", call.params["type"])
	}
	if call.params["value"] != true {
		t.Errorf("value = %v (%T), want boolean true", call.params["value"], call.params["value"])
	}
}

func TestApplyACPSpeedOptionNoopsWhenSessionHasNone(t *testing.T) {
	t.Parallel()
	request, calls := recordingACPRequest(`{}`, nil)
	applyACPSpeedOption(context.Background(), request, "hermes", discardLogger(),
		"ses-1", json.RawMessage(`{"sessionId":"s","modes":{}}`), "on", true)
	if len(*calls) != 0 {
		t.Errorf("calls = %+v, want none when the session advertises no speed option", *calls)
	}
}

func TestApplyACPSpeedOptionSkipsUnadvertisedToken(t *testing.T) {
	t.Parallel()
	request, calls := recordingACPRequest(`{}`, nil)
	applyACPSpeedOption(context.Background(), request, "hermes", discardLogger(),
		"ses-1", json.RawMessage(codexACPFastModeSessionResult), "priority", true)
	if len(*calls) != 0 {
		t.Errorf("calls = %+v, want none for an unadvertised token", *calls)
	}
}

func TestACPJSONValueAcceptsBooleanWithoutFailingSiblings(t *testing.T) {
	t.Parallel()
	raw := json.RawMessage(`{"configOptions":[` +
		`{"id":"model","category":"model","currentValue":"kimi-code/k3","options":[{"value":"kimi-code/k3","name":"K3"}]},` +
		`{"id":"fast_mode","category":"model_config","type":"boolean","currentValue":false}` +
		`]}`)
	models := parseACPConfigOptionModels(raw)
	if len(models) != 1 || models[0].ID != "kimi-code/k3" {
		t.Fatalf("boolean sibling hid the model catalog: %+v", models)
	}

	effortRaw := json.RawMessage(`{"configOptions":[` +
		`{"id":"effort","category":"thought_level","currentValue":"low","options":[{"value":"low","name":"Low"}]},` +
		`{"id":"fast_mode","type":"boolean","currentValue":false}` +
		`]}`)
	effort, ok := parseACPEffortOption(effortRaw)
	if !ok || effort.ConfigID != "effort" || effort.CurrentValue != "low" {
		t.Fatalf("boolean sibling hid the effort option: ok=%v option=%+v", ok, effort)
	}
}

func TestACPClientCapabilitiesAdvertisesBoolean(t *testing.T) {
	t.Parallel()
	caps := acpClientCapabilities(map[string]any{"terminal": true})
	if caps["terminal"] != true {
		t.Errorf("terminal = %v, want true", caps["terminal"])
	}
	session, _ := caps["session"].(map[string]any)
	options, _ := session["configOptions"].(map[string]any)
	if _, ok := options["boolean"]; !ok {
		t.Errorf("session.configOptions.boolean missing: %+v", caps)
	}
}
