# ADR-0013: CLI–Daemon IPC (Unix Domain Sockets / Named Pipes)

## Status
**Accepted** (2025-02-04)

## Context

Robson v2 (Atena CLI) targets a **single, easy-to-install binary** for end users on Windows, Linux, and macOS. The architecture is:

- **Core**: Rust daemon (`robsond`) — engine, execution, persistence.
- **CLI**: Bun/TypeScript (Atena CLI) — user-facing commands, output formatting.

Today the CLI talks to the daemon over **HTTP** on `localhost:8080`. That was chosen for Phase 6/6b (MVP) because it is simple, debuggable, and works for both local and remote daemons. For the **primary use case** — one machine, one user, CLI + daemon — we want:

1. **No port binding** — avoid conflicts with other apps and firewall prompts.
2. **Local-only by default** — no network stack, simpler security model.
3. **Low latency** — minimal overhead for frequent status/arm/disarm calls.
4. **Cross-platform** — same conceptual model on Windows, Linux, and macOS.

### Forces

- Users install a single CLI binary; the daemon may run as a background process or be spawned by the CLI.
- Communication is **local process-to-process** in the vast majority of deployments.
- Remote daemon (e.g., server + laptop CLI) is a possible future scenario but not the main target for the “easy binary” story.
- Bun (CLI) and Rust (daemon) both support Unix domain sockets (Unix) and named pipes (Windows).

---

## Decision

We will use **local IPC** for Atena CLI ↔ Core (Rust daemon) communication:

- **Linux / macOS**: **Unix Domain Sockets (UDS)** — a socket file (e.g. `~/.robson/robsond.sock` or `$XDG_RUNTIME_DIR/robsond.sock`).
- **Windows**: **Named pipes** — e.g. `\\.\pipe\robsond` (or a configurable pipe name).

The **transport protocol on top of the socket/pipe** remains **request/response** (e.g. HTTP-like or a small JSON protocol). The important part is the **transport layer**: no TCP port, local-only by default.

### Configuration

- **Socket/pipe path** is configurable (environment variable and/or config file) so that:
  - Defaults work out-of-the-box for single-user, single-machine.
  - Advanced users can override (e.g. custom path, or future HTTP fallback for remote daemon).
- **Fallback to HTTP** (e.g. `ROBSON_DAEMON_URL=http://localhost:8080`) can be retained as an **optional** mode for remote or legacy setups, without being the default.

### Out of scope for this ADR

- Exact wire format on the socket/pipe (e.g. HTTP/1.1 over UDS, or a tiny custom protocol) — left to implementation.
- How the daemon is started (user-managed service vs CLI-spawned subprocess) — separate design.

---

## Consequences

### Positive

- **No port** — no binding to 8080 or similar; avoids “port already in use” and firewall prompts on Windows.
- **Local-only by default** — no accidental exposure on the network; simpler mental model for “one binary, one machine.”
- **Lower latency** — kernel IPC instead of loopback TCP.
- **Aligns with “easy binary”** — one installable (CLI + optional daemon), communication stays on the machine.
- **Cross-platform** — same abstraction (path or pipe name) on all three OSes; implementation uses UDS vs named pipe per platform.

### Negative / Trade-offs

- **Remote daemon** is not the default; users who want “CLI on laptop, daemon on server” will use HTTP (or an explicit “remote” mode) if we support it.
- **Implementation effort** — daemon and CLI must both support socket/pipe listener and client; we add code paths and tests.
- **Observability** — some tools assume “HTTP on a port”; we may need to document how to trace or proxy (e.g. if we add a debug mode that exposes HTTP on a port).

---

## Alternatives

### A: Keep HTTP on localhost (current)

- **Why not chosen**: Port binding, firewall prompts on Windows, and “network” semantics when both processes are on the same machine. Acceptable for MVP but not the long-term default for “Atena CLI = one easy binary.”

### B: gRPC over TCP or over UDS

- **Why not chosen for now**: More complexity (codegen, .proto, client/server stubs). We can introduce gRPC later if we need streaming or a stricter API contract; UDS/named pipe does not block that (gRPC can run over UDS).

### C: Stdio / in-process (CLI spawns daemon and talks via stdin/stdout)

- **Why not chosen here**: Requires a different process model (CLI always owns the daemon process). We keep the option “daemon runs as a service” open; IPC (UDS/pipe) works for both “CLI + long-lived daemon” and “CLI spawns daemon” if we want that later.

---

## Implementation Notes

- **Rust (robsond)**:
  - Listen on a configurable UDS path (Unix) or named pipe (Windows).
  - Reuse existing HTTP handlers (e.g. Axum) over the socket/pipe if the wire format is HTTP/1.1; or implement a small JSON request/response over the same transport.
- **Bun (Atena CLI)**:
  - Client that connects to a path (Unix) or pipe name (Windows); same request/response API as today (`status`, `arm`, `disarm`, `panic`).
- **Configuration**:
  - e.g. `ROBSON_SOCKET_PATH` (Unix) / `ROBSON_PIPE_NAME` (Windows), or a single `ROBSON_DAEMON_IPC` that accepts path or pipe.
  - Optional: `ROBSON_DAEMON_URL` for HTTP fallback (remote or legacy).
- **Tests**:
  - Integration tests: CLI connects to daemon over UDS (Unix) or named pipe (Windows); same acceptance criteria as current HTTP tests.
- **Documentation**:
  - Update `v2/docs/ARCHITECTURE.md` and `v2/docs/CLI.md` to state that CLI–daemon communication uses local IPC (UDS / named pipe) by default, with optional HTTP for remote.

---

## References

- [v2/docs/ARCHITECTURE.md](../ARCHITECTURE.md) — system overview, CLI ↔ daemon
- [v2/docs/CLI.md](../CLI.md) — CLI reference, configuration
- [v2/docs/EXECUTION-PLAN.md](../EXECUTION-PLAN.md) — Phase 6/6b, future phases
- ADR-0012 (Event Sourcing) — unrelated; this ADR is scoped to CLI–daemon transport only.
