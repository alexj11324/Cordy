package agent

// extractCodexErrorCode accepts both turn/completed.turn and error params.
func extractCodexErrorCode(value any) string {
	object, _ := value.(map[string]any)
	detail, _ := object["error"].(map[string]any)
	if code, ok := detail["codexErrorInfo"].(string); ok {
		return code
	}
	// Preserve an unknown structured variant as well: it must not acquire a
	// retry policy just because its human-readable message mentions capacity.
	if codes, ok := detail["codexErrorInfo"].(map[string]any); ok {
		if len(codes) != 1 {
			return "other"
		}
		for code := range codes {
			return code
		}
	}
	return ""
}

// A failed turn/completed is authoritative over earlier error notifications.
func (c *codexClient) setProviderTurnError(message, code string, terminal bool) {
	c.turnErrorMu.Lock()
	defer c.turnErrorMu.Unlock()
	if terminal || c.turnError == "" {
		c.turnError = message
		c.turnErrorCode = code
	}
}

func (c *codexClient) getTurnErrorCode() string {
	c.turnErrorMu.Lock()
	defer c.turnErrorMu.Unlock()
	return c.turnErrorCode
}
