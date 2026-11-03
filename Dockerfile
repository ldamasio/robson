# ============================================================================
# Stage 1: Build
# ============================================================================
# Pinned by digest (immutable) and to the exact toolchain in rust-toolchain.toml
# (1.92.0): the image used to be the rolling `rust:1-slim`, so production was
# compiled with whatever 1.x Docker Hub served that day, not the version CI
# tested. robsond.yml asserts that this tag matches rust-toolchain.toml.
# Refresh the digest with
#   skopeo inspect --raw docker://docker.io/library/rust:<ver>-slim-bookworm | sha256sum
# (or the Docker-Content-Digest header of the registry's manifest-list HEAD).
FROM docker.io/library/rust:1.92.0-slim-bookworm@sha256:f1f73538ebe623fd3673a35aff3df358ae1084c64c55646516e5b17b321b6c9b AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY robson-domain/ ./robson-domain/
COPY robson-engine/ ./robson-engine/
COPY robson-exec/ ./robson-exec/
COPY robson-connectors/ ./robson-connectors/
COPY robson-store/ ./robson-store/
COPY robson-eventlog/ ./robson-eventlog/
COPY robson-projector/ ./robson-projector/
COPY robson-db/ ./robson-db/
COPY migrations/ ./migrations/
COPY robson-testkit/ ./robson-testkit/
COPY robson-sim/ ./robson-sim/
COPY robson-cli/ ./robson-cli/
COPY robsond/ ./robsond/

# Build release binary with postgres feature for production
RUN cargo build --release -p robsond --features postgres

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM docker.io/library/debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3

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
