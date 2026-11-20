# Private local budget model

This checkpoint implements numerical local transitions only. It has no runtime
caller and produces no send grant, durable receipt, emergency confirmation or
complete daily-count evidence. SD12 and USD-M behavior are unchanged.

The model preallocates at most 1,200 records and owns one Arc identity marker.
Single-owned weak references retain that marker allocation after model drop,
preventing allocation identity reuse. Checked sequence IDs never wrap or reuse
removed IDs. References have no public constructor, Clone, Copy or Deserialize.
Dropping a reference conservatively retains its Pending charge; this is not the
later runtime RAII cleanup implementation.

Each time-bearing call first validates monotonic same-domain time. A valid
observation advances the watermark even when later validation refuses the
request. A reversal or domain change sticks invalid and preserves charges.
Time-free explicit Pending cancellation remains available. No method refunds
Sent records. Their original handoff controls exact 10/60-second aging; Pending
never expires automatically, even after its handoff deadline.

One combined scan prunes expired Sent records and sums existing factual kernel
results. Lookup is at most one further scan: no more than 2,400 record visits per
operation. Insertion reuses the accepted startup and prospective-limit kernels.
Fixed startup epoch counts are not aged down during the initial windows.
Normal and expanded tiers remain unqualified scenarios. Expanded records still
need the absent emergency authorization bridge; daily totals are supplied data.

Construction has two allocations: fixed record storage and the identity marker.
Unit-binary allocator instrumentation separately measures construction, hot
insertion/handoff/pruning/cancellation and destruction with a retained Weak.
This is a component allocation test, not live runtime profiling or a hard latency
guarantee for allocation, refcounts or scheduling. Zero SQL, HTTP, native reads,
awaits, application locks, tasks, timers and persistence occur in the model.
On the measured x86_64 test binary each record is 256 bytes, the reference is
16 bytes and the model (including a test-only visit counter) is 176 bytes.
Construction allocates 307,200 record bytes plus a 16-byte marker; the measured
hot operations allocate zero. A retained Weak keeps only the marker until drop.

The independent event-log oracle recomputes final rolling charges after every
generated event, including refused insertions and abandoned references. Named
regressions cover cross-instance IDs, destruction/recreation, saturation, exact
deadlines, startup limits and the refused 160-second prune followed by a
159-second observation. Mutation and review results are recorded separately;
no full-workspace gate or operational acceptance follows from these tests.

Runtime publication remains blocked on complete durable bootstrap and daily
binding, current emergency generation, pause/header ownership, the reviewed
gate/journal/budget lock graph, atomic first-poll handoff and exactly-once proved
unsent cleanup. The actual model methods are private and cannot bypass these
requirements. N5 recovery, ADR section 10, 24-hour rules, dated overrides and
35/25/200 SQL composition requirements remain unchanged.
