# Temporary Chrono maintenance-risk disposition

**OPERATOR AUTHORIZED 2026-11-17 — IMPLEMENTATION AND INTEGRATION PENDING ACCEPTANCE.** The operator explicitly authorized this exact temporary maintenance-risk disposition on 2026-11-17 in the coordinating conversation. This authorization does not assert a green CI result, review approval, merge or Spot activation. It replaces the earlier config-only idea; `osv-scanner.toml` receives no new ignore.

On 2026-11-17, upstream RUSTSEC-2026-0543 reports Chrono as informational/unmaintained for every version, with no patch, CVE alias or severity. Robson uses Chrono directly and oauth2 5.0.0 also requires it without an optional feature. It is a real maintenance warning, not a false positive.

Authorized duration: **strictly before 2026-12-17T00:00:00Z**, the start of December 17 UTC (December 16, 21:00 Sao Paulo). No automatic renewal. The exact deadline is encoded in the proposed classifier.

The pinned OSV 2.4.0 scanner would still scan the same three explicit lockfiles using the existing scanner configuration. Its JSON output, error stream, original status, input/binary hashes and decision would be preserved as one CI artifact. The Chrono advisory stays unignored and visible; expected raw scanner status remains **1**. A separate explicit maintenance-risk disposition may return CI success only for that same scan's one exact Chrono object, its exact crates.io identity, unmaintained classification and alias-free/severity-free group. A canonical digest of the full reviewed advisory makes any advisory-content change blocking, including a new fix. No metadata is fetched later to decide the disposition, so there is no precheck-versus-scan metadata race.

Any unrelated finding, either SvelteKit CVE (GHSA-3r8g-jcqq-3p5j / GHSA-6cf5-gc4q-fm3x), additional advisory, alias/CVE, severity, reclassification, unexpected schema, missing source, scanner/input change, nonempty error stream, operational status, malformed output, missing exact maintenance object or expiry remains blocking. This is neither a severity filter nor a package/group ignore. Existing unrelated approved scanner exceptions remain as they are; this proposal does not newly defer them or assert that they were rescanned unfiltered.

The scanner's own raw JSON is not rewritten or filtered by this classifier. `--all-packages` enables source coverage checks. `--verbosity=error` plus rejection of any stderr guards against a partial scan that logs an error but exits 1 due to vulnerability-result precedence. The scanner implementation is trusted as before; the wrapper does not independently reimplement package extraction or prove completeness of the OSV service's database. Any legitimate serializer/schema difference must fail closed and be reviewed rather than broadened silently.

Removal plan:

- Resolve a supported Chrono-free authentication dependency path without an OAuth/library fork or handwritten authorization flow. SQLx 0.8.6 already offers native optional `time` codecs, but replacing local imports alone does not prove a Chrono-free lockfile.
- Migrate date/time uses while preserving timestamp JSON, event-hash bytes, precision/offsets, UTC calendar boundaries, PG12/16 codecs, Google device/refresh behavior and Decimal financial arithmetic.
- Demonstrate the resolved graph and actual enabled graph no longer require Chrono. Remove this temporary classifier/workflow disposition and obtain exact-head independent review, applicable nine gates and CI4. If it is not removed by the deadline the gate fails; extending it requires a new operator decision.

Operator decision: explicit authorization received on 2026-11-17, recorded by the coordinator before this implementation. Integration results, exact implementation commit, independent review and CI evidence remain pending acceptance. Official source inspection and offline model tests precede this change; they are not a substitute for the real scanner run. This change does not enable Spot or authorize real capital.

Sources: [OSV 2.4 full JSON model](https://github.com/google/osv-scanner/blob/v2.4.0/pkg/models/results.go), [JSON serializer](https://github.com/google/osv-scanner/blob/v2.4.0/internal/output/machinejson.go), [CLI status precedence](https://github.com/google/osv-scanner/blob/v2.4.0/cmd/osv-scanner/internal/cmd/run.go), [partial matcher error handling](https://github.com/google/osv-scanner/blob/v2.4.0/pkg/osvscanner/osvscanner.go), [RustSec advisory](https://github.com/RustSec/advisory-db/blob/main/crates/chrono/RUSTSEC-2026-0543.md).
