use sqlx::{migrate::MigrateError, PgConnection};

use super::refusal;

const INSTALL: &str = "WITH old AS MATERIALIZED (SELECT current_setting('lock_timeout') AS old_lock, current_setting('statement_timeout') AS old_statement, current_setting('idle_in_transaction_session_timeout') AS old_idle), changed AS MATERIALIZED (SELECT old.*, set_config('lock_timeout','200ms',false) AS new_lock, set_config('statement_timeout','1s',false) AS new_statement, set_config('idle_in_transaction_session_timeout','1s',false) AS new_idle FROM old) SELECT * FROM changed";
const RESTORE: &str = "SELECT set_config('lock_timeout',$1,false), set_config('statement_timeout',$2,false), set_config('idle_in_transaction_session_timeout',$3,false)";

pub(super) struct Settings(String, String, String);

pub(super) async fn install(connection: &mut PgConnection) -> Result<Settings, MigrateError> {
    let (old_lock, old_statement, old_idle, lock, statement, idle): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(INSTALL).fetch_one(connection).await?;
    if (lock.as_str(), statement.as_str(), idle.as_str()) != ("200ms", "1s", "1s") {
        return Err(refusal("migration timeout installation mismatch"));
    }
    Ok(Settings(old_lock, old_statement, old_idle))
}

impl Settings {
    pub(super) async fn restore(self, connection: &mut PgConnection) -> Result<(), MigrateError> {
        let returned: (String, String, String) = sqlx::query_as(RESTORE)
            .bind(&self.0)
            .bind(&self.1)
            .bind(&self.2)
            .fetch_one(connection)
            .await?;
        if returned != (self.0, self.1, self.2) {
            return Err(refusal("migration timeout restoration mismatch"));
        }
        Ok(())
    }
}
