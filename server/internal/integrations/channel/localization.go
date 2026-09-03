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

// HubCopyForLocale uses the linked member's existing language preference.
// Missing and unknown locales retain the product's English fallback.
func HubCopyForLocale(locale string) HubCopy {
	locale = strings.ToLower(strings.TrimSpace(locale))
	for _, supported := range []string{"zh", "ja", "ko"} {
		if locale == supported || strings.HasPrefix(locale, supported+"-") {
			if supported == "zh" {
				return hubCopies["zh-Hans"]
			}
			return hubCopies[supported]
		}
	}
	return hubCopies["en"]
}
