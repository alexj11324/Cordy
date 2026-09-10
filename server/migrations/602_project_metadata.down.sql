-- Project labels have no representation in the pre-metadata schema. Remove
-- that namespace before restoring the older label constraint, then remove the
-- metadata columns.
DELETE FROM issue_label WHERE resource_type = 'project';

ALTER TABLE issue_label
    DROP CONSTRAINT IF EXISTS issue_label_resource_type_check,
    ADD CONSTRAINT issue_label_resource_type_check
        CHECK (resource_type IN ('issue', 'agent', 'skill'));

ALTER TABLE project
    DROP COLUMN IF EXISTS member_ids,
    DROP COLUMN IF EXISTS label_ids,
    DROP COLUMN IF EXISTS dependency_ids,
    DROP COLUMN IF EXISTS milestones;
