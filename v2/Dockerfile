# ============================================================================
# Stage 1: Build
# ============================================================================
FROM rust:1.83-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY robsond/ ./robsond/
COPY robson-engine/ ./robson-engine/
COPY robson-domain/ ./robson-domain/
COPY robson-store/ ./robson-store/
COPY robson-connectors/ ./robson-connectors/
COPY robson-exec/ ./robson-exec/

# Build release binary
RUN cargo build --release -p robsond

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM debian:bookworm-slim

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
