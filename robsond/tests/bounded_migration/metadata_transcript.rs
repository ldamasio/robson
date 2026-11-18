//! Test-only physical transcript conservation. NEVER reconstructs Known,
//! protocol ordinals, SQL namespaces, C3 evidence or control authority.
use anyhow::{ensure, Result};
use robson_testkit::db_proxy::{
    DiagnosticDelivery as Delivery, DiagnosticEvent as Event, DiagnosticRecord as Record,
    DiagnosticSnapshot as Snapshot,
};

pub(super) fn physical(d: &Snapshot, pid: i32) -> Result<()> {
    ensure!(
        d.dropped == 0
            && d.connection_overflow == 0
            && d.phase_overflow == 0
            && d.records.len() <= 4096
            && d.connections.len() == 1,
        "metadata transcript loss/overflow/connections"
    );
    let c = d.connections[0];
    ensure!(c.pid == Some(pid) && c.live, "metadata transcript PID/liveness");
    ensure!(
        d.records.iter().all(|r| r.identity.connection == c.connection
            && r.identity.pid.is_none_or(|p| p == pid)
            && r.event != Event::Error),
        "metadata transcript foreign identity/Error"
    );
    Ok(())
}

fn protocol(r: &Record) -> bool {
    matches!(r.event, Event::Complete | Event::Ready)
}

fn same_message(received: Record, mut delivered: Record) -> bool {
    delivered.delivery = received.delivery;
    delivered.micros = received.micros;
    received == delivered
}

pub(super) struct Fence {
    prefix: Vec<Record>,
    connection: u64,
    pid: i32,
}

impl Fence {
    /// None means the last physical response has not yet been forwarded.
    /// Every earlier Complete/Ready must be paired, not just the last one.
    pub(super) fn capture(d: &Snapshot, pid: i32) -> Result<Option<Self>> {
        physical(d, pid)?;
        let mut pending = None;
        let mut last = None;
        for r in d.records.iter().filter(|r| protocol(r)) {
            ensure!(r.identity.pid == Some(pid), "old response lacks actual PID");
            match r.delivery {
                Delivery::Received => {
                    ensure!(pending.is_none(), "old response overlap/unpaired");
                    pending = Some(*r);
                },
                Delivery::Forwarded => {
                    let old = pending
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("old forwarded response unpaired"))?;
                    ensure!(same_message(old, *r), "old response pair changed");
                    last = Some(*r);
                },
                _ => anyhow::bail!("old response Held/Dropped/local"),
            }
        }
        if pending.is_some() {
            return Ok(None);
        }
        ensure!(
            last.is_some_and(|r| r.event == Event::Ready && r.transaction == Some(b'T')),
            "last public DDL52 ReadyT not drained"
        );
        Ok(Some(Self {
            prefix: d.records.clone(),
            connection: d.connections[0].connection,
            pid,
        }))
    }

    pub(super) fn len(&self) -> usize {
        self.prefix.len()
    }

    /// Only a log offset: full historical records must remain BYTE-FIELD
    /// equal. Unknown/AttributionLost are retained, never repaired or promoted.
    pub(super) fn complete(&self, d: &Snapshot) -> Result<bool> {
        physical(d, self.pid)?;
        ensure!(
            d.connections[0].connection == self.connection
                && d.records.len() >= self.prefix.len()
                && d.records[..self.prefix.len()] == self.prefix[..],
            "metadata physical prefix drift/truncation/new connection"
        );
        let expected = [
            (Event::Complete, Delivery::Received, None),
            (Event::Complete, Delivery::Dropped, None),
            (Event::Ready, Delivery::Received, Some(b'T')),
            (Event::Ready, Delivery::Dropped, Some(b'T')),
        ];
        let mut n = 0;
        let mut received = None;
        for r in d.records[self.prefix.len()..].iter().filter(|r| protocol(r)) {
            ensure!(
                n < 4
                    && r.identity.connection == self.connection
                    && r.identity.pid == Some(self.pid)
                    && (r.event, r.delivery, r.transaction) == expected[n],
                "metadata extra/missing/reordered/escaped/nonT response"
            );
            if n % 2 == 0 {
                received = Some(*r);
            } else {
                ensure!(
                    same_message(
                        received
                            .take()
                            .ok_or_else(|| anyhow::anyhow!("metadata received response absent"))?,
                        *r
                    ),
                    "metadata response copy changed"
                );
            }
            n += 1;
        }
        Ok(n == 4)
    }
}

#[cfg(test)]
mod tests {
    use robson_testkit::db_proxy::{DiagnosticIdentity, DiagnosticPhase, SqlCategory};

    use super::*;

    fn record(event: Event, delivery: Delivery, transaction: Option<u8>, micros: u64) -> Record {
        Record {
            micros,
            identity: DiagnosticIdentity {
                connection: 1,
                pid: Some(35),
                journal: None,
                live: true,
                attribution_unknown: true,
            },
            ordinal: None,
            phase: DiagnosticPhase::Unknown,
            category: SqlCategory::Other,
            event,
            delivery,
            sqlstate: None,
            transaction,
        }
    }

    fn prefix() -> Snapshot {
        Snapshot {
            records: vec![
                record(Event::AttributionLost, Delivery::Local, None, 0),
                record(Event::Complete, Delivery::Received, None, 1),
                record(Event::Complete, Delivery::Forwarded, None, 2),
                record(Event::Ready, Delivery::Received, Some(b'T'), 3),
                record(Event::Ready, Delivery::Forwarded, Some(b'T'), 4),
            ],
            connections: vec![record(Event::Connected, Delivery::Local, None, 0).identity],
            dropped: 0,
            connection_overflow: 0,
            phase_overflow: 0,
        }
    }

    fn full() -> (Fence, Snapshot) {
        let mut d = prefix();
        let fence = Fence::capture(&d, 35).unwrap().unwrap();
        d.records.extend([
            record(Event::Sent, Delivery::Local, None, 5),
            record(Event::AttributionLost, Delivery::Local, None, 6),
            record(Event::Complete, Delivery::Received, None, 7),
            record(Event::Complete, Delivery::Dropped, None, 8),
            record(Event::AttributionLost, Delivery::Local, None, 9),
            record(Event::Ready, Delivery::Received, Some(b'T'), 10),
            record(Event::Ready, Delivery::Dropped, Some(b'T'), 11),
        ]);
        (fence, d)
    }

    #[test]
    fn unknown_bind_close_attribution_remains_unknown_without_authority() {
        let (f, d) = full();
        assert!(f.complete(&d).unwrap());
        assert!(d.connections[0].attribution_unknown);
        assert!(d
            .records
            .iter()
            .all(|r| r.ordinal.is_none() && r.phase == DiagnosticPhase::Unknown));
        assert!(d.records.iter().filter(|r| r.event == Event::AttributionLost).count() >= 3);
    }

    #[test]
    fn every_delivery_and_all_negative_physical_cases_fail_closed() {
        let (f, d) = full();
        let relevant: Vec<_> =
            (f.len()..d.records.len()).filter(|&n| protocol(&d.records[n])).collect();
        for &n in &relevant {
            let mut x = d.clone();
            x.records.remove(n);
            assert!(!matches!(f.complete(&x), Ok(true)), "missing delivery{n}");
            let mut x = d.clone();
            x.records.push(x.records[n]);
            assert!(f.complete(&x).is_err(), "extra delivery{n}");
            for bad in [Delivery::Held, Delivery::Forwarded] {
                let mut x = d.clone();
                x.records[n].delivery = bad;
                assert!(f.complete(&x).is_err());
            }
        }
        for ready in [b'I', b'E'] {
            let mut x = d.clone();
            for r in &mut x.records[f.len()..] {
                if r.event == Event::Ready {
                    r.transaction = Some(ready);
                }
            }
            assert!(f.complete(&x).is_err());
        }
        let mut x = d.clone();
        x.records.push(record(Event::Error, Delivery::Received, None, 12));
        assert!(f.complete(&x).is_err());
        let mut x = d.clone();
        x.connections[0].live = false;
        assert!(f.complete(&x).is_err());
        let mut x = d.clone();
        x.connections[0].connection = 2;
        assert!(f.complete(&x).is_err());
        let mut x = d.clone();
        x.connections[0].pid = Some(36);
        assert!(f.complete(&x).is_err());
        let mut x = d.clone();
        x.records[0].micros += 1;
        assert!(f.complete(&x).is_err());
        let mut x = d.clone();
        x.records.truncate(f.len() - 1);
        assert!(f.complete(&x).is_err());
        for field in 0..3 {
            let mut x = d.clone();
            match field {
                0 => x.dropped = 1,
                1 => x.connection_overflow = 1,
                _ => x.phase_overflow = 1,
            }
            assert!(f.complete(&x).is_err());
        }
    }

    #[test]
    fn every_old_response_must_be_drained_before_the_immutable_fence() {
        assert!(Fence::capture(&prefix(), 35).unwrap().is_some());
        let mut x = prefix();
        x.records.pop();
        assert!(Fence::capture(&x, 35).unwrap().is_none());
        for n in [1, 2, 3, 4] {
            for bad in [Delivery::Dropped, Delivery::Held] {
                let mut x = prefix();
                x.records[n].delivery = bad;
                assert!(Fence::capture(&x, 35).is_err());
            }
        }
        let mut x = prefix();
        x.records[2].ordinal = Some(1);
        assert!(Fence::capture(&x, 35).is_err());
        let mut x = prefix();
        x.records.push(record(Event::Error, Delivery::Received, None, 5));
        assert!(Fence::capture(&x, 35).is_err());
    }
}
