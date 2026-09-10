-- A short project summary is distinct from the longer Markdown description.
-- Keep it nullable so existing projects retain the old empty-summary behavior.
ALTER TABLE project
    ADD COLUMN summary TEXT;
