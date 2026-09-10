ALTER TABLE chat_message
    DROP COLUMN citations,
    DROP COLUMN sources;

ALTER TABLE task_message
    DROP COLUMN citations,
    DROP COLUMN sources;
