-- Linear imports use the provider Issue UUID as a stable origin identity.
-- Keep this contract explicit so a retry can recover the local row without
-- guessing by title or silently creating a duplicate.
ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_origin_type_check;
ALTER TABLE issue ADD CONSTRAINT issue_origin_type_check
    CHECK (origin_type IN (
        'automation', 'quick_create', 'lark_chat', 'slack_chat',
        'agent_create', 'dingtalk_chat', 'wecom_chat', 'telegram_chat', 'linear'
    )) NOT VALID;
ALTER TABLE issue VALIDATE CONSTRAINT issue_origin_type_check;
