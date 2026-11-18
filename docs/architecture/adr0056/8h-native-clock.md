# Native clock candidates — ADR-0056 step 8h-1a

Status: local implementation; acceptance and operational rollout pending. This
slice provides unqualified coordinates and checked elapsed candidates only. No
production caller, SQL, HTTP, timer, worker, queue or financial authority exists.
SD12 and the venue factory remain unchanged. Neither a parsed TickCapture nor a
successful elapsed comparison grants fee freshness or request eligibility.

## Native identity and lifecycle

GNU Linux LP64 x86_64/aarch64 uses a direct SYS_clock_gettime/CLOCK_BOOTTIME call
through the existing exact libc0.2.191 binding. Other targets return Unsupported.
Boot UUID plus exact signed BOOTTIME namespace offset identifies the candidate
domain. Inode equality is an attribution check, not a persisted domain key.

Initialization reads the canonical TGID/task/TID target of /proc/thread-self.
Both positive i32 components are validated; the suffix selects the top-level
/proc/TID/timens_offsets file. Unlike gettid, these coordinates belong to the
mounted procfs PID namespace, including ancestor-mounted procfs. The nested
thread-self/timens_offsets file is absent on the tested Linux kernels.
Current and children time-namespace links must agree before and after the clock
call, and both boot/offset reads must match. Offsets describe the children
namespace, so merely reading a stable offset twice would be insufficient.

This requires an authentic stable proc mount and a fixed time namespace across
all calling threads for the source lifetime. Checks do not exclude ABA, VM
restore, privileged mutation or future topology changes. Deployment verification,
restore fencing and readiness ownership are later integration requirements.
No namespace, clock, privilege or host state is changed by this slice.

## Work and failure bounds

| Operation | Explicit bound |
|---|---|
| Initialization | 4 opens, at most16 reads,4 closes,5 readlinks,1 clock call: at most30 owned native calls. |
| Each subsequent capture | Exactly1 clock syscall;0 metadata/file/SQL/HTTP calls. |
| Each file | At most4 read attempts including EINTR; EOF required. Boot cap128 bytes, offsets cap1024, each with a cap+1 overflow buffer. |
| Symlinks/path | Thread link64-byte buffer; namespace links128; final path64 (maximum31 bytes plus NUL). Truncation refuses. |

All storage in the helper is fixed-size. No userspace allocation, global lazy
initializer, fallback or retry loop is introduced. Kernel allocations and loader
startup are outside that claim. This is a work bound, not a scheduling deadline:
a syscall may stall. Initialization must remain outside the final send seam.

Files close exactly once, including error paths; a close error is not retried.
Invalid/missing/oversized metadata, native errors or unsupported ABI refuse the
source. Malformed timespecs refuse capture. Domain mismatch, reversal and range
errors refuse elapsed candidates. No wall-clock replacement, saturated age or
new-domain reset is used. Missing DB/venue evidence cannot be supplied here.
Process death loses only an in-memory handle; no durable recovery is claimed.

## Evidence and remaining gates

The initial native test failed with ENOENT and remains recorded. The corrected
protocol passed real nonleader-thread smoke and injected exact-path, malformed
identity, byte/read caps, all22 normal-path native failure points, descriptor
cleanup and independent signed-integer elapsed properties. The injected PID-space
collision checks selection; no changed-namespace environment was created.

The measurement example runs fresh cold processes with trace markers, checks
userspace allocations and descriptor counts, then optionally samples capture
latency. Full process traces retain loader work outside the marked region. A
production-clock substitution must be caught by tracing the actual syscall,
not only by arithmetic tests. Ten fresh optimized-release processes on local
Linux7.0.0-38 x86_64 observed22 cold calls and1 BOOTTIME call per hot capture,
zero helper allocations and descriptor delta. Traced cold marker spans were
272–647 microseconds, including tracing/marker overhead. An untraced10,000-capture
run measured min281ns, p50283ns, p95285ns, p99286ns and max12,900ns, including
measurement overhead under concurrent local build load. These observations are
not hard latency guarantees or production startup evidence. Earlier debug traces
remain separately recorded. At checkpoint cd7721, 18 clean compiled mutants
reached inspected failures: 15 unit assertions, two external syscall-trace
assertions and one default-feature runtime assertion. An earlier duplicate-row
mutant survived; its regression now supplies identical malformed metadata for
both reads so domain-change rejection cannot mask the parser defect. The original
survivor remains recorded. Independent code review, coordinator gates and security
CI remain pending.

Making libc nonoptional does not enable crash-points. The module/macro feature
cfgs and forbidden-release guard remain intact. Default-feature runtime checking
and release-feature rejection passed as separate controls, not elapsed-clock evidence.
The release/crash combination failed specifically at the unchanged compile_error.
Linux6.12 and7.0 source checks confirm the same procfs identity/offset semantics;
aarch64 LP64 layout is checked against locked libc definitions, with actual
runtime measurements only on x86_64.

C8 server/wall error math, strict age<24h, logical request spacing, original
receipt provenance and positive current durable qualification belong to later
slices. This source does not implement CostAgeAuthority. Same-domain restart
support is a primitive; complete fast recovery and its performance remain an
independent pre-8o gate. ADR§10, the24h rule and dated override are unchanged.
