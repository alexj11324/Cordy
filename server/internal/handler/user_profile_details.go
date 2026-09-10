package handler

import (
	"encoding/json"
	"fmt"
	"net/url"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"
)

var profileUsernamePattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$`)
var profilePhonePattern = regexp.MustCompile(`^\+[1-9][0-9]{1,14}$`)

// Profile role is a personal job function, never an authorization role.
func normalizeProfileDetails(patch map[string]json.RawMessage) (map[string]string, error) {
	limits := map[string]int{"first_name": 100, "last_name": 100, "preferred_name": 100, "username": 64, "role": 64, "phone": 32, "website": 2048, "start_week": 16, "time_format": 16}
	out := make(map[string]string, len(patch))
	for key, raw := range patch {
		limit, allowed := limits[key]
		if !allowed {
			return nil, fmt.Errorf("unknown profile field %q", key)
		}
		var value string
		if string(raw) == "null" || json.Unmarshal(raw, &value) != nil {
			return nil, fmt.Errorf("profile %s must be a string", key)
		}
		value = strings.TrimSpace(value)
		if utf8.RuneCountInString(value) > limit || strings.ContainsFunc(value, unicode.IsControl) {
			return nil, fmt.Errorf("invalid profile %s", key)
		}
		if value != "" {
			switch key {
			case "username":
				if !profileUsernamePattern.MatchString(value) {
					return nil, fmt.Errorf("invalid profile username")
				}
			case "phone":
				if !profilePhonePattern.MatchString(value) {
					return nil, fmt.Errorf("phone must use E.164 format")
				}
			case "website":
				parsed, err := url.Parse(value)
				if err != nil || (parsed.Scheme != "https" && parsed.Scheme != "http") || parsed.Hostname() == "" || parsed.User != nil {
					return nil, fmt.Errorf("website must be an http(s) URL")
				}
			case "role":
				if value != "product-ops" && value != "growth-lead" && value != "customer-ops" {
					return nil, fmt.Errorf("invalid profile role")
				}
			case "start_week":
				if value != "monday" && value != "sunday" && value != "saturday" {
					return nil, fmt.Errorf("invalid start of week")
				}
			case "time_format":
				if value != "12-hour" && value != "24-hour" {
					return nil, fmt.Errorf("invalid time format")
				}
			}
		}
		out[key] = value
	}
	return out, nil
}
