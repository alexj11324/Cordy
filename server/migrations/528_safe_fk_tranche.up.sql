-- Remove only relationships already covered by explicit application cleanup:
-- project deletion removes project resources, comment deletion handles the
-- complete comment tree, workspace deletion removes every workspace-owned
-- attachment, and issue deletion removes direct, comment, and task attachments.
-- This migration intentionally performs no data changes.
ALTER TABLE project_resource
    DROP CONSTRAINT IF EXISTS project_resource_project_id_fkey;

ALTER TABLE comment
    DROP CONSTRAINT IF EXISTS comment_parent_id_fkey;

ALTER TABLE attachment
    DROP CONSTRAINT IF EXISTS attachment_workspace_id_fkey;

ALTER TABLE attachment
    DROP CONSTRAINT IF EXISTS attachment_issue_id_fkey;

ALTER TABLE attachment
    DROP CONSTRAINT IF EXISTS attachment_comment_id_fkey;
