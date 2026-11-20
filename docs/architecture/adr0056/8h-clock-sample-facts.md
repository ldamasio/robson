# C8 clock arithmetic checkpoint

This private, inert module calculates unqualified sample and capture facts. It
has no production caller and constructs neither `ClockEvidence` nor cost-age
authority. It does not implement a sampler, persistence, step latch, watchdog,
request eligibility or startup recovery. USD-M and the SD12 factory are unchanged.

Each supplied wall observation has a before/after BOOTTIME bracket. The existing
complete domain comparison and checked tick subtraction establish numerical
ordering only; callers must later prove that the real observation was bracketed.
For send `[Sb, Sa]` and receive `[Rb, Ra]`, RTT is bounded by
`[Rb - Sa, Ra - Sb]`. Both endpoint discrepancies from the wall span must be at
most 50 ms. Overlapping or reversed brackets refuse.

The signed offset retains doubled nanoseconds:
`theta2 = 2 * server_ns - send_wall_ns - receive_wall_ns`.
The conservative doubled bound is `abs(theta2) + RTT_upper_ns + 200,000,000`.
Compare this exact integer with 2,000,000,000 before rounding; a half-nanosecond
excess refuses. The separate display bound rounds upward to whole nanoseconds.
No floating point, Decimal product or millisecond truncation participates.

A later comparison uses the original receive bracket, the same process UUID
epoch and clock domain, an age upper bound at most 60 s, and both endpoints of
the 50 ms discrepancy check. A bad sample cannot be repaired by a later capture.
Matching coordinates are not authentic evidence. The process-epoch rule is
different from N5's same-domain restart rules and strict 24-hour age limit.

All operations have fixed work and no heap allocation in the inspected source:
zero SQL, HTTP, file/native reads, await, lock, task, timer, cache or history loop.
Sample calculation has eleven explicit checked arithmetic operations plus two
checked elapsed subtractions; comparison has five plus two elapsed subtractions.
Constructing each raw wall mark separately uses two checked operations and one
elapsed subtraction. These are work inventories, not scheduling guarantees.
Measured x86_64 sizes are 128 bytes per wall mark, 368 per sample and 96 per
comparison. The borrowed venue is checked and not retained or allocated.

The six focused controls include 256 generated independent integer cases and
explicit threshold, signed-extreme, identity, ordering and healthy-progress
checks. Additional mutation, full-workspace and independent code acceptance are
recorded outside this checkpoint; no production performance claim follows.

Before qualification, later reviewed producers must establish real request
bookends and bracketing, actual same-process ownership, positive exact sample
persistence, sticky observed-step invalidation, immutable capture-time verdicts,
and full-precision storage/consumer verification. Existing millisecond-only
ledger fields cannot independently prove a 50 ms plus 1 ns decision. A later
sample or successful publication must never upgrade a previously invalid
timestamp. Raw facts do not substitute for that missing bridge.

Invalid clock evidence never suppresses protection or exits; their separate
journal, pause and budget requirements remain binding. The unchanged C8 alerts
and bounded sampler lifecycle remain later work, as does actual fast recovery.
