-- Rename the team feature's persisted schema and discriminator values.
-- Historical migrations remain immutable; this migration moves databases that
-- already ran them to the names used by the current implementation.

ALTER TABLE squad RENAME TO team;
ALTER TABLE squad_member RENAME TO team_member;
ALTER TABLE team_member RENAME COLUMN squad_id TO team_id;
ALTER TABLE agent_task_queue RENAME COLUMN squad_id TO team_id;
ALTER TABLE autopilot_run RENAME COLUMN squad_id TO team_id;

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_assignee_type_check;
ALTER TABLE autopilot DROP CONSTRAINT IF EXISTS autopilot_assignee_type_check;
ALTER TABLE quick_action DROP CONSTRAINT IF EXISTS quick_action_assignee_type_check;

UPDATE issue SET assignee_type = 'team' WHERE assignee_type = 'squad';
UPDATE autopilot SET assignee_type = 'team' WHERE assignee_type = 'squad';
UPDATE quick_action SET assignee_type = 'team' WHERE assignee_type = 'squad';
UPDATE issue
SET title = replace(replace(title, 'mention://squad/', 'mention://team/'), '@squad', '@team'),
    description = replace(replace(description, 'mention://squad/', 'mention://team/'), '@squad', '@team')
WHERE title LIKE '%mention://squad/%' OR title LIKE '%@squad%'
   OR description LIKE '%mention://squad/%' OR description LIKE '%@squad%';
UPDATE comment
SET content = replace(replace(content, 'mention://squad/', 'mention://team/'), '@squad', '@team')
WHERE content LIKE '%mention://squad/%' OR content LIKE '%@squad%';
UPDATE chat_message
SET content = replace(replace(content, 'mention://squad/', 'mention://team/'), '@squad', '@team')
WHERE content LIKE '%mention://squad/%' OR content LIKE '%@squad%';
UPDATE task_message
SET content = replace(replace(content, 'mention://squad/', 'mention://team/'), '@squad', '@team'),
    output = replace(replace(output, 'mention://squad/', 'mention://team/'), '@squad', '@team')
WHERE content LIKE '%mention://squad/%' OR content LIKE '%@squad%'
   OR output LIKE '%mention://squad/%' OR output LIKE '%@squad%';
UPDATE team
SET description = replace(replace(description, 'mention://squad/', 'mention://team/'), '@squad', '@team'),
    instructions = replace(replace(instructions, 'mention://squad/', 'mention://team/'), '@squad', '@team')
WHERE description LIKE '%mention://squad/%' OR description LIKE '%@squad%'
   OR instructions LIKE '%mention://squad/%' OR instructions LIKE '%@squad%';
UPDATE activity_log
SET details = jsonb_set(details - 'squad_id', '{team_id}', details->'squad_id', true)
WHERE action = 'squad_leader_evaluated' AND details ? 'squad_id';
UPDATE activity_log SET action = 'team_leader_evaluated' WHERE action = 'squad_leader_evaluated';

ALTER TABLE issue ADD CONSTRAINT issue_assignee_type_check
    CHECK (assignee_type IN ('member', 'agent', 'team'));
ALTER TABLE autopilot ADD CONSTRAINT autopilot_assignee_type_check
    CHECK (assignee_type IN ('agent', 'team'));
ALTER TABLE quick_action ADD CONSTRAINT quick_action_assignee_type_check
    CHECK (assignee_type IN ('agent', 'team'));

ALTER INDEX IF EXISTS squad_pkey RENAME TO team_pkey;
ALTER INDEX IF EXISTS idx_squad_workspace RENAME TO idx_team_workspace;
ALTER INDEX IF EXISTS squad_member_pkey RENAME TO team_member_pkey;
ALTER INDEX IF EXISTS idx_squad_member_squad RENAME TO idx_team_member_team;
ALTER INDEX IF EXISTS idx_squad_member_entity RENAME TO idx_team_member_entity;
ALTER INDEX IF EXISTS idx_autopilot_run_squad_id RENAME TO idx_autopilot_run_team_id;
ALTER INDEX IF EXISTS agent_task_queue_squad_id_idx RENAME TO agent_task_queue_team_id_idx;
ALTER INDEX IF EXISTS squad_member_squad_id_member_type_member_id_key
    RENAME TO team_member_team_id_member_type_member_id_key;
