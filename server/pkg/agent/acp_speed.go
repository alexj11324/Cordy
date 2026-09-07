package agent

import (
	"context"
	"encoding/json"
	"log/slog"
	"strings"
)

// acp_speed.go is the provider-neutral half of ACP speed support: locating a
// speed / Fast / service-tier selector in session/new, turning it into the
// model catalog's service_tiers list, and pushing a persisted value back onto
// a live session.
//
// This mirrors acp_effort.go. The protocol gives us configOptions plus
// session/set_config_option; it does not give a "this option is speed" marker.
// Category `model_config` is placement metadata for "context size or
// speed/quality trade-off", so treating every model_config field as speed
// would put context windows in the speed column. Matching is therefore:
//
//   - id or category against acpSpeedOptionIDs (fast_mode, service_tier, …)
//   - or id/category/name/description containing a speed token (fast, speed,
//     latency, priority, service tier), unless the same blob looks like
//     context size / tokens / temperature
//   - never model, thought_level, or mode
//
// Vocabulary is passed through verbatim. Boolean Fast mode is stored as the
// tokens "true" / "false" so the existing service_tier persist gate can keep
// using isValidDynamicThinkingValue; applyACPSpeedOption sends a real boolean
// on the wire.
//
// Advertising is not evidence the runtime honours the setting. applyACPSpeedOption
// reads the value back the same way effort does. A runtime only reaches the
// persist gate once its Execute opts into this helper (acpCatalogSpeedProviders).

var acpSpeedOptionIDs = map[string]bool{
	"speed":        true,
	"service_tier": true,
	"service-tier": true,
	"fast_mode":    true,
	"fast-mode":    true,
	"fastmode":     true,
	"fast":         true,
}

var acpSpeedMatchTokens = []string{
	"speed",
	"fast",
	"latency",
	"priority",
	"service tier",
	"service_tier",
	"service-tier",
}

var acpSpeedSkipTokens = []string{
	"context",
	"token",
	"window",
	"temperature",
	"top_p",
	"top-p",
	"max_output",
	"max-output",
}

// acpSpeedOption is the speed selector one backend advertised.
type acpSpeedOption struct {
	ConfigID     string
	Type         string
	CurrentValue string
	Choices      []ModelServiceTier
}

func (o acpSpeedOption) supports(value string) bool {
	for _, choice := range o.Choices {
		if choice.ID == value {
			return true
		}
	}
	return false
}

func (o acpSpeedOption) values() []string {
	out := make([]string, 0, len(o.Choices))
	for _, choice := range o.Choices {
		out = append(out, choice.ID)
	}
	return out
}

func (o acpSpeedOption) boolean() bool {
	return strings.EqualFold(strings.TrimSpace(o.Type), "boolean")
}

func looksLikeACPSpeedOption(id, category, name, description string) bool {
	id = strings.ToLower(strings.TrimSpace(id))
	category = strings.ToLower(strings.TrimSpace(category))
	name = strings.ToLower(strings.TrimSpace(name))
	description = strings.ToLower(strings.TrimSpace(description))
	if id == "" {
		return false
	}
	switch category {
	case "model", "thought_level", "mode":
		return false
	}
	blob := strings.Join([]string{id, category, name, description}, " ")
	for _, skip := range acpSpeedSkipTokens {
		if strings.Contains(blob, skip) {
			return false
		}
	}
	if acpSpeedOptionIDs[id] || acpSpeedOptionIDs[category] {
		return true
	}
	for _, token := range acpSpeedMatchTokens {
		if strings.Contains(id, token) ||
			strings.Contains(category, token) ||
			strings.Contains(name, token) ||
			strings.Contains(description, token) {
			return true
		}
	}
	return false
}

// parseACPSpeedOption extracts the first speed-like selector from an ACP
// session/new (or session/resume) result. Reports false when the response
// carries no recognisable option, which every caller treats as "this runtime
// has no speed dial" rather than as an error.
func parseACPSpeedOption(raw json.RawMessage) (acpSpeedOption, bool) {
	type acpChoice struct {
		Value       string `json:"value"`
		Name        string `json:"name"`
		Description string `json:"description"`
	}
	type acpOption struct {
		ID                string       `json:"id"`
		Name              string       `json:"name"`
		Description       string       `json:"description"`
		Category          string       `json:"category"`
		Type              string       `json:"type"`
		CurrentValue      acpJSONValue `json:"currentValue"`
		CurrentValueSnake acpJSONValue `json:"current_value"`
		Options           []acpChoice  `json:"options"`
	}
	var resp struct {
		ConfigOptions      []acpOption `json:"configOptions"`
		ConfigOptionsSnake []acpOption `json:"config_options"`
	}
	if err := json.Unmarshal(raw, &resp); err != nil {
		return acpSpeedOption{}, false
	}
	options := resp.ConfigOptions
	if len(options) == 0 {
		options = resp.ConfigOptionsSnake
	}

	for _, opt := range options {
		id := strings.TrimSpace(opt.ID)
		if id == "" {
			continue
		}
		if !looksLikeACPSpeedOption(id, opt.Category, opt.Name, opt.Description) {
			continue
		}
		result := acpSpeedOption{
			ConfigID: id,
			Type:     strings.TrimSpace(opt.Type),
		}
		seen := map[string]bool{}
		for _, choice := range opt.Options {
			value := strings.TrimSpace(choice.Value)
			if value == "" || seen[value] {
				continue
			}
			seen[value] = true
			label := strings.TrimSpace(choice.Name)
			if label == "" {
				label = strings.Title(value) //nolint:staticcheck
			}
			result.Choices = append(result.Choices, ModelServiceTier{
				ID:          value,
				Name:        label,
				Description: strings.TrimSpace(choice.Description),
			})
		}
		if result.boolean() && len(result.Choices) == 0 {
			onLabel := strings.TrimSpace(opt.Name)
			if onLabel == "" {
				onLabel = "On"
			}
			result.Choices = []ModelServiceTier{
				{ID: "false", Name: "Off"},
				{ID: "true", Name: onLabel, Description: strings.TrimSpace(opt.Description)},
			}
		}
		if len(result.Choices) == 0 {
			continue
		}
		current := strings.TrimSpace(opt.CurrentValue.String())
		if current == "" {
			current = strings.TrimSpace(opt.CurrentValueSnake.String())
		}
		if result.supports(current) {
			result.CurrentValue = current
		}
		return result, true
	}
	return acpSpeedOption{}, false
}

// annotateACPSpeedForSessionModel fills in service_tiers for the model the
// session is currently on, from the same session/new response the models came
// from. Only that model is annotated: ACP options can depend on the current
// model, so copying one model's speed catalog onto its siblings would invent
// tiers the runtime will refuse. See annotateACPThinkingForSessionModel.
func annotateACPSpeedForSessionModel(models []Model, sessionResult json.RawMessage) {
	option, ok := parseACPSpeedOption(sessionResult)
	if !ok || len(option.Choices) == 0 {
		return
	}
	for i := range models {
		if models[i].Default {
			models[i].ServiceTiers = append([]ModelServiceTier(nil), option.Choices...)
		}
	}
}

// annotateACPSessionConfig enriches the catalog from the session-level
// configOptions the shared model parser ignores: effort (thought_level) and
// speed (model_config / fast_mode). Used by ACP-execute discovery paths so a
// runtime that advertises either dial shows it, and a runtime that advertises
// neither (Hermes Agent) shows nothing.
func annotateACPSessionConfig(models []Model, sessionResult json.RawMessage) {
	annotateACPThinkingForSessionModel(models, sessionResult)
	annotateACPSpeedForSessionModel(models, sessionResult)
}

// applyACPSpeedOption pushes a persisted speed value onto a live ACP session
// using the selector that session advertised. A configuration failure never
// blocks the task: the prompt goes out either way. See applyACPEffortOption
// for why the read-back is diagnostics, not proof the runtime honours it.
func applyACPSpeedOption(
	ctx context.Context,
	request acpRequestFn,
	backend string,
	logger *slog.Logger,
	sessionID string,
	sessionResult json.RawMessage,
	value string,
	stateIsCurrent bool,
) {
	if value == "" {
		return
	}
	if logger == nil {
		logger = slog.Default()
	}

	option, ok := parseACPSpeedOption(sessionResult)
	if !ok || len(option.Choices) == 0 {
		logger.Warn("session advertises no speed option; sending the prompt without it",
			"backend", backend,
			"requested_tier", value,
		)
		return
	}
	if stateIsCurrent && !option.supports(value) {
		logger.Warn("session does not advertise the requested speed; sending the prompt without it",
			"backend", backend,
			"config_id", option.ConfigID,
			"requested_tier", value,
			"advertised_tiers", strings.Join(option.values(), ","),
		)
		return
	}

	params := map[string]any{
		"sessionId": sessionID,
		"configId":  option.ConfigID,
		"value":     value,
	}
	if option.boolean() {
		params["type"] = "boolean"
		params["value"] = value == "true"
	}

	result, err := request(ctx, "session/set_config_option", params)
	if err != nil {
		logger.Warn("runtime rejected the speed request; sending the prompt anyway",
			"backend", backend,
			"config_id", option.ConfigID,
			"requested_tier", value,
			"effective_tier", "unchanged",
			"error", err,
		)
		return
	}

	effective, confirmed := acpConfigOptionCurrentValue(result, option.ConfigID)
	if !confirmed || effective != value {
		if !confirmed {
			effective = "unknown"
		}
		logger.Warn("runtime did not confirm the requested speed; sending the prompt anyway",
			"backend", backend,
			"config_id", option.ConfigID,
			"requested_tier", value,
			"effective_tier", effective,
		)
		return
	}
	logger.Info("session speed confirmed",
		"backend", backend,
		"config_id", option.ConfigID,
		"tier", effective,
	)
}
