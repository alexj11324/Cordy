-- Roll back the Linear origin contract only after Linear-origin Issues have
-- been removed or migrated by the coordinated application rollback.
ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_origin_type_check;
ALTER TABLE issue ADD CONSTRAINT issue_origin_type_check
    CHECK (origin_type IN (
        'automation', 'quick_create', 'lark_chat', 'slack_chat',
        'agent_create', 'dingtalk_chat', 'wecom_chat', 'telegram_chat'
    )) NOT VALID;
ALTER TABLE issue VALIDATE CONSTRAINT issue_origin_type_check;
