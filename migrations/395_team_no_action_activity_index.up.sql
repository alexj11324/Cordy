CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_activity_log_team_no_action_task
    ON activity_log (issue_id, actor_id, ((details->>'task_id')))
    WHERE actor_type = 'agent'
      AND action = 'team_leader_evaluated'
      AND details->>'outcome' = 'no_action';
