package channel

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestHubCatalogsHaveCompleteMatchingCopy(t *testing.T) {
	for _, locale := range []string{"en", "zh-Hans", "ja", "ko"} {
		raw, err := channelLocales.ReadFile("locales/" + locale + ".json")
		if err != nil {
			t.Fatal(err)
		}
		var catalog struct {
			Hub map[string]string `json:"hub"`
		}
		if err := json.Unmarshal(raw, &catalog); err != nil {
			t.Fatal(err)
		}
		if len(catalog.Hub) != 7 {
			t.Fatalf("%s has an incomplete Hub catalog", locale)
		}
		for _, key := range []string{"available", "empty", "no_available", "not_found", "switched", "current", "switch_help"} {
			if catalog.Hub[key] == "" {
				t.Fatalf("%s is missing %s", locale, key)
			}
		}
		if strings.Count(catalog.Hub["not_found"], "%s") != 1 || strings.Count(catalog.Hub["switched"], "%s") != 1 {
			t.Fatalf("%s changed the Hub interpolation contract", locale)
		}
		if HubCopyForLocale(locale).Available != catalog.Hub["available"] {
			t.Fatalf("%s is not reachable from the linked member's language preference", locale)
		}
	}
	if HubCopyForLocale("zh-TW") != HubCopyForLocale("zh-Hans") || HubCopyForLocale("ja-JP") != HubCopyForLocale("ja") || HubCopyForLocale("unknown") != HubCopyForLocale("") {
		t.Fatal("language variants or English fallback are inconsistent")
	}
}
