-- Collision-safe slug renames are intentionally irreversible because the
-- previous slug may have been claimed after the forward migration.
SELECT 1;
