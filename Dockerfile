# Use a smaller base image for builder
FROM --platform=$BUILDPLATFORM rust:1.83.0-slim-bookworm AS builder

# Use buildx ARGs for cross-compilation
ARG TARGETPLATFORM
ARG BUILDPLATFORM

# Install build dependencies and set up cross-compilation environment
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    && case "$TARGETPLATFORM" in \
        "linux/amd64") \
            echo "Building for amd64" && \
            rustup target add x86_64-unknown-linux-gnu ;; \
        "linux/arm64") \
            echo "Building for arm64" && \
            rustup target add aarch64-unknown-linux-gnu && \
            apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu ;; \
        *) \
            echo "Unsupported platform: $TARGETPLATFORM" && exit 1 ;; \
    esac \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Set up working directory
WORKDIR /usr/src/app

# Copy the local repository with submodules
COPY . .

# Set up cross-compilation config and build
RUN case "$TARGETPLATFORM" in \
        "linux/amd64") \
            echo "Building for amd64" && \
            cargo build --release --target x86_64-unknown-linux-gnu && \
            mkdir -p /build && \
            cp target/x86_64-unknown-linux-gnu/release/rgb-lightning-node /build/ ;; \
        "linux/arm64") \
            echo "Building for arm64" && \
            mkdir -p .cargo && \
            echo '[target.aarch64-unknown-linux-gnu]' > .cargo/config.toml && \
            echo 'linker = "aarch64-linux-gnu-gcc"' >> .cargo/config.toml && \
            cargo build --release --target aarch64-unknown-linux-gnu && \
            mkdir -p /build && \
            cp target/aarch64-unknown-linux-gnu/release/rgb-lightning-node /build/ ;; \
    esac

# Create smaller runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Copy the built binary from the builder stage
COPY --from=builder /build/rgb-lightning-node /usr/bin/

# Create a non-root user
RUN useradd -ms /bin/bash rln
USER rln

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]