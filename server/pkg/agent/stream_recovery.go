package agent

import (
	"fmt"
	"strings"
)

// Stream adapters have no separate resume RPC. Reject competing CLI selectors
// before launch, then verify the session identity before forwarding activity.
func validateStreamRecoveryOptions(opts ExecOptions) error {
	if !opts.RequireResume {
		return nil
	}
	if opts.ResumeSessionID == "" {
		return fmt.Errorf("capacity recovery refused: missing original session")
	}
	for _, args := range [][]string{opts.ExtraArgs, opts.CustomArgs} {
		for _, arg := range args {
			arg = unshellQuoteArg(arg)
			flag, _, _ := strings.Cut(arg, "=")
			switch flag {
			case "--resume", "-r", "--continue", "-c", "--fork-session", "--session-id", "--workspace", "--worktree", "-w", "--bg", "--background", "--cloud", "--teleport":
				return fmt.Errorf("capacity recovery refused: custom %s can change the original session or working directory", flag)
			}
		}
	}
	return nil
}

func streamRecoverySessionError(opts ExecOptions, sessionID string, needsIdentity bool) string {
	if !opts.RequireResume || (sessionID == "" && !needsIdentity) {
		return ""
	}
	if sessionID == "" {
		return "capacity recovery refused: original session identity was not confirmed"
	}
	if sessionID != opts.ResumeSessionID {
		return "capacity recovery refused: runtime switched away from the original session"
	}
	return ""
}
