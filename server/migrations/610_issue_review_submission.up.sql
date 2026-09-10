-- Persist the review handoff atomically with the issue status.
ALTER TABLE issue ADD COLUMN review_submission JSONB;

CREATE FUNCTION enforce_issue_review_submission() RETURNS trigger AS $$
DECLARE
    next_category text;
    previous_category text;
    submission jsonb;
    previous_submission jsonb;
    reviewer_changed boolean := false;
    pr jsonb;
BEGIN
    SELECT category INTO next_category FROM issue_status
      WHERE workspace_id = NEW.workspace_id AND key = NEW.status;
    next_category := COALESCE(next_category, NEW.status);
    IF next_category <> 'in_review' THEN
        IF (TG_OP = 'INSERT' AND NEW.review_submission IS NOT NULL)
           OR (TG_OP = 'UPDATE' AND NEW.review_submission IS DISTINCT FROM OLD.review_submission)
        THEN
            RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_submission_required',
              MESSAGE = 'Submit the review handoff together with the In Review transition';
        END IF;
        RETURN NEW;
    END IF;

    IF TG_OP = 'UPDATE' THEN
        SELECT category INTO previous_category FROM issue_status
          WHERE workspace_id = OLD.workspace_id AND key = OLD.status;
        previous_category := COALESCE(previous_category, OLD.status);
        previous_submission := OLD.review_submission;
        reviewer_changed := NEW.reviewer_type IS DISTINCT FROM OLD.reviewer_type
          OR NEW.reviewer_id IS DISTINCT FROM OLD.reviewer_id
          OR NEW.executor_type IS DISTINCT FROM OLD.executor_type
          OR NEW.executor_id IS DISTINCT FROM OLD.executor_id;
    END IF;

    IF (previous_category IS DISTINCT FROM 'in_review' OR reviewer_changed)
       AND (NEW.reviewer_type IS NULL OR NEW.reviewer_type NOT IN ('member', 'agent', 'team')
            OR NEW.reviewer_id IS NULL
            OR (NEW.reviewer_type = NEW.executor_type AND NEW.reviewer_id = NEW.executor_id))
    THEN
        RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_reviewer_required',
          MESSAGE = 'Review requires a reviewer different from the executor';
    END IF;

    IF TG_OP = 'UPDATE' THEN
        IF previous_category = 'in_review'
           AND NEW.review_submission IS NOT DISTINCT FROM previous_submission
        THEN RETURN NEW; END IF;
        IF previous_category = 'in_review' THEN
            RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_submission_immutable',
              MESSAGE = 'Return to implementation before replacing an active review handoff';
        END IF;
    END IF;

    submission := NEW.review_submission;
    IF jsonb_typeof(submission) IS DISTINCT FROM 'object'
       OR jsonb_typeof(submission->'worktree') IS DISTINCT FROM 'string'
       OR btrim(submission->>'worktree') = ''
       OR jsonb_typeof(submission->'branch') IS DISTINCT FROM 'string'
       OR btrim(submission->>'branch') = ''
       OR jsonb_typeof(submission->'commit') IS DISTINCT FROM 'string'
       OR (submission->>'commit') !~ '^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$'
       OR jsonb_typeof(submission->'pull_requests') IS DISTINCT FROM 'array'
       OR jsonb_typeof(submission->'submission_id') IS DISTINCT FROM 'string'
       OR btrim(submission->>'submission_id') = ''
    THEN
        RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_submission_required',
          MESSAGE = 'Review requires worktree, branch, full commit SHA, and at least one PR in one submission';
    END IF;
    IF jsonb_array_length(submission->'pull_requests') = 0
       OR (previous_category IS DISTINCT FROM 'in_review' AND previous_submission IS NOT NULL
           AND submission->>'submission_id' IS NOT DISTINCT FROM previous_submission->>'submission_id')
    THEN
        RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_submission_required',
          MESSAGE = 'Entering review requires a new complete handoff';
    END IF;
    FOR pr IN SELECT value FROM jsonb_array_elements(submission->'pull_requests') LOOP
        IF jsonb_typeof(pr) IS DISTINCT FROM 'string'
           OR (pr #>> '{}') !~ '^https?://[^[:space:]]+/(pull|pulls|merge_requests)/[0-9]+/?$'
        THEN
            RAISE EXCEPTION USING ERRCODE = '23514', CONSTRAINT = 'issue_review_submission_required',
              MESSAGE = 'Review requires valid PR URLs';
        END IF;
    END LOOP;
    NEW.review_submission := jsonb_set(NEW.review_submission, '{submitted_at}', to_jsonb(now()));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER issue_review_submission_gate
BEFORE INSERT OR UPDATE OF status, review_submission, reviewer_type, reviewer_id, executor_type, executor_id ON issue
FOR EACH ROW EXECUTE FUNCTION enforce_issue_review_submission();
