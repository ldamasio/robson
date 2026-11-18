#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod heap_trend {
    use std::{ffi::CStr, fs};

    use robson_exec::order_journal::{OrderJournal, StubJournalBackend};

    use super::*;

    fn sample(label: &str, cycles: usize) -> (usize, usize, usize) {
        // SAFETY: glibc owns the non-null, static NUL-terminated version string;
        // mallinfo2 returns a by-value snapshot and changes no allocator policy.
        let (version, heap) = unsafe {
            (
                CStr::from_ptr(libc::gnu_get_libc_version()).to_string_lossy(),
                libc::mallinfo2(),
            )
        };
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let value = |key: &str| -> usize {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .and_then(|v| v.split_whitespace().next())
                .unwrap()
                .parse()
                .unwrap()
        };
        let fds = fs::read_dir("/proc/self/fd").unwrap().count();
        let tasks = tokio::runtime::Handle::current().metrics().num_alive_tasks();
        let threads = value("Threads:");
        eprintln!("OBSERVATION_HEAP label={label} cycles={cycles} pid={} glibc={} arena={} uordblks={} hblkhd={} fordblks={} keepcost={} rss_kib={} anon_kib={} threads={threads} fds={fds} alive_tasks={tasks}; allocator_route_requires_independent_ELF_inspection=true", std::process::id(),version,heap.arena,heap.uordblks,heap.hblkhd,heap.fordblks,heap.keepcost,value("VmRSS:"),value("RssAnon:"));
        (tasks, threads, fds)
    }

    async fn drained_cycle(cancel: bool) {
        let backend = Arc::new(CountedBackend {
            inner: StubJournalBackend::default(),
            actual: AtomicU64::new(0),
        });
        let journal = Arc::new(OrderJournal::new(
            backend.clone(),
            &robson_exec::venue::VenueId::new("observed-test"),
        ));
        let counts = if cancel {
            let (reached, ready) = tokio::sync::oneshot::channel();
            let worker_journal = journal.clone();
            let task = tokio::spawn(
                robson_exec::observation::reconciliation(async move {
                    let counts = current();
                    worker_journal.record_suspension(suspension());
                    let _ = reached.send(counts);
                    std::future::pending::<()>().await;
                })
                .with_current_subscriber(),
            );
            // A failed handshake still aborts and joins our task before panic.
            let ready = tokio::time::timeout(std::time::Duration::from_secs(3), ready).await;
            task.abort();
            let joined = task.await;
            assert!(joined.unwrap_err().is_cancelled());
            ready.expect("owned cancellation fixture reached queue").unwrap()
        } else {
            robson_exec::observation::reconciliation(async {
                let counts = current();
                journal.record_suspension(suspension());
                tracing::Span::current().record("outcome", 1_u64);
                counts
            })
            .await
        };
        assert_eq!(counts.db.load(Ordering::Relaxed), 0);
        assert_eq!(journal.retry_set_depth(), 2, "actual incident and suspension origins retained");
        assert_eq!(journal.flush_pending().await, 2);
        assert_eq!(backend.actual.load(Ordering::Relaxed), 2);
        assert_eq!(counts.db.load(Ordering::Relaxed), 2);
        assert_eq!(
            counts.incomplete.load(Ordering::Relaxed),
            cancel,
            "cancelled roots remain visibly incomplete"
        );
        assert_eq!(journal.retry_set_depth(), 0);
        assert!(journal.is_quiescent(), "actual journal has no pending/inflight entries");
        assert_eq!(Arc::strong_count(&counts), 1, "landed work retains no root owner");
        // Dropping these real owners precedes every matched-phase sample.
        drop(counts);
        drop(journal);
        drop(backend);
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires isolated live allocator-series qualification and ELF allocator-route inspection"]
    async fn live_allocator_series_after_drained_observation_blocks() {
        async {
            // Workload sizes, never queue/product limits or an invented byte
            // leak threshold. No trim, arena tuning or allocator replacement.
            for cycle in 0..10 {
                drained_cycle(cycle % 2 == 1).await;
            }
            tokio::task::yield_now().await;
            let baseline = sample("warm-drained", 10);
            for block in 1..=3 {
                for cycle in 0..100 {
                    drained_cycle(cycle % 2 == 1).await;
                }
                tokio::task::yield_now().await;
                let current = sample("block-drained", 10 + 100 * block);
                eprintln!("OBSERVATION_HEAP_PROCESS_COUNTS warm={baseline:?} current={current:?}; peers_may_share_process=true isolated_named_run_required_for_trend=true");
            }
        }
        .with_subscriber(tracing_subscriber::registry().with(layer()))
        .await;
        // Heap/RSS raw series and block slopes require independent review.
        // glibc arena/tcache retention is not exact live Rust ownership, and
        // this test cannot establish service-wide or 24-hour leak absence.
    }
}

mod metrics_acceptance {
    use super::*;

    #[derive(Clone)]
    struct Warnings(Arc<AtomicU64>);
    impl<S: Subscriber> Layer<S> for Warnings {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if event.metadata().target() == "robsond::operation_summary"
                && *event.metadata().level() == tracing::Level::WARN
            {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires operation metric acceptance; isolate named run for global baseline evidence"]
    async fn production_layer_renders_fixed_labels_and_actual_outcomes() {
        use robson_exec::order_journal::{OrderJournal, StubJournalBackend};
        initialize_metrics();
        let pending_before = PENDING.get();
        let over_before = OVER.get();
        let outcomes_before: [u64; 3] = std::array::from_fn(|i| {
            OUTCOMES.with_label_values(&[["success", "error", "cancelled"][i]]).get()
        });
        let warnings = Arc::new(AtomicU64::new(0));
        async {
            // Real journal flushes: 14 suspensions + incident reach15 (no
            // alert);16 + incident reach17 (one crossing, not two alerts).
            for (suspensions, outcome) in [(14, 1_u64), (16, 2_u64)] {
                let backend = Arc::new(CountedBackend {
                    inner: StubJournalBackend::default(),
                    actual: AtomicU64::new(0),
                });
                let journal = OrderJournal::new(
                    backend.clone(),
                    &robson_exec::venue::VenueId::new("observed-test"),
                );
                let counts = robson_exec::observation::reconciliation(async {
                    let counts = current();
                    for index in 0..suspensions {
                        let mut row = suspension();
                        row.stream_key = format!("position:metric-{index}");
                        journal.record_suspension(row);
                    }
                    tracing::Span::current().record("outcome", outcome);
                    counts
                })
                .await;
                assert_eq!(journal.flush_pending().await, suspensions + 1);
                assert_eq!(backend.actual.load(Ordering::Relaxed), (suspensions + 1) as u64);
                assert_eq!(counts.db.load(Ordering::Relaxed), (suspensions + 1) as u64);
                assert_eq!(Arc::strong_count(&counts), 1);
                assert!(journal.is_quiescent());
            }
            let (reached, ready) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(
                robson_exec::observation::reconciliation(async move {
                    let _ = reached.send(());
                    std::future::pending::<()>().await;
                })
                .with_current_subscriber(),
            );
            let ready = tokio::time::timeout(std::time::Duration::from_secs(3), ready).await;
            task.abort();
            let joined = task.await;
            assert!(joined.unwrap_err().is_cancelled());
            ready.expect("actual root reached before cancellation").unwrap();
        }
        .with_subscriber(
            tracing_subscriber::registry()
                .with(layer())
                .with(Warnings(Arc::clone(&warnings))),
        )
        .await;
        assert_eq!(warnings.load(Ordering::Relaxed), 1, "one dispatch-local warning above15");
        assert!(OVER.get() > over_before);
        for (i, outcome) in ["success", "error", "cancelled"].iter().enumerate() {
            assert!(OUTCOMES.with_label_values(&[outcome]).get() > outcomes_before[i]);
        }
        let rendered = crate::metrics::render(); // actual production TextEncoder
        for (metric, label, expected) in [
            ("robsond_reconciliation_http_endpoint_total", "endpoint", ENDPOINTS.to_vec()),
            ("robsond_reconciliation_observations_total", "outcome", vec![
                "success",
                "error",
                "cancelled",
            ]),
        ] {
            let actual: std::collections::BTreeSet<_> = rendered
                .lines()
                .filter_map(|line| line.strip_prefix(&format!("{metric}{{{label}=\"")))
                .map(|rest| rest.split('"').next().unwrap())
                .collect();
            assert_eq!(actual, expected.into_iter().collect());
        }
        assert!(rendered
            .lines()
            .any(|line| line.starts_with("robsond_reconciliation_work_pending ")));
        eprintln!("OBSERVATION_METRICS pending_before={pending_before} pending_after={} over_before={over_before} over_after={} own_warning=1; global_deltas_are_lower_bounds_under_peers=true",PENDING.get(),OVER.get());
    }
}
