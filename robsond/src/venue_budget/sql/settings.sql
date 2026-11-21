-- B02: no binds; exactly one row/result, rows_affected=1.
SELECT set_config('lock_timeout','300ms',true) AS lock_timeout,
       set_config('statement_timeout','1s',true) AS statement_timeout,
       set_config('idle_in_transaction_session_timeout','1s',true) AS idle_timeout;
