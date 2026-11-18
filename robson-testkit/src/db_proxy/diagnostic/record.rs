#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqlCategory {
    Other,
    Begin,
    Commit,
    Rollback,
    JournalAccounting,
    LedgerAccounting,
    OrderRead,
    OrderWrite,
    FillRead,
    FillWrite,
    LegAdmission,
    LegWrite,
    JournalIntent,
    JournalAttempt,
    JournalClock,
    Timeouts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Unknown,
    Parse,
    Bind,
    Describe,
    Close,
    Execute,
    Simple,
    Sync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Connected,
    BackendPid,
    /// Frontend request observed locally before write; no physical-send proof.
    Sent,
    Complete,
    Error,
    Ready,
    Skipped,
    Closed,
    Overflow,
    AttributionLost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Local,
    Received,
    Forwarded,
    Held,
    Dropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalLabel {
    pub case: u8,
    pub leg: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    pub connection: u64,
    pub pid: Option<i32>,
    pub journal: Option<JournalLabel>,
    pub live: bool,
    pub attribution_unknown: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub micros: u64,
    pub identity: Identity,
    pub ordinal: Option<u64>,
    pub phase: Phase,
    pub category: SqlCategory,
    pub event: Event,
    pub delivery: Delivery,
    pub sqlstate: Option<[u8; 5]>,
    pub transaction: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub records: Vec<Record>,
    pub connections: Vec<Identity>,
    pub dropped: u64,
    pub connection_overflow: u64,
    pub phase_overflow: u64,
}
