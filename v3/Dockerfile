# ============================================================================
# Stage 1: Build
# ============================================================================
FROM docker.io/library/rust:1-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files (context is repo root, v3/ subdirectory)
COPY v3/Cargo.toml v3/Cargo.lock ./
COPY v3/robson-domain/ ./robson-domain/
COPY v3/robson-engine/ ./robson-engine/
COPY v3/robson-exec/ ./robson-exec/
COPY v3/robson-connectors/ ./robson-connectors/
COPY v3/robson-store/ ./robson-store/
COPY v3/robson-eventlog/ ./robson-eventlog/
COPY v3/robson-projector/ ./robson-projector/
COPY v3/robson-db/ ./robson-db/
COPY v3/migrations/ ./migrations/
COPY v3/robson-testkit/ ./robson-testkit/
COPY v3/robson-sim/ ./robson-sim/
COPY v3/robsond/ ./robsond/

# Build release binary with postgres feature for production
RUN cargo build --release -p robsond --features postgres

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM docker.io/library/debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 robson && \
    mkdir -p /home/robson/.robson && \
    chown -R robson:robson /home/robson

# Copy binary from builder
COPY --from=builder /app/target/release/robsond /usr/local/bin/robsond
RUN chown robson:robson /usr/local/bin/robsond

# Switch to non-root user
USER robson
WORKDIR /home/robson

# Expose API port
EXPOSE 8080

# Health check using curl
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/healthz || exit 1

# Run daemon
CMD ["robsond"]

