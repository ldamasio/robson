-- no-transaction
--
-- ADR-0056 step 8, PR 8e (S8-1): at most one submit attempt per client id on Binance spot, durably (design §3.1;
-- 8c-plan §7, §13 item 2). One-attempt journals refuse a second attempt in memory (order_journal.rs:1770); this
-- index refuses it in the database for every writer. J maps exactly its violation to an id already spent.
-- Built CONCURRENTLY, as migration 35 is, so J's attempt inserts never wait on the build during the deploy's hook;
-- SQLx 0.8.6 runs this file as one simple query outside a transaction, so it holds this one statement. No row
-- matches on a USD-M database. A build that fails leaves an invalid index that the next run's IF NOT EXISTS
-- accepts: a one-attempt journal then refuses to become ready, which checks the full definition; repair is in
-- docs/runbooks/robsond-db-migrations.md.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_submit_attempts_one_per_spot_client_id
    ON submit_attempts (venue_id, account_scope, client_order_id)
    WHERE venue_id = 'binance:spot';
