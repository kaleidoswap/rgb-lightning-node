# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.83.0-slim-bookworm AS builder

ARG TARGETPLATFORM
ARG BUILDPLATFORM
ARG TARGETOS
ARG TARGETARCH

# Install base dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install target-specific dependencies and set up cross-compilation
RUN case "$TARGETARCH" in \
    "amd64") \
        rustup target add x86_64-unknown-linux-gnu ;; \
    "arm64") \
        apt-get update && apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu \
            libc6-dev-arm64-cross \
            libssl-dev:arm64 \
        && rustup target add aarch64-unknown-linux-gnu \
        && rm -rf /var/lib/apt/lists/* ;; \
    *) \
        echo "Unsupported architecture: $TARGETARCH" && exit 1 ;; \
    esac

WORKDIR /usr/src/app

# Copy dependency files first for better caching
COPY Cargo.toml Cargo.lock ./
# Copy the entire source since you're using submodules
COPY . .

# Set up cross-compilation environment for ARM64
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        mkdir -p .cargo && \
        echo '[target.aarch64-unknown-linux-gnu]' > .cargo/config.toml && \
        echo 'linker = "aarch64-linux-gnu-gcc"' >> .cargo/config.toml && \
        echo '[env]' >> .cargo/config.toml && \
        echo 'PKG_CONFIG_ALLOW_CROSS = "1"' >> .cargo/config.toml; \
    fi

# Build for the target architecture
RUN case "$TARGETARCH" in \
    "amd64") \
        cargo build --release --target x86_64-unknown-linux-gnu && \
        cp target/x86_64-unknown-linux-gnu/release/rgb-lightning-node /usr/local/bin/ ;; \
    "arm64") \
        PKG_CONFIG_ALLOW_CROSS=1 \
        PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig \
        cargo build --release --target aarch64-unknown-linux-gnu && \
        cp target/aarch64-unknown-linux-gnu/release/rgb-lightning-node /usr/local/bin/ ;; \
    esac

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /usr/local/bin/rgb-lightning-node /usr/bin/rgb-lightning-node

# Create non-root user
RUN useradd -r -s /bin/false -d /nonexistent rln

USER rln
ENTRYPOINT ["/usr/bin/rgb-lightning-node"]