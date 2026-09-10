ALTER TABLE verification_code ADD COLUMN purpose TEXT NOT NULL DEFAULT 'login', ADD COLUMN requester_user_id UUID;
