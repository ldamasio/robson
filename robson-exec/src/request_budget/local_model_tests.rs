use uuid::Uuid;

use super::*;
use crate::venue_clock::ClockDomain;

fn tick(seconds: i64) -> TickCapture {
    TickCapture::new(ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap(), seconds, 0).unwrap()
}
fn model() -> LocalModel {
    LocalModel::new(tick(0), EpochHistoryCandidate::Comparable { k60: 1, k10: 1 }).unwrap()
}
fn input(weight: u32, orders: u32) -> PendingCandidate {
    PendingCandidate {
        weight,
        orders,
        class: Class::Protective,
        tier: Tier::Expanded,
        deadline: tick(10000),
        attempt: None,
        daily: UnqualifiedDailyCount(0),
    }
}
fn copy_reference(m: &LocalModel, id: u64) -> ModelReference {
    // Private corruption/replay seam; ordinary callers cannot clone references.
    ModelReference { owner: Arc::downgrade(&m.marker), id }
}

#[test]
fn refusal_advances_watermark_before_pruning_and_reversal_is_sticky() {
    for refusal in 0..4 {
        let mut m = model();
        let h = m.insert_pending_candidate(tick(100), input(600, 0)).unwrap();
        m.record_handoff_candidate(h, tick(100)).unwrap();
        let mut next = input(1, 1);
        match refusal {
            0 => next.daily = UnqualifiedDailyCount(10000),
            1 => next.weight = 0,
            2 => next.daily = UnqualifiedDailyCount(u64::MAX),
            _ => m.sequence = u64::MAX,
        }
        assert!(m.insert_pending_candidate(tick(160), next).is_err());
        assert!(m.records.is_empty(), "safe pruning survives a refused insertion");
        assert_eq!(m.observed, tick(160), "refusal watermark must not rewind with charge");
        assert!(matches!(
            m.insert_pending_candidate(tick(159), input(1, 0)),
            Err(ModelError::InvalidTime)
        ));
        assert!(m.invalid_time);
        assert!(matches!(
            m.insert_pending_candidate(tick(200), input(1, 0)),
            Err(ModelError::InvalidTime)
        ));
        assert!(m.records.is_empty());
    }
}

#[test]
fn pending_never_expires_and_failed_handoff_returns_cleanup_reference() {
    let mut m = model();
    let mut i = input(40, 1);
    i.deadline = tick(110);
    let h = m.insert_pending_candidate(tick(100), i).unwrap();
    assert_eq!(m.usage_at(tick(110)).unwrap().weight(), 40);
    let (error, h) = m.record_handoff_candidate(h, tick(110)).unwrap_err();
    assert_eq!(error, ModelError::Deadline);
    assert_eq!(m.usage_at(tick(1000)).unwrap().short_orders(), 1);
    assert_eq!(
        m.cancel_pending_candidate(h).unwrap(),
        RequestCostCandidate::new(40, 1).unwrap()
    );
    let h = m.insert_pending_candidate(tick(1000), input(1, 0)).unwrap();
    drop(h);
    assert_eq!(m.usage_at(tick(2000)).unwrap().weight(), 1, "reference Drop cannot refund");
}

#[test]
fn sent_origin_is_immutable_and_no_outcome_or_replay_refunds_it() {
    let mut m = model();
    let h = m.insert_pending_candidate(tick(100), input(40, 1)).unwrap();
    let id = h.id;
    m.record_handoff_candidate(h, tick(109)).unwrap();
    let replay = copy_reference(&m, id);
    let (error, _) = m.record_handoff_candidate(replay, tick(110)).unwrap_err();
    assert_eq!(error, ModelError::Reference);
    let replay = copy_reference(&m, id);
    assert_eq!(m.cancel_pending_candidate(replay), Err(ModelError::Reference));
    assert_eq!(m.usage_at(tick(118)).unwrap(), UsageFacts { weight: 40, short_orders: 1 });
    assert_eq!(m.usage_at(tick(119)).unwrap(), UsageFacts { weight: 40, short_orders: 0 });
    assert_eq!(m.usage_at(tick(168)).unwrap().weight(), 40);
    assert_eq!(m.usage_at(tick(169)).unwrap().weight(), 0);
    assert!(m.records.is_empty());
    let h = m.insert_pending_candidate(tick(169), input(1, 0)).unwrap();
    assert!(h.id > id, "removed IDs are never reused");
    let stale = copy_reference(&m, id);
    assert_eq!(m.cancel_pending_candidate(stale), Err(ModelError::Reference));
    assert_eq!(m.records.len(), 1);
}

#[test]
fn instance_identity_and_destroyed_owner_cannot_target_matching_ids() {
    let mut a = model();
    let mut b = model();
    let old = a.insert_pending_candidate(tick(100), input(1, 0)).unwrap();
    let current = b.insert_pending_candidate(tick(100), input(2, 0)).unwrap();
    assert_eq!(old.id, current.id);
    assert_eq!(b.cancel_pending_candidate(old), Err(ModelError::Reference));
    assert_eq!(b.usage_at(tick(100)).unwrap().weight(), 2);
    let stale = a.insert_pending_candidate(tick(100), input(3, 0)).unwrap();
    let weak = Arc::downgrade(&a.marker);
    drop(a);
    assert!(weak.upgrade().is_none());
    let mut replacement = model();
    replacement.sequence = stale.id - 1;
    let live = replacement.insert_pending_candidate(tick(100), input(4, 0)).unwrap();
    assert_eq!(live.id, stale.id);
    assert_eq!(replacement.cancel_pending_candidate(stale), Err(ModelError::Reference));
    assert_eq!(replacement.usage_at(tick(100)).unwrap().weight(), 4);
}

#[test]
fn time_failure_preserves_charge_and_time_free_cancellation() {
    let mut m = model();
    let h = m.insert_pending_candidate(tick(100), input(1, 0)).unwrap();
    let foreign =
        TickCapture::new(ClockDomain::new(Uuid::from_u128(2), 0, 0).unwrap(), 1000, 0).unwrap();
    let (error, h) = m.record_handoff_candidate(h, foreign).unwrap_err();
    assert_eq!(error, ModelError::InvalidTime);
    assert_eq!(m.observed, tick(100));
    assert_eq!(m.records.len(), 1);
    assert!(m.usage_at(tick(1000)).is_err());
    assert_eq!(m.cancel_pending_candidate(h).unwrap(), RequestCostCandidate::new(1, 0).unwrap());
    assert_eq!(m.observed, tick(100));
}

#[test]
fn attempt_deadline_and_startup_edges_reuse_exact_parent_facts() {
    let mut m = model();
    let mut i = input(1, 0);
    i.attempt = Some(tick(100));
    let h = m.insert_pending_candidate(tick(100), i).unwrap();
    m.record_handoff_candidate(h, tick(160)).unwrap();
    let mut i = input(1, 0);
    i.attempt = Some(tick(100));
    let h = m.insert_pending_candidate(tick(160), i).unwrap();
    let (error, h) = m.record_handoff_candidate(h, tick(161)).unwrap_err();
    assert_eq!(error, ModelError::Attempt);
    m.cancel_pending_candidate(h).unwrap();
    let mut i = input(1, 0);
    i.deadline = tick(161);
    assert!(matches!(m.insert_pending_candidate(tick(161), i), Err(ModelError::Deadline)));
    let mut m =
        LocalModel::new(tick(0), EpochHistoryCandidate::Comparable { k60: 2, k10: 2 }).unwrap();
    let h = m.insert_pending_candidate(tick(0), input(100, 1)).unwrap();
    assert!(matches!(
        m.insert_pending_candidate(tick(1), input(1, 0)),
        Err(ModelError::Limits)
    ));
    m.cancel_pending_candidate(h).unwrap();
    assert!(matches!(
        m.insert_pending_candidate(tick(59), input(101, 0)),
        Err(ModelError::Limits)
    ));
    let mut h = Vec::new();
    for _ in 0..20 {
        h.push(m.insert_pending_candidate(tick(60), input(1, 1)).unwrap());
    }
    assert!(matches!(
        m.insert_pending_candidate(tick(60), input(1, 1)),
        Err(ModelError::Limits)
    ));
    let mut unknown = LocalModel::new(tick(0), EpochHistoryCandidate::UnknownPriorHistory).unwrap();
    assert!(matches!(
        unknown.insert_pending_candidate(tick(59), input(1, 0)),
        Err(ModelError::Limits)
    ));
    assert!(unknown.insert_pending_candidate(tick(60), input(1, 0)).is_ok());
}

#[test]
fn bounded_storage_saturation_pruning_sequence_and_allocation() {
    let mut m = model();
    let capacity = m.records.capacity();
    let pointer = m.records.as_ptr();
    for _ in 0..1200 {
        drop(m.insert_pending_candidate(tick(100), input(1, 0)).unwrap());
    }
    assert!(m.insert_pending_candidate(tick(100), input(1, 0)).is_err());
    assert_eq!(m.records.len(), 1200);
    assert_eq!(m.records.capacity(), capacity);
    assert_eq!(m.records.as_ptr(), pointer);
    assert_eq!(m.usage_at(tick(100)).unwrap().weight(), 1200);
    assert_eq!(m.visits, 1200);
    let h = copy_reference(&m, m.records[1199].id);
    m.record_handoff_candidate(h, tick(100)).unwrap();
    assert!(m.visits <= 2400);
    assert_eq!(m.usage_at(tick(160)).unwrap().weight(), 1199);
    assert!(m.insert_pending_candidate(tick(160), input(1, 0)).is_ok());
    m.sequence = u64::MAX;
    let h = copy_reference(&m, m.records[0].id);
    m.cancel_pending_candidate(h).unwrap();
    assert!(matches!(
        m.insert_pending_candidate(tick(160), input(1, 0)),
        Err(ModelError::Capacity)
    ));
    assert_eq!(m.sequence, u64::MAX);
    assert_eq!(m.records.len(), 1199);
    assert!(std::mem::size_of::<ModelReference>() <= 24);
    eprintln!(
        "local model sizes record={} model={} reference={} capacity={}",
        std::mem::size_of::<Record>(),
        std::mem::size_of::<LocalModel>(),
        std::mem::size_of::<ModelReference>(),
        capacity
    );
}

#[test]
fn refused_inputs_preserve_existing_charges_and_each_numerical_tier() {
    for (class, tier, cap) in [
        (Class::Steady, Tier::Normal, 400),
        (Class::Protective, Tier::Normal, 600),
        (Class::Protective, Tier::Expanded, 1200),
    ] {
        let mut m = model();
        let mut i = input(cap, 0);
        i.class = class;
        i.tier = tier;
        let h = m.insert_pending_candidate(tick(100), i).unwrap();
        assert_eq!(m.records[0].class, class);
        assert_eq!(m.records[0].tier, tier);
        let mut next = input(1, 0);
        next.class = class;
        next.tier = tier;
        assert!(matches!(m.insert_pending_candidate(tick(100), next), Err(ModelError::Limits)));
        m.cancel_pending_candidate(h).unwrap();
    }
    let mut m = model();
    drop(m.insert_pending_candidate(tick(100), input(40, 1)).unwrap());
    for which in 0..3 {
        let mut i = input(1, 1);
        match which {
            0 => i.weight = 0,
            1 => i.daily = UnqualifiedDailyCount(u64::MAX),
            _ => i.deadline = tick(100),
        };
        assert!(m.insert_pending_candidate(tick(100), i).is_err());
        assert_eq!(m.usage_at(tick(100)).unwrap(), UsageFacts { weight: 40, short_orders: 1 });
        assert_eq!(m.records.len(), 1);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(128))]
    #[test]
    fn generated_events_match_independent_event_log(events in proptest::collection::vec((0u8..8, 0u8..65, 1u16..300, 0u8..2), 1..100)) {
        // An independent history retains each insertion and explicit phase;
        // numerical counters are recomputed from integer seconds, never kernels.
        struct Expected { weight: u64, orders: u64, sent: Option<i64>, removed: bool }
        let mut expected: Vec<Expected> = Vec::new();
        let mut handles: Vec<Option<ModelReference>> = Vec::new();
        let mut m=model(); let mut now=100_i64; let mut watermark=0_i64; let mut invalid=false;
        for (kind, delta, weight, orders) in events {
            now += i64::from(delta);
            let index=if handles.is_empty(){0}else{usize::from(weight)%handles.len()};
            if kind==6 && !handles.is_empty() {
                if let Some(h)=handles[index].take() { drop(h); }
            } else if kind==5 && !handles.is_empty() {
                if let Some(h)=handles[index].take() {
                    proptest::prop_assert!(m.cancel_pending_candidate(h).is_ok());
                    expected[index].removed=true;
                }
            } else {
                let observed=if kind==7 && watermark>0 {watermark-1} else {now};
                if observed<watermark { invalid=true; }
                if !invalid {watermark=observed;}
                if kind==4 && !handles.is_empty() && handles[index].is_some() {
                    let h=handles[index].take().unwrap();
                    match m.record_handoff_candidate(h,tick(observed.max(0))) {
                        Ok(())=>{proptest::prop_assert!(!invalid);expected[index].sent=Some(observed);},
                        Err((e,h))=>{proptest::prop_assert!(invalid);proptest::prop_assert_eq!(e,ModelError::InvalidTime);handles[index]=Some(h);},
                    }
                } else {
                    let usage=expected.iter().filter(|r|!r.removed).fold((0,0),|(w,o),r| {
                        (w+if r.sent.is_none_or(|h|observed-h<60){r.weight}else{0},
                         o+if r.sent.is_none_or(|h|observed-h<10){r.orders}else{0})
                    });
                    let daily=if kind==3 {10000}else{0};
                    let class=if kind==0 {Class::Steady}else{Class::Protective};
                    let tier=if kind==2 {Tier::Expanded}else{Tier::Normal};
                    let cap=if tier==Tier::Expanded {1200}else if class==Class::Steady {400}else{600};
                    let order_cap=if tier==Tier::Expanded {20}else{10};
                    let fits=!invalid && usage.0+u64::from(weight)<=cap && usage.1+u64::from(orders)<=order_cap && daily+u64::from(orders)<=10000;
                    let mut i=input(u32::from(weight),u32::from(orders));i.class=class;i.tier=tier;i.daily=UnqualifiedDailyCount(daily);
                    let result=m.insert_pending_candidate(tick(observed.max(0)),i);
                    proptest::prop_assert_eq!(result.is_ok(),fits);
                    if let Ok(h)=result {
                        handles.push(Some(h));expected.push(Expected{weight:u64::from(weight),orders:u64::from(orders),sent:None,removed:false});
                    }
                }
            }
            proptest::prop_assert_eq!(m.invalid_time,invalid);
            proptest::prop_assert_eq!(m.observed,tick(watermark));
            proptest::prop_assert!(m.records.len()<=1200 && m.visits<=2400);
            let oracle=expected.iter().filter(|r|!r.removed).fold((0,0),|(w,o),r| {
                (w+if r.sent.is_none_or(|h|watermark-h<60){r.weight}else{0},
                 o+if r.sent.is_none_or(|h|watermark-h<10){r.orders}else{0})
            });
            let actual=m.records.iter().fold((0,0),|(w,o),r| {
                let f=local_usage_candidates(std::slice::from_ref(&r.charge), &m.observed).unwrap();
                (w+f.weight(),o+f.short_orders())
            });
            proptest::prop_assert_eq!(actual,oracle,"final state after every event");
        }
    }
}

#[path = "local_model_allocation_tests.rs"]
mod allocation;
