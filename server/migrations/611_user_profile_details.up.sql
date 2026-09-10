ALTER TABLE "user" ADD COLUMN profile_details JSONB NOT NULL DEFAULT '{}'::jsonb;
