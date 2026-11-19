use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const PAGE_SIZE: usize = 2;
const PAGE: &str = r#"
SELECT i.intent_id, i.kind,
       i.not_sent AND i.venue_outcome IS NULL AND n.intent_id IS NULL AS no_send,
       (i.not_sent AND (i.venue_outcome IS NOT NULL OR n.intent_id IS NOT NULL))
         OR (n.intent_id IS NOT NULL AND NOT COALESCE(
              n.tenant_id=$1 AND n.venue_id=$2 AND n.account_scope=$3 AND n.symbol=$5
              AND n.position_id=$4 AND n.owner='leg' AND n.client_order_id=i.client_order_id,
              false)) AS named_conflict,
       i.disposition_kind IS NOT NULL AS has_disposition,
       n.exchange_order_id AS named_id, n.phase AS named_phase,
       CASE WHEN n.status IN ('NEW','PENDING_NEW','PARTIALLY_FILLED',
             'FILLED','CANCELED','EXPIRED','EXPIRED_IN_MATCH','REJECTED')
            THEN n.status END AS named_status,
       CASE WHEN ledger_canonical_id(i.cancels_order_id)
            THEN i.cancels_order_id END AS original_target,
       t.exchange_order_id AS target_id, t.phase AS target_phase,
       CASE WHEN t.status IN ('NEW','PENDING_NEW','PARTIALLY_FILLED',
             'FILLED','CANCELED','EXPIRED','EXPIRED_IN_MATCH','REJECTED')
            THEN t.status END AS target_status,
       t.exchange_order_id IS NOT NULL AND
         NOT COALESCE(t.tenant_id=$1 AND t.position_id=$4 AND t.owner='leg',false)
         AS target_conflict
FROM order_intents i
LEFT JOIN venue_orders n ON n.intent_id=i.intent_id
LEFT JOIN LATERAL (
    SELECT target_row.exchange_order_id,target_row.tenant_id,target_row.position_id,
           target_row.owner,target_row.phase,target_row.status
    FROM venue_orders target_row
    WHERE i.kind='cancel'
      AND target_row.venue_id=$2 AND target_row.account_scope=$3 AND target_row.symbol=$5
      AND target_row.exchange_order_id = i.cancels_order_id
    ORDER BY target_row.exchange_order_id LIMIT 1
) t ON true
WHERE i.tenant_id=$1 AND i.venue_id=$2 AND i.account_scope=$3 AND i.position_id=$4
  AND ($6::uuid IS NULL OR i.intent_id>$6)
ORDER BY i.intent_id LIMIT 2
"#;

#[derive(Clone, Copy)]
pub(super) struct Scope {
    pub(super) tenant: Uuid,
    pub(super) venue: &'static str,
    pub(super) account: &'static str,
    pub(super) position: Uuid,
    pub(super) symbol: &'static str,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct Row {
    pub(super) intent_id: Uuid,
    pub(super) kind: String,
    pub(super) no_send: bool,
    pub(super) named_conflict: bool,
    pub(super) has_disposition: bool,
    pub(super) named_id: Option<String>,
    pub(super) named_phase: Option<String>,
    pub(super) named_status: Option<String>,
    pub(super) original_target: Option<String>,
    pub(super) target_id: Option<String>,
    pub(super) target_phase: Option<String>,
    pub(super) target_status: Option<String>,
    pub(super) target_conflict: bool,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum Refusal {
    #[error("no matching admitted route")]
    Route,
    #[error("admission has no exact journal entry")]
    Admission,
    #[error("journal disagrees with the admitted route")]
    JournalRoute,
    #[error("journal cursor did not advance")]
    Cursor,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug)]
pub(super) struct Report {
    pub(super) tenant: Uuid,
    pub(super) version: i64,
    pub(super) epoch: Option<Uuid>,
    pub(super) rows: Vec<Row>,
    pub(super) page_lengths: Vec<usize>,
    pub(super) ambiguous_unbound: bool,
}

pub(super) async fn begin(pool: &PgPool) -> Result<PgConnection, sqlx::Error> {
    let mut conn = PgConnection::connect_with(&pool.connect_options()).await?;
    sqlx::query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut conn)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '1s'").execute(&mut conn).await?;
    Ok(conn)
}

pub(super) async fn page(
    conn: &mut PgConnection,
    scope: Scope,
    after: Option<Uuid>,
) -> Result<Vec<Row>, Refusal> {
    let rows: Vec<Row> = sqlx::query_as(PAGE)
        .bind(scope.tenant)
        .bind(scope.venue)
        .bind(scope.account)
        .bind(scope.position)
        .bind(scope.symbol)
        .bind(after)
        .fetch_all(conn)
        .await?;
    let mut prev = after;
    for row in &rows {
        if prev.is_some_and(|id| id >= row.intent_id) {
            return Err(Refusal::Cursor);
        }
        prev = Some(row.intent_id);
    }
    Ok(rows)
}

pub(super) async fn explain(
    conn: &mut PgConnection,
    scope: Scope,
) -> Result<Vec<String>, sqlx::Error> {
    let query = format!("EXPLAIN (COSTS OFF) {PAGE}");
    sqlx::query_scalar(&query)
        .bind(scope.tenant)
        .bind(scope.venue)
        .bind(scope.account)
        .bind(scope.position)
        .bind(scope.symbol)
        .bind(None::<Uuid>)
        .fetch_all(conn)
        .await
}

pub(super) struct Snapshot {
    conn: PgConnection,
    scope: Scope,
    after: Option<Uuid>,
    report: Report,
}

impl Snapshot {
    pub(super) async fn open(pool: &PgPool, scope: Scope) -> Result<Self, Refusal> {
        let mut conn = begin(pool).await?;
        let control: Option<(i64, Option<Uuid>, bool, bool)> = sqlx::query_as(
            "SELECT a.version,
                    (SELECT e.epoch_id FROM process_epochs e WHERE e.tenant_id=$1
                     ORDER BY e.epoch_seq DESC LIMIT 1),
                    EXISTS(SELECT 1 FROM order_intents j
                           WHERE j.intent_id=l.admission_intent_id AND j.kind='entry'
                             AND j.tenant_id=$1 AND j.venue_id=$2 AND j.account_scope=$3
                             AND j.position_id=$4),
                    EXISTS(SELECT 1 FROM order_intents j WHERE j.position_id=$4
                             AND (j.tenant_id<>$1 OR j.venue_id<>$2 OR j.account_scope<>$3))
             FROM balance_legs l
             JOIN accounting_state a ON a.tenant_id=l.tenant_id AND a.venue_id=l.venue_id
             WHERE l.tenant_id=$1 AND l.venue_id=$2 AND l.account_scope=$3
               AND l.position_id=$4 AND l.symbol=$5",
        )
        .bind(scope.tenant)
        .bind(scope.venue)
        .bind(scope.account)
        .bind(scope.position)
        .bind(scope.symbol)
        .fetch_optional(&mut conn)
        .await?;
        let (version, epoch, admission, conflict) = control.ok_or(Refusal::Route)?;
        if !admission {
            return Err(Refusal::Admission);
        }
        if conflict {
            return Err(Refusal::JournalRoute);
        }
        let report = Report {
            tenant: scope.tenant,
            version,
            epoch,
            rows: Vec::new(),
            page_lengths: Vec::new(),
            ambiguous_unbound: false,
        };
        Ok(Self { conn, scope, after: None, report })
    }

    /// This is available only in the test target for deterministic
    /// interleaving.
    pub(super) async fn step(&mut self) -> Result<(), Refusal> {
        if self.exhausted() {
            return Ok(());
        }
        let rows = page(&mut self.conn, self.scope, self.after).await?;
        if let Some(last) = rows.last() {
            self.after = Some(last.intent_id);
        }
        self.report.page_lengths.push(rows.len());
        self.report.rows.extend(rows);
        Ok(())
    }

    fn exhausted(&self) -> bool {
        self.report.page_lengths.last().is_some_and(|&n| n < PAGE_SIZE)
    }

    pub(super) async fn finish(mut self) -> Result<Report, Refusal> {
        while !self.exhausted() {
            self.step().await?;
        }
        self.report.ambiguous_unbound = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM order_intents i
             WHERE i.tenant_id=$1 AND i.venue_id=$2 AND i.account_scope=$3
               AND i.position_id IS NULL AND i.kind='cancel'
               AND (NOT i.not_sent OR i.venue_outcome IS NOT NULL
                    OR EXISTS(SELECT 1 FROM venue_orders o WHERE o.intent_id=i.intent_id)))",
        )
        .bind(self.scope.tenant)
        .bind(self.scope.venue)
        .bind(self.scope.account)
        .fetch_one(&mut self.conn)
        .await?;
        sqlx::query("COMMIT").execute(&mut self.conn).await?;
        Ok(self.report)
    }
}
