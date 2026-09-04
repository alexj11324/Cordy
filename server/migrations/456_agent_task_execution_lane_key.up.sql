ALTER TABLE agent_task_queue
ADD COLUMN execution_lane_key TEXT GENERATED ALWAYS AS (
    CASE
        WHEN chat_session_id IS NOT NULL THEN
            'chat:' || chat_session_id::text
        WHEN issue_id IS NOT NULL
             AND COALESCE(
                 NULLIF(
                     CASE
                         WHEN jsonb_typeof(context->'side_chat_root_comment_id') = 'string'
                         THEN context->>'side_chat_root_comment_id'
                     END,
                     ''
                 ),
                 NULLIF(
                     CASE
                         WHEN jsonb_typeof(context->'side_chat_parent_task_id') = 'string'
                         THEN context->>'side_chat_parent_task_id'
                     END,
                     ''
                 )
             ) IS NOT NULL THEN
            'issue:' || issue_id::text || ':agent:' || agent_id::text || ':side:' ||
            COALESCE(
                NULLIF(
                    CASE
                        WHEN jsonb_typeof(context->'side_chat_root_comment_id') = 'string'
                        THEN context->>'side_chat_root_comment_id'
                    END,
                    ''
                ),
                NULLIF(
                    CASE
                        WHEN jsonb_typeof(context->'side_chat_parent_task_id') = 'string'
                        THEN context->>'side_chat_parent_task_id'
                    END,
                    ''
                )
            )
        WHEN issue_id IS NOT NULL THEN
            'issue:' || issue_id::text || ':agent:' || agent_id::text || ':main'
        ELSE
            'agent:' || agent_id::text || ':default'
    END
) STORED;

ALTER TABLE agent_task_queue
ALTER COLUMN execution_lane_key SET NOT NULL;
