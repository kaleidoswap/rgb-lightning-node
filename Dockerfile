# syntax=docker/dockerfile:1
FROM rust:1.87.0-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set build arguments for cross-compilation optimization
ARG TARGETPLATFORM
ARG BUILDPLATFORM

WORKDIR /app

# Copy the Git submodule first (required for dependency resolution)
COPY rust-lightning rust-lightning/

# Copy manifests for dependency caching
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# Set optimal build jobs and environment based on platform
ENV CARGO_NET_RETRY="10"
ENV CARGO_NET_TIMEOUT="60"

# Create a dummy main.rs to cache dependencies
RUN mkdir -p src && echo "fn main() {}" > src/main.rs

# Cache dependencies with optimized settings
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    if [ "$TARGETPLATFORM" = "linux/arm64" ]; then \
        export RUSTFLAGS="-C target-cpu=cortex-a72 -C opt-level=2" && \
        export CARGO_BUILD_JOBS="4"; \
    elif [ "$TARGETPLATFORM" = "linux/amd64" ]; then \
        export RUSTFLAGS="-C target-cpu=x86-64 -C opt-level=3" && \
        export CARGO_BUILD_JOBS="6"; \
    else \
        export RUSTFLAGS="-C opt-level=2" && \
        export CARGO_BUILD_JOBS="4"; \
    fi && \
    cargo build --release --locked

# Remove the dummy source
RUN rm -rf src/

# Copy source code
COPY ./src ./src

# Build application with platform-specific optimizations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    if [ "$TARGETPLATFORM" = "linux/arm64" ]; then \
        export RUSTFLAGS="-C target-cpu=cortex-a72 -C opt-level=2 -C codegen-units=1" && \
        export CARGO_BUILD_JOBS="4"; \
    elif [ "$TARGETPLATFORM" = "linux/amd64" ]; then \
        export RUSTFLAGS="-C target-cpu=x86-64 -C opt-level=3" && \
        export CARGO_BUILD_JOBS="6"; \
    else \
        export RUSTFLAGS="-C opt-level=2" && \
        export CARGO_BUILD_JOBS="4"; \
    fi && \
    cargo build --release --locked && \
    cp target/release/rgb-lightning-node /usr/local/bin/rgb-lightning-node

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    openssl \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Copy the binary from builder
COPY --from=builder /usr/local/bin/rgb-lightning-node /usr/bin/rgb-lightning-node

# Create non-root user for security
RUN groupadd -r appuser && useradd -r -g appuser appuser
USER appuser

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]