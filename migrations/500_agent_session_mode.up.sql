-- Per-agent protocol session mode (for example Claude Code `auto`,
-- Codex `auto` labelled "Approve for me"). Empty/NULL means full access:
-- keep the daemon's current yolo / bypass default. Distinct from
-- permission_mode, which gates who may invoke the agent.
ALTER TABLE agent ADD COLUMN session_mode TEXT;
