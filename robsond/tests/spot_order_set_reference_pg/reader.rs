use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const PAGE_SIZE: usize = 2;
// Migration 37 guarantees at most one Spot attempt per venue/account/client ID.
const PAGE: &str = r#"
SELECT i.intent_id, i.kind, i.client_order_id, i.not_sent, i.venue_outcome, i.venue_evidence,
        a.attempt_id, a.state AS attempt_state, a.rejected_at AS attempt_rejected_at,
        i.not_sent AND i.venue_outcome IS NULL AND i.venue_evidence IS NULL
          AND n.intent_id IS NULL AS no_send,
        (i.not_sent AND (i.venue_outcome IS NOT NULL OR i.venue_evidence IS NOT NULL
                       OR n.intent_id IS NOT NULL))
         OR (n.intent_id IS NOT NULL AND NOT identity_match.named_identity_matches)
         AS named_conflict,
       i.disposition_kind IS NOT NULL AS has_disposition,
        identity_match.named_identity_matches,
        n.exchange_order_id AS named_id, n.phase AS named_phase, n.status AS named_status,
        n.kind AS named_kind, n.side AS named_side, n.terminal AS named_terminal,
        i.cancels_order_id,
       CASE WHEN ledger_canonical_id(i.cancels_order_id)
            THEN i.cancels_order_id END AS original_target,
        t.exchange_order_id AS target_id, t.phase AS target_phase, t.status AS target_status,
       t.exchange_order_id IS NOT NULL AND
          NOT COALESCE(t.tenant_id=$1 AND t.position_id=$4 AND t.owner='leg'
            AND tj.tenant_id=t.tenant_id AND tj.venue_id=t.venue_id
            AND tj.account_scope=t.account_scope AND tj.position_id=t.position_id
            AND tj.client_order_id=t.client_order_id AND NOT tj.not_sent
            AND t.kind=CASE tj.kind WHEN 'entry' THEN 'entry'
                                    WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END,
            false)
         AS target_conflict
FROM order_intents i
LEFT JOIN venue_orders n ON n.intent_id=i.intent_id
LEFT JOIN submit_attempts a ON a.venue_id='binance:spot' AND a.venue_id=i.venue_id
  AND a.account_scope=i.account_scope AND a.client_order_id=i.client_order_id
CROSS JOIN LATERAL (
    SELECT COALESCE(n.tenant_id=$1 AND n.venue_id=$2 AND n.account_scope=$3 AND n.symbol=$5
             AND n.position_id=$4 AND n.owner='leg' AND n.client_order_id=i.client_order_id
             AND n.kind=CASE i.kind WHEN 'entry' THEN 'entry'
                                    WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END,
           false) AS named_identity_matches
) identity_match
 LEFT JOIN venue_orders t ON i.kind='cancel'
   AND t.venue_id=$2 AND t.account_scope=$3 AND t.symbol=$5
   AND t.exchange_order_id=i.cancels_order_id
 LEFT JOIN order_intents tj ON tj.intent_id=t.intent_id
WHERE i.tenant_id=$1 AND i.venue_id=$2 AND i.account_scope=$3 AND i.position_id=$4
  AND ($6::uuid IS NULL OR i.intent_id>$6)
ORDER BY i.intent_id LIMIT 2
"#;

// The journal-driven page cannot see a venue row whose linkage is absent or
// outside its scope. Read the inverse side under the same snapshot, including
// historical rows that disagree with their generated terminal flag.
const VENUE_PAGE: &str = r#"
 SELECT o.exchange_order_id, o.tenant_id, o.venue_id, o.account_scope, o.symbol,
        o.client_order_id, o.owner, o.position_id, o.intent_id, o.kind,
        o.side, o.phase, o.status, o.terminal,
        j.intent_id AS journal_intent_id, j.not_sent AS journal_not_sent,
        COALESCE(o.owner='leg' AND j.tenant_id=o.tenant_id AND j.venue_id=o.venue_id
          AND j.account_scope=o.account_scope AND j.position_id=o.position_id
          AND j.client_order_id=o.client_order_id
          AND o.kind=CASE j.kind WHEN 'entry' THEN 'entry'
                                WHEN 'protection' THEN 'stop' WHEN 'exit' THEN 'sell' END,
          false) AS journal_identity_matches
 FROM venue_orders o
 LEFT JOIN order_intents j ON j.intent_id=o.intent_id
 WHERE o.tenant_id=$1 AND o.venue_id=$2 AND o.account_scope=$3 AND o.symbol=$5
   AND (o.position_id=$4 OR o.position_id IS NULL)
   AND ($6::text IS NULL OR o.exchange_order_id COLLATE "C">$6)
 ORDER BY o.exchange_order_id COLLATE "C" LIMIT 2
 "#;

// No position or symbol is inferred for an unbound cancel.
const UNBOUND_CANCEL_PAGE: &str = r#"
 SELECT i.intent_id, i.client_order_id, i.cancels_order_id,
        i.not_sent, i.venue_outcome, i.venue_evidence, n.exchange_order_id AS named_id,
        i.not_sent AND i.venue_outcome IS NULL AND i.venue_evidence IS NULL
          AND n.intent_id IS NULL AS no_send
 FROM order_intents i
 LEFT JOIN venue_orders n ON n.intent_id=i.intent_id
 WHERE i.tenant_id=$1 AND i.venue_id=$2 AND i.account_scope=$3
   AND i.position_id IS NULL AND i.kind='cancel'
   AND ($4::uuid IS NULL OR i.intent_id>$4)
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
    pub(super) client_order_id: String,
    pub(super) not_sent: bool,
    pub(super) venue_outcome: Option<String>,
    pub(super) venue_evidence: Option<Value>,
    pub(super) attempt_id: Option<Uuid>,
    pub(super) attempt_state: Option<String>,
    pub(super) attempt_rejected_at: Option<DateTime<Utc>>,
    pub(super) no_send: bool,
    pub(super) named_conflict: bool,
    pub(super) has_disposition: bool,
    pub(super) named_identity_matches: bool,
    pub(super) named_id: Option<String>,
    pub(super) named_phase: Option<String>,
    pub(super) named_status: Option<String>,
    pub(super) named_kind: Option<String>,
    pub(super) named_side: Option<String>,
    pub(super) named_terminal: Option<bool>,
    pub(super) cancels_order_id: Option<String>,
    pub(super) original_target: Option<String>,
    pub(super) target_id: Option<String>,
    pub(super) target_phase: Option<String>,
    pub(super) target_status: Option<String>,
    pub(super) target_conflict: bool,
}

impl Row {
    /// Matches the durable rejection record for one Spot protection or exit.
    pub(super) fn has_durable_rejection(&self) -> bool {
        matches!(self.kind.as_str(), "protection" | "exit")
            && self.venue_outcome.as_deref() == Some("rejected")
            && self.attempt_id.is_some()
            && self.attempt_state.as_deref() == Some("rejected")
            && self.attempt_rejected_at.is_some()
            && !self.not_sent
            && self.venue_evidence.is_none()
            && self.named_id.is_none()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum VenueLinkage {
    Linked,
    MissingJournal,
    JournalConflict,
    RouteUnowned,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct VenueRow {
    pub(super) exchange_order_id: String,
    pub(super) tenant_id: Uuid,
    pub(super) venue_id: String,
    pub(super) account_scope: String,
    pub(super) symbol: String,
    pub(super) client_order_id: String,
    pub(super) owner: String,
    pub(super) position_id: Option<Uuid>,
    pub(super) intent_id: Option<Uuid>,
    pub(super) kind: Option<String>,
    pub(super) side: String,
    pub(super) phase: String,
    pub(super) status: String,
    pub(super) terminal: bool,
    journal_intent_id: Option<Uuid>,
    journal_not_sent: Option<bool>,
    pub(super) journal_identity_matches: bool,
}

impl VenueRow {
    pub(super) fn linkage(&self) -> VenueLinkage {
        let journal_matches = self.journal_identity_matches && self.journal_not_sent == Some(false);
        match (self.position_id, self.journal_intent_id, journal_matches) {
            (None, _, _) => VenueLinkage::RouteUnowned,
            (Some(_), None, _) => VenueLinkage::MissingJournal,
            (Some(_), Some(_), false) => VenueLinkage::JournalConflict,
            (Some(_), Some(_), true) => VenueLinkage::Linked,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct UnboundCancelRow {
    pub(super) intent_id: Uuid,
    pub(super) client_order_id: String,
    pub(super) cancels_order_id: Option<String>,
    pub(super) not_sent: bool,
    pub(super) venue_outcome: Option<String>,
    pub(super) venue_evidence: Option<Value>,
    pub(super) named_id: Option<String>,
    pub(super) no_send: bool,
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
    /// Includes positive no-send rows across the exact account scope.
    pub(super) unbound_cancels: Vec<UnboundCancelRow>,
    pub(super) venue_complement: Vec<VenueRow>,
    pub(super) venue_page_lengths: Vec<usize>,
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
            unbound_cancels: Vec::new(),
            venue_complement: Vec::new(),
            venue_page_lengths: Vec::new(),
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

    async fn collect_venue_complement(&mut self) -> Result<(), Refusal> {
        let mut after: Option<String> = None;
        loop {
            let rows: Vec<VenueRow> = sqlx::query_as(VENUE_PAGE)
                .bind(self.scope.tenant)
                .bind(self.scope.venue)
                .bind(self.scope.account)
                .bind(self.scope.position)
                .bind(self.scope.symbol)
                .bind(after.as_deref())
                .fetch_all(&mut self.conn)
                .await?;
            let mut previous = after.as_deref();
            for row in &rows {
                let id = row.exchange_order_id.as_str();
                if previous.is_some_and(|key| key >= id) {
                    return Err(Refusal::Cursor);
                }
                previous = Some(id);
            }
            let length = rows.len();
            after = rows.last().map(|row| row.exchange_order_id.clone());
            self.report.venue_page_lengths.push(length);
            self.report
                .venue_complement
                .extend(rows.into_iter().filter(|row| row.linkage() != VenueLinkage::Linked));
            if length < PAGE_SIZE {
                return Ok(());
            }
        }
    }

    async fn collect_unbound_cancels(&mut self) -> Result<(), Refusal> {
        let mut after: Option<Uuid> = None;
        loop {
            let rows: Vec<UnboundCancelRow> = sqlx::query_as(UNBOUND_CANCEL_PAGE)
                .bind(self.scope.tenant)
                .bind(self.scope.venue)
                .bind(self.scope.account)
                .bind(after)
                .fetch_all(&mut self.conn)
                .await?;
            for row in &rows {
                if after.is_some_and(|id| id >= row.intent_id) {
                    return Err(Refusal::Cursor);
                }
                after = Some(row.intent_id);
            }
            let length = rows.len();
            self.report.ambiguous_unbound |= rows.iter().any(|row| !row.no_send);
            self.report.unbound_cancels.extend(rows);
            if length < PAGE_SIZE {
                return Ok(());
            }
        }
    }

    pub(super) async fn finish(mut self) -> Result<Report, Refusal> {
        while !self.exhausted() {
            self.step().await?;
        }
        self.collect_venue_complement().await?;
        self.collect_unbound_cancels().await?;
        sqlx::query("COMMIT").execute(&mut self.conn).await?;
        Ok(self.report)
    }
}
