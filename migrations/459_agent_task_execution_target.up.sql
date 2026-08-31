ALTER TABLE agent_task_queue
    ADD COLUMN model_id TEXT,
    ADD COLUMN policy_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN failover_reason TEXT;

UPDATE agent_task_queue task
SET model_id = agent.model
FROM agent
WHERE agent.id = task.agent_id
  AND task.model_id IS NULL;

CREATE FUNCTION snapshot_agent_task_execution_target()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.model_id IS NULL THEN
        SELECT agent.model
        INTO NEW.model_id
        FROM agent
        WHERE agent.id = NEW.agent_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER agent_task_execution_target_snapshot
BEFORE INSERT ON agent_task_queue
FOR EACH ROW
EXECUTE FUNCTION snapshot_agent_task_execution_target();
