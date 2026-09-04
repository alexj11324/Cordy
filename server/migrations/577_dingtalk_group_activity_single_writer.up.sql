-- The Go inbound pipeline records activity only after a message commits.
-- Group discovery is routing metadata, not a second activity writer. Keeping
-- the old rollout trigger would count failed messages and double-count success.
DO $$
BEGIN
    IF to_regclass('dingtalk_group_route') IS NOT NULL THEN
        DROP TRIGGER IF EXISTS mirror_legacy_dingtalk_group_route_presence ON dingtalk_group_route;
    END IF;
END;
$$;
