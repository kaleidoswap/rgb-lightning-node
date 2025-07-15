# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:latest-rust-1.87.0-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set build arguments for cross-compilation optimization
ARG TARGETPLATFORM
ARG BUILDPLATFORM

# Optimize for specific architectures
RUN if [ "$TARGETPLATFORM" = "linux/arm64" ]; then \
        export RUSTFLAGS="-C target-cpu=cortex-a72 -C opt-level=2"; \
    elif [ "$TARGETPLATFORM" = "linux/amd64" ]; then \
        export RUSTFLAGS="-C target-cpu=x86-64 -C opt-level=3"; \
    else \
        export RUSTFLAGS="-C opt-level=2"; \
    fi

# Set optimal build jobs based on platform
ENV CARGO_BUILD_JOBS="4"
ENV CARGO_NET_RETRY="10"
ENV CARGO_NET_TIMEOUT="60"

# Copy the recipe from planner stage
COPY --from=planner /app/recipe.json recipe.json

# Build only dependencies (this will be cached)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# Copy source code
COPY . .

# Build application with optimizations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    if [ "$TARGETPLATFORM" = "linux/arm64" ]; then \
        RUSTFLAGS="-C target-cpu=cortex-a72 -C opt-level=2 -C codegen-units=1"; \
    elif [ "$TARGETPLATFORM" = "linux/amd64" ]; then \
        RUSTFLAGS="-C target-cpu=x86-64 -C opt-level=3"; \
    else \
        RUSTFLAGS="-C opt-level=2"; \
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