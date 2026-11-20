-- F01--F07: ONLY cfg(test), fresh sqlx::test-owned isolated database.
-- Canonical driver/target fixture and exclusive test ownership are external
-- preconditions. Database name or supplied bool alone is not such ownership.
-- No production initializer/Ready token is returned by these statements.
-- F01 no binds, command-only completion.
BEGIN ISOLATION LEVEL READ COMMITTED;
