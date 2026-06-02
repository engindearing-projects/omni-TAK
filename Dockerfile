# Multi-stage Dockerfile for OmniTAK
# Stage 1: Build the Rust application
FROM rust:1.90-slim-bookworm AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /usr/src/omnitak

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# Copy source code
COPY src/ ./src/

# Build for release (headless: skip eframe/egui/winit, which have no server backend)
RUN cargo build --release --bin omnitak --no-default-features

# Stage 2: Create minimal runtime image
FROM debian:bookworm-slim

# Install runtime dependencies (curl is used by the healthcheck)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -m -u 1000 omnitak

# Create directories
RUN mkdir -p /app/config /app/certs && \
    chown -R omnitak:omnitak /app

WORKDIR /app

# Copy binary from builder
COPY --from=builder /usr/src/omnitak/target/release/omnitak /app/omnitak

# Bake in a sensible default config so `docker run` works with no extra files.
# Override by mounting your own file over /app/config/config.yaml.
COPY docker/config.docker.yaml /app/config/config.yaml

# Change ownership
RUN chown -R omnitak:omnitak /app

# Switch to app user
USER omnitak

# Expose the API + embedded web UI port
EXPOSE 8080

# Health check hits the unauthenticated health endpoint
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -fsS http://localhost:8080/api/v1/health || exit 1

# Run the binary
ENTRYPOINT ["/app/omnitak"]
CMD ["--config", "/app/config/config.yaml"]
