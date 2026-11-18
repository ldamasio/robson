// Real observation Layer plus the existing scripted journal backend. These
// controls count backend method attempts, not native PostgreSQL statements or
// financial-authority acceptance.
#[cfg(any(feature = "postgres", feature = "crash-points"))]
mod derived_retry {
    use robson_exec::{
        gate::{IncidentReason, OperationKind},
        order_journal::{
            IntentKind, IntentSpec, JournalFault, OrderJournal, PendingEntry, StubJournalBackend,
            WriteClass,
        },
    };

    use super::*;

    async fn flush_conflict_and_derived_incident(
        journal: &OrderJournal,
        backend: &CountedBackend,
        old: &Arc<Counts>,
    ) {
        backend.inner.push_fault(JournalFault::IdSpent);
        assert_eq!(journal.flush_pending().await, 0, "failed retry writes no entry");
        assert_eq!(backend.actual.load(Ordering::Relaxed), 3);
        assert_eq!(backend.inner.commits().len(), 3);
        assert_eq!(old.db.load(Ordering::Relaxed), 3, "retry attempt belongs to A");
        assert_eq!(journal.pending_entries(), vec![PendingEntry::Incident(
            IncidentReason::PersistenceFailure
        )]);
        assert!(journal.latch_set());
        assert!(journal.gate().fenced());

        assert_eq!(journal.flush_pending().await, 1, "derived incident lands once");
        assert_eq!(backend.actual.load(Ordering::Relaxed), 4);
        assert_eq!(backend.inner.incident_write_attempts(), 1);
        assert_eq!(old.db.load(Ordering::Relaxed), 4, "derived incident work stays with scan A");
        assert_eq!(old.score.load(Ordering::Relaxed), 4);
        assert!(!old.incomplete.load(Ordering::Relaxed));
        assert_eq!(journal.retry_set_depth(), 0);
        assert!(journal.is_quiescent());
        assert!(journal.latch_set(), "observation never clears the existing latch");
        assert!(journal.gate().fenced(), "persisting the incident is not a release");
        let incidents = backend.inner.incidents();
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].incident.reason, IncidentReason::PersistenceFailure);
        assert_eq!(incidents[0].incident.count, 1);
        assert!(incidents[0].released_by.is_none());
    }

    async fn exercise_retry_origin(inside_new_scan: bool) {
        let probe = crate::operation_observation::probe::Probe::new();
        let (old, new, journal, backend) = async {
            let backend = Arc::new(CountedBackend {
                inner: StubJournalBackend::default(),
                actual: AtomicU64::new(0),
            });
            let journal = Arc::new(OrderJournal::new(
                backend.clone(),
                &robson_exec::venue::VenueId::new("observed-test"),
            ));
            let old = robson_exec::observation::reconciliation(async {
                let counts = current();
                journal
                    .with_permit(OperationKind::MainLoop, async {
                        let intent = journal.open_intent(IntentSpec {
                            position_id: None,
                            kind: IntentKind::Exit,
                            client_order_id: format!("retry-origin-{}", uuid::Uuid::now_v7()),
                            cancels_order_id: None,
                            params: serde_json::Value::Null,
                        });
                        journal.before_send(&intent, WriteClass::Bounded).await.unwrap();
                        backend.inner.push_fault(JournalFault::Known);
                        journal.finish_success(&intent, None).await;
                    })
                    .await
                    .unwrap();
                tracing::Span::current().record("outcome", 1_u64);
                counts
            })
            .await;
            assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
            assert_eq!(old.db.load(Ordering::Relaxed), 2);
            assert_eq!(journal.retry_set_depth(), 1);
            assert!(journal.gate().open_incidents().is_empty());
            assert!(!robson_exec::observation::reconciliation_active());

            let new = if inside_new_scan {
                Some(
                    robson_exec::observation::reconciliation(async {
                        let counts = current();
                        flush_conflict_and_derived_incident(&journal, &backend, &old).await;
                        assert_eq!(counts.db.load(Ordering::Relaxed), 0, "A never moves to B");
                        assert_eq!(counts.score.load(Ordering::Relaxed), 0);
                        assert!(!counts.incomplete.load(Ordering::Relaxed));
                        tracing::Span::current().record("outcome", 1_u64);
                        counts
                    })
                    .await,
                )
            } else {
                flush_conflict_and_derived_incident(&journal, &backend, &old).await;
                None
            };
            (old, new, journal, backend)
        }
        .with_subscriber(probe.dispatch())
        .await;
        // Capture intentionally owns the most recent Counts. Release only that
        // test capture, keeping the real journal/backend alive for drain proof.
        probe.release();
        assert_eq!(Arc::strong_count(&old), 1, "no retained old origin after landing");
        if let Some(new) = new {
            assert_eq!(Arc::strong_count(&new), 1, "no derived work retained by B");
        }
        assert!(journal.is_quiescent());
        assert_eq!(backend.actual.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn late_id_spent_incident_remains_owned_after_scan_return() {
        exercise_retry_origin(false).await;
    }

    #[tokio::test]
    async fn old_id_spent_incident_inside_new_scan_stays_with_old_origin() {
        exercise_retry_origin(true).await;
    }
}
