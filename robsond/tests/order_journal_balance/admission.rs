use super::*;

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn supersession_through_j_needs_a_durable_not_sent(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let position = Uuid::now_v7();
    seed_route(&pool, tenant, position).await;
    let journal = journal_on(&pool, tenant, true, true, SPOT);
    let op = journal.begin_operation();
    op.scope(async {
        let a_terms=terms(position);let a=journal.open_balance_entry(IntentSpec {position_id:Some(position),..spec("a",IntentKind::Entry)},a_terms.clone()).unwrap();
        journal.before_send(&a,WriteClass::Strict).await.unwrap();
        let mut b_terms=a_terms.clone();b_terms.decided_month=NaiveDate::from_ymd_opt(2026,12,1).unwrap();
        let blocked=journal.open_balance_entry(IntentSpec {position_id:Some(position),..spec("blocked",IntentKind::Entry)},b_terms.clone()).unwrap();
        assert!(matches!(journal.before_send(&blocked,WriteClass::Strict).await,Err(SendRefusal::NotSent(_))));
        assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT admission_intent_id FROM balance_legs WHERE position_id=$1").bind(position).fetch_one(&pool).await.unwrap(),a.intent_id());
        journal.finish_failed(&a).await;
        assert!(sqlx::query_scalar::<_,bool>("SELECT not_sent FROM order_intents WHERE intent_id=$1").bind(a.intent_id()).fetch_one(&pool).await.unwrap());
        let b=journal.open_balance_entry(IntentSpec {position_id:Some(position),..spec("b",IntentKind::Entry)},b_terms.clone()).unwrap();
        journal.before_send(&b,WriteClass::Strict).await.unwrap();
        let row=sqlx::query("SELECT admission_intent_id,decided_month,cohort_month FROM balance_legs WHERE position_id=$1").bind(position).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<Uuid,_>("admission_intent_id"),b.intent_id());assert_eq!(row.get::<NaiveDate,_>("decided_month"),b_terms.decided_month);assert!(row.get::<Option<NaiveDate>,_>("cohort_month").is_none());
        assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT superseded_by FROM balance_leg_admissions WHERE entry_intent_id=$1").bind(a.intent_id()).fetch_one(&pool).await.unwrap(),b.intent_id());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM balance_leg_admissions WHERE position_id=$1").bind(position).fetch_one(&pool).await.unwrap(),2);
    }).await;
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_cohort_follows_explicit_handoff_only(pool: PgPool) {
    let tenant = Uuid::now_v7();
    seed_account(&pool, tenant, SPOT).await;
    let (proxy, proxied) = through_proxy(&pool).await;
    let journal = journal_on(&proxied, tenant, true, true, SPOT);
    let op = journal.begin_operation();
    let lost_position = Uuid::now_v7();
    seed_route(&pool, tenant, lost_position).await;
    op.scope(async {
        let lost = journal
            .open_balance_entry(
                IntentSpec {
                    position_id: Some(lost_position),
                    ..spec("lost", IntentKind::Entry)
                },
                terms(lost_position),
            )
            .unwrap();
        proxy.set_mode(ProxyMode::DropCommitAckOf("balance_leg_admit"));
        assert!(matches!(
            journal.authorize_send(&lost, WriteClass::Strict).await,
            Err(SendRefusal::UncertainCommit(_))
        ));
        journal.finish_failed(&lost).await;
        assert_eq!(cohort(&pool, lost_position).await, None);
        assert!(sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT uncertain_at FROM order_intents WHERE intent_id=$1"
        )
        .bind(lost.intent_id())
        .fetch_one(&pool)
        .await
        .unwrap()
        .is_some());
    })
    .await;
    // A fresh in-memory gate isolates actual ticket evidence from the lost-ack
    // case.
    let journal = journal_on(&pool, tenant, true, true, SPOT);
    let ledger = SubmitAttemptLedger::one_attempt(32);
    ledger.attach_journal(journal.clone());
    let op = journal.begin_operation();
    op.scope(async {
        for sent in [false, true] {
            let position = Uuid::now_v7();
            seed_route(&pool, tenant, position).await;
            let admission = terms(position);
            let month = admission.decided_month;
            let id = if sent { "sent" } else { "not-sent" };
            let intent = journal
                .open_balance_entry(
                    IntentSpec {
                        position_id: Some(position),
                        ..spec(id, IntentKind::Entry)
                    },
                    admission,
                )
                .unwrap();
            let guard = journal.authorize_send(&intent, WriteClass::Strict).await.unwrap();
            assert_eq!(cohort(&pool, position).await, None);
            if sent {
                ledger
                    .submit(id, |_: &ExecError| false, async { Ok::<(), ExecError>(()) })
                    .await
                    .unwrap();
                journal.finish_success(&intent, None).await;
                assert_eq!(cohort(&pool, position).await, Some(month));
            } else {
                drop(guard);
                journal.finish_failed(&intent).await;
                assert_eq!(cohort(&pool, position).await, None);
                assert!(sqlx::query_scalar::<_, bool>(
                    "SELECT not_sent FROM order_intents WHERE intent_id=$1"
                )
                .bind(intent.intent_id())
                .fetch_one(&pool)
                .await
                .unwrap());
            }
        }
    })
    .await;
}
