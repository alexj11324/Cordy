-- The graph is the execution authority for planned issue tasks. Keep this
-- predicate in Postgres so every queue INSERT/claim path, including legacy
-- handlers and retries, shares the same fail-closed invariant.
CREATE OR REPLACE FUNCTION dependency_graph_issue_gate_open(
    p_workspace_id UUID,
    p_issue_id UUID
)
RETURNS BOOLEAN
LANGUAGE SQL
STABLE
AS $$
    SELECT p_workspace_id IS NOT NULL
       AND p_issue_id IS NOT NULL
       AND NOT EXISTS (
           SELECT 1
           FROM dependency_graph_edge edge
           JOIN dependency_graph_plan plan
             ON plan.id = edge.plan_id
            AND plan.workspace_id = edge.workspace_id
            AND plan.status = 'active'
           WHERE edge.workspace_id = p_workspace_id
             AND edge.to_issue_id = p_issue_id
             AND NOT EXISTS (
                 SELECT 1
                 FROM issue prerequisite
                 WHERE prerequisite.id = edge.from_issue_id
                   AND prerequisite.workspace_id = edge.workspace_id
                   AND issue_effective_status(
                       prerequisite.workspace_id,
                       prerequisite.status
                   ) = 'done'
             )
       )
$$;

CREATE OR REPLACE FUNCTION dependency_graph_task_admission_gate()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    issue_workspace_id UUID;
BEGIN
    -- Only admission/active queue states can start work. Historical terminal
    -- rows are allowed to be recorded even when a prerequisite later fails or
    -- the graph is no longer available.
    IF NEW.issue_id IS NULL
       OR NEW.status IS NULL
       OR NEW.status NOT IN (
           'queued',
           'deferred',
           'dispatched',
           'running',
           'waiting_local_directory'
       ) THEN
        RETURN NEW;
    END IF;

    SELECT issue.workspace_id
      INTO issue_workspace_id
      FROM issue
     WHERE issue.id = NEW.issue_id;

    IF NOT dependency_graph_issue_gate_open(issue_workspace_id, NEW.issue_id) THEN
        RAISE EXCEPTION 'dependency gate is closed for issue %', NEW.issue_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_dependency_graph_task_admission ON agent_task_queue;
CREATE TRIGGER trg_dependency_graph_task_admission
    BEFORE INSERT OR UPDATE OF issue_id, status ON agent_task_queue
    FOR EACH ROW
    EXECUTE FUNCTION dependency_graph_task_admission_gate();
