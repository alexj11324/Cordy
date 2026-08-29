-- Restore the historical squad schema and discriminator values.

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_assignee_type_check;
ALTER TABLE autopilot DROP CONSTRAINT IF EXISTS autopilot_assignee_type_check;
ALTER TABLE quick_action DROP CONSTRAINT IF EXISTS quick_action_assignee_type_check;

UPDATE issue SET assignee_type = 'squad' WHERE assignee_type = 'team';
UPDATE autopilot SET assignee_type = 'squad' WHERE assignee_type = 'team';
UPDATE quick_action SET assignee_type = 'squad' WHERE assignee_type = 'team';
UPDATE issue
SET title = replace(replace(title, 'mention://team/', 'mention://squad/'), '@team', '@squad'),
    description = replace(replace(description, 'mention://team/', 'mention://squad/'), '@team', '@squad')
WHERE title LIKE '%mention://team/%' OR title LIKE '%@team%'
   OR description LIKE '%mention://team/%' OR description LIKE '%@team%';
UPDATE comment
SET content = replace(replace(content, 'mention://team/', 'mention://squad/'), '@team', '@squad')
WHERE content LIKE '%mention://team/%' OR content LIKE '%@team%';
UPDATE chat_message
SET content = replace(replace(content, 'mention://team/', 'mention://squad/'), '@team', '@squad')
WHERE content LIKE '%mention://team/%' OR content LIKE '%@team%';
UPDATE task_message
SET content = replace(replace(content, 'mention://team/', 'mention://squad/'), '@team', '@squad'),
    output = replace(replace(output, 'mention://team/', 'mention://squad/'), '@team', '@squad')
WHERE content LIKE '%mention://team/%' OR content LIKE '%@team%'
   OR output LIKE '%mention://team/%' OR output LIKE '%@team%';
UPDATE team
SET description = replace(replace(description, 'mention://team/', 'mention://squad/'), '@team', '@squad'),
    instructions = replace(replace(instructions, 'mention://team/', 'mention://squad/'), '@team', '@squad')
WHERE description LIKE '%mention://team/%' OR description LIKE '%@team%'
   OR instructions LIKE '%mention://team/%' OR instructions LIKE '%@team%';
UPDATE activity_log
SET details = jsonb_set(details - 'team_id', '{squad_id}', details->'team_id', true)
WHERE action = 'team_leader_evaluated' AND details ? 'team_id';
UPDATE activity_log SET action = 'squad_leader_evaluated' WHERE action = 'team_leader_evaluated';

ALTER TABLE issue ADD CONSTRAINT issue_assignee_type_check
    CHECK (assignee_type IN ('member', 'agent', 'squad'));
ALTER TABLE autopilot ADD CONSTRAINT autopilot_assignee_type_check
    CHECK (assignee_type IN ('agent', 'squad'));
ALTER TABLE quick_action ADD CONSTRAINT quick_action_assignee_type_check
    CHECK (assignee_type IN ('agent', 'squad'));

ALTER INDEX IF EXISTS team_pkey RENAME TO squad_pkey;
ALTER INDEX IF EXISTS idx_team_workspace RENAME TO idx_squad_workspace;
ALTER INDEX IF EXISTS team_member_pkey RENAME TO squad_member_pkey;
ALTER INDEX IF EXISTS idx_team_member_team RENAME TO idx_squad_member_squad;
ALTER INDEX IF EXISTS idx_team_member_entity RENAME TO idx_squad_member_entity;
ALTER INDEX IF EXISTS idx_autopilot_run_team_id RENAME TO idx_autopilot_run_squad_id;
ALTER INDEX IF EXISTS agent_task_queue_team_id_idx RENAME TO agent_task_queue_squad_id_idx;
ALTER INDEX IF EXISTS team_member_team_id_member_type_member_id_key
    RENAME TO squad_member_squad_id_member_type_member_id_key;

ALTER TABLE team_member RENAME COLUMN team_id TO squad_id;
ALTER TABLE agent_task_queue RENAME COLUMN team_id TO squad_id;
ALTER TABLE autopilot_run RENAME COLUMN team_id TO squad_id;
ALTER TABLE team_member RENAME TO squad_member;
ALTER TABLE team RENAME TO squad;
