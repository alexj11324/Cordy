ALTER TABLE task_message
    ADD COLUMN sources JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN citations JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE chat_message
    ADD COLUMN sources JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN citations JSONB NOT NULL DEFAULT '[]'::jsonb;
