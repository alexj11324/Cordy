-- Deliberately do not recreate the historical foreign keys. Re-adding either
-- FK would restore database-owned deletion behavior that the application now
-- owns, and could reintroduce cascading deletes during rollback.
SELECT 1;
