-- A workspace may designate one normal, unarchived agent as its default
-- assistant. The relationship is enforced in the application layer so this
-- migration keeps the workspace/agent tables free of foreign keys and
-- cascading actions.
ALTER TABLE workspace
    ADD COLUMN lead_agent_id UUID;
