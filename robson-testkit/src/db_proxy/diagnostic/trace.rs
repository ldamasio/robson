use std::{collections::VecDeque, time::Instant};

use super::{Delivery, Event, Identity, Phase, Record, Snapshot, CONNECTION_LIMIT, RECORD_LIMIT};

pub(in crate::db_proxy) struct Trace {
    started: Instant,
    records: VecDeque<Record>,
    pub(super) connections: Vec<Identity>,
    dropped: u64,
    pub(super) connection_overflow: u64,
    pub(super) phase_overflow: u64,
}

impl Trace {
    pub(in crate::db_proxy) fn new() -> Self {
        Self {
            started: Instant::now(),
            records: VecDeque::with_capacity(RECORD_LIMIT),
            connections: Vec::with_capacity(CONNECTION_LIMIT),
            dropped: 0,
            connection_overflow: 0,
            phase_overflow: 0,
        }
    }

    pub(in crate::db_proxy) fn snapshot(&self) -> Snapshot {
        Snapshot {
            records: self.records.iter().copied().collect(),
            connections: self.connections.clone(),
            dropped: self.dropped,
            connection_overflow: self.connection_overflow,
            phase_overflow: self.phase_overflow,
        }
    }

    pub(in crate::db_proxy) fn identities(&self) -> Vec<Identity> {
        self.connections.clone()
    }

    pub(in crate::db_proxy) fn control_identity(&self, id: u64) -> Option<Identity> {
        if self.dropped != 0 || self.connection_overflow != 0 || self.phase_overflow != 0 {
            return None;
        }
        self.connections.iter().copied().find(|identity| {
            identity.connection == id
                && identity.live
                && !identity.attribution_unknown
                && identity.pid.is_some_and(|pid| pid > 0)
        })
    }

    // Only the serial backend hook asks immediately after recording its Ready.
    // Delivery copies do not add completions; a missing prior Ready is unknown.
    pub(in crate::db_proxy) fn control_ready_ordinal(&self, id: u64) -> Option<u64> {
        self.control_identity(id)?;
        let mut received = self.records.iter().rev().filter(|record| {
            record.identity.connection == id && record.delivery == Delivery::Received
        });
        if received.next()?.event != Event::Ready {
            return None;
        }
        let mut ordinal = None;
        for record in received {
            match record.event {
                Event::Ready => return ordinal,
                Event::Error => return None,
                Event::Complete => {
                    if ordinal.is_some() || !matches!(record.phase, Phase::Simple | Phase::Execute)
                    {
                        return None;
                    }
                    ordinal = Some(record.ordinal?);
                },
                _ => {},
            }
        }
        None
    }

    pub(super) fn push(&mut self, mut record: Record) {
        record.micros = self.started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        if let Some(identity) = self
            .connections
            .iter_mut()
            .find(|identity| identity.connection == record.identity.connection)
            .filter(|_| matches!(record.delivery, Delivery::Local | Delivery::Received))
        {
            // Delivery records keep historical identity, never restore it.
            *identity = record.identity;
        }
        if self.records.len() == RECORD_LIMIT {
            self.records.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.records.push_back(record);
    }
}
