-- Defense in depth for issueroles.WorkflowGate's active-executor invariant.
-- Direct SQL, imports and worker writes must not create active unassigned work.
-- Actor existence/permissions and review handoff remain application-owned.
-- No foreign keys or automatic reassignment/backfill are introduced.

CREATE FUNCTION enforce_issue_active_executor()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    category TEXT;
BEGIN
    IF NEW.status IN ('backlog', 'todo', 'in_progress', 'in_review', 'done', 'blocked', 'cancelled') THEN
        category := NEW.status;
    ELSE
        -- Hold the catalog row through commit. A concurrent catalog edit/delete
        -- cannot change the meaning of a status after this check succeeds.
        SELECT s.category INTO category
        FROM issue_status s
        WHERE s.workspace_id = NEW.workspace_id AND s.key = NEW.status
        FOR SHARE;
        IF NOT FOUND THEN
            RAISE EXCEPTION USING ERRCODE = '23514',
                CONSTRAINT = 'issue_status_known',
                MESSAGE = 'issue status must exist in its workspace catalog';
        END IF;
    END IF;

    IF category IN ('in_progress', 'in_review', 'blocked')
       AND (NEW.executor_type IS NULL OR NEW.executor_id IS NULL
            OR NEW.executor_type NOT IN ('agent', 'team')) THEN
        RAISE EXCEPTION USING ERRCODE = '23514',
            CONSTRAINT = 'issue_active_executor_required',
            MESSAGE = 'in_progress, in_review, and blocked issues require an executor';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER trg_issue_active_executor
    BEFORE INSERT OR UPDATE ON issue
    FOR EACH ROW EXECUTE FUNCTION enforce_issue_active_executor();

CREATE FUNCTION enforce_issue_status_executor_category()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- Key/category are already immutable in UpdateIssueStatusRequest. Enforce
    -- that contract here too, so SQL cannot bypass the issue guard by changing
    -- the meaning or workspace of an existing catalog entry.
    IF TG_OP = 'UPDATE' THEN
        IF NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
           OR NEW.key IS DISTINCT FROM OLD.key
           OR NEW.category IS DISTINCT FROM OLD.category THEN
            RAISE EXCEPTION USING ERRCODE = '23514',
                CONSTRAINT = 'issue_status_identity_immutable',
                MESSAGE = 'issue status workspace, key, and category are immutable';
        END IF;
    END IF;

    -- A catalog entry may repair an old, unknown status. Refuse to turn legacy
    -- unassigned rows into active work as a side effect of that repair.
    IF NEW.category IN ('in_progress', 'in_review', 'blocked')
       AND NEW.key NOT IN ('backlog', 'todo', 'in_progress', 'in_review', 'done', 'blocked', 'cancelled')
       AND EXISTS (
           SELECT 1 FROM issue i
           WHERE i.workspace_id = NEW.workspace_id AND i.status = NEW.key
             AND (i.executor_type IS NULL OR i.executor_id IS NULL
                  OR i.executor_type NOT IN ('agent', 'team'))
       ) THEN
        RAISE EXCEPTION USING ERRCODE = '23514',
            CONSTRAINT = 'issue_active_executor_required',
            MESSAGE = 'status catalog entry would make unassigned issues active';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER trg_issue_status_executor_category
    BEFORE INSERT OR UPDATE OF workspace_id, key, category ON issue_status
    FOR EACH ROW EXECUTE FUNCTION enforce_issue_status_executor_category();

-- The trigger DDL above locks out issue writes while this migration validates
-- existing rows. Fail visibly instead of silently changing user-owned state.
DO $$
DECLARE invalid_count BIGINT;
BEGIN
    SELECT count(*) INTO invalid_count
    FROM issue i LEFT JOIN issue_status s
      ON s.workspace_id = i.workspace_id AND s.key = i.status
    WHERE (CASE
        WHEN i.status IN ('backlog', 'todo', 'in_progress', 'in_review', 'done', 'blocked', 'cancelled') THEN i.status
        ELSE s.category
    END) IN ('in_progress', 'in_review', 'blocked')
      AND (i.executor_type IS NULL OR i.executor_id IS NULL
           OR i.executor_type NOT IN ('agent', 'team'));
    IF invalid_count > 0 THEN
        RAISE EXCEPTION USING ERRCODE = '23514',
            CONSTRAINT = 'issue_active_executor_required',
            MESSAGE = format('%s active issues lack executors; assign valid executors or explicitly move them to a non-active status before retrying migration 585', invalid_count);
    END IF;
END
$$;
