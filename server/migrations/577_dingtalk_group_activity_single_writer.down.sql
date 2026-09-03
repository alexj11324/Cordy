-- Restore the preceding binary's discovery contract on an explicit rollback.
-- The function is owned by immutable migration 386; no route/history is lost.
DO $$
BEGIN
    IF to_regclass('dingtalk_group_route') IS NOT NULL THEN
        DROP TRIGGER IF EXISTS mirror_legacy_dingtalk_group_route_presence ON dingtalk_group_route;
        CREATE TRIGGER mirror_legacy_dingtalk_group_route_presence
            AFTER INSERT OR UPDATE ON dingtalk_group_route
            FOR EACH ROW
            EXECUTE FUNCTION mirror_legacy_dingtalk_group_route_presence();
    END IF;
END;
$$;
