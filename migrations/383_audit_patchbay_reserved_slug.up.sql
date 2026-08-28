-- The Patchbay brand slug becomes reserved in this release. Rename any
-- pre-existing workspace collision to the lowest available `patchbay-N`
-- value before the application starts enforcing the reservation.
DO $$
DECLARE
    workspace_row RECORD;
    suffix INT;
BEGIN
    FOR workspace_row IN
        SELECT id, slug
        FROM workspace
        WHERE slug = 'patchbay'
        ORDER BY id
    LOOP
        suffix := 1;
        WHILE EXISTS (
            SELECT 1
            FROM workspace
            WHERE slug = workspace_row.slug || '-' || suffix
        ) LOOP
            suffix := suffix + 1;
        END LOOP;

        UPDATE workspace
        SET slug = workspace_row.slug || '-' || suffix
        WHERE id = workspace_row.id;

        RAISE NOTICE 'Renamed workspace % slug from % to %',
            workspace_row.id,
            workspace_row.slug,
            workspace_row.slug || '-' || suffix;
    END LOOP;
END $$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM workspace WHERE slug = 'patchbay') THEN
        RAISE EXCEPTION 'A workspace still owns the reserved Patchbay brand slug';
    END IF;
END $$;
