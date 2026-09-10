-- Project metadata is stored with the project so reads remain one parent-row
-- lookup and create/update can commit all metadata as one unit. References are
-- validated in the application layer; this intentionally adds no foreign keys
-- or database cascades.
ALTER TABLE project
    ADD COLUMN member_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN label_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN dependency_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN milestones JSONB NOT NULL DEFAULT '[]'::jsonb;

-- Project labels share the existing workspace label catalog. Keep the check
-- explicit so old deployments cannot accidentally persist an unsupported
-- namespace.
ALTER TABLE issue_label
    DROP CONSTRAINT IF EXISTS issue_label_resource_type_check,
    ADD CONSTRAINT issue_label_resource_type_check
        CHECK (resource_type IN ('issue', 'agent', 'skill', 'project'));
