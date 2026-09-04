package channel

import (
	"embed"
	"encoding/json"
	"fmt"
	"strings"
)

//go:embed locales/*.json
var channelLocales embed.FS

type HubCopy struct {
	Available   string `json:"available"`
	Empty       string `json:"empty"`
	NoAvailable string `json:"no_available"`
	NotFound    string `json:"not_found"`
	Switched    string `json:"switched"`
	Current     string `json:"current"`
	SwitchHelp  string `json:"switch_help"`
}

type QuotaCopy struct {
	Exceeded    string `json:"exceeded"`
	Unavailable string `json:"unavailable"`
}

var hubCopies = func() map[string]HubCopy {
	copies := make(map[string]HubCopy, 4)
	for _, locale := range []string{"en", "zh-Hans", "ja", "ko"} {
		raw, err := channelLocales.ReadFile("locales/" + locale + ".json")
		if err != nil {
			panic(fmt.Sprintf("load embedded channel locale %s: %v", locale, err))
		}
		var catalog struct {
			Hub HubCopy `json:"hub"`
		}
		if err := json.Unmarshal(raw, &catalog); err != nil {
			panic(fmt.Sprintf("decode embedded channel locale %s: %v", locale, err))
		}
		copies[locale] = catalog.Hub
	}
	return copies
}()

var quotaCopies = func() map[string]QuotaCopy {
	copies := make(map[string]QuotaCopy, 4)
	for _, locale := range []string{"en", "zh-Hans", "ja", "ko"} {
		raw, err := channelLocales.ReadFile("locales/" + locale + ".json")
		if err != nil {
			panic(fmt.Sprintf("load embedded channel locale %s: %v", locale, err))
		}
		var catalog struct {
			Quota QuotaCopy `json:"quota"`
		}
		if err := json.Unmarshal(raw, &catalog); err != nil {
			panic(fmt.Sprintf("decode embedded channel locale %s: %v", locale, err))
		}
		copies[locale] = catalog.Quota
	}
	return copies
}()

// HubCopyForLocale uses the linked member's existing language preference.
// Missing and unknown locales retain the product's English fallback.
func HubCopyForLocale(locale string) HubCopy {
	return hubCopies[normalizedLocale(locale)]
}

func QuotaCopyForLocale(locale string) QuotaCopy {
	return quotaCopies[normalizedLocale(locale)]
}

func normalizedLocale(locale string) string {
	locale = strings.ToLower(strings.TrimSpace(locale))
	for _, supported := range []string{"zh", "ja", "ko"} {
		if locale == supported || strings.HasPrefix(locale, supported+"-") {
			if supported == "zh" {
				return "zh-Hans"
			}
			return supported
		}
	}
	return "en"
}

func QuotaCopyForMessage(message InboundMessage) QuotaCopy {
	var raw map[string]any
	if json.Unmarshal(message.Raw, &raw) == nil {
		for _, key := range []string{"locale", "language", "language_code"} {
			if value, ok := raw[key].(string); ok && value != "" {
				return QuotaCopyForLocale(value)
			}
		}
	}
	return QuotaCopyForLocale(localeFromText(message.Text))
}

func localeFromText(text string) string {
	for _, ch := range text {
		switch {
		case ch >= '\uac00' && ch <= '\ud7af':
			return "ko"
		case ch >= '\u3040' && ch <= '\u30ff', ch >= '\u31f0' && ch <= '\u31ff':
			return "ja"
		case ch >= '\u3400' && ch <= '\u9fff':
			return "zh-Hans"
		}
	}
	return "en"
}
