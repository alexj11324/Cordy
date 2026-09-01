-- Hosted IM usage counts join durable channel messages back to their task.
-- Keep this partial index independent so PostgreSQL can build it concurrently
-- without blocking inbound message writes.
CREATE INDEX CONCURRENTLY idx_chat_message_channel_task
    ON chat_message (task_id)
    WHERE channel_ingested = TRUE AND role = 'user';
