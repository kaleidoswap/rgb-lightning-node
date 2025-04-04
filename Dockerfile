# Use a smaller base image for builder
FROM --platform=$BUILDPLATFORM rust:1.83.0-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Set up working directory
WORKDIR /usr/src/app

# Copy the local repository with submodules
COPY . .

# Use buildx ARGs for cross-compilation
ARG TARGETPLATFORM
ARG BUILDPLATFORM

# Set up cross-compilation environment if needed
RUN case "$TARGETPLATFORM" in \
        "linux/amd64") \
            echo "Building for amd64" && \
            rustup target add x86_64-unknown-linux-gnu; \
            TARGET="x86_64-unknown-linux-gnu" ;; \
        "linux/arm64") \
            echo "Building for arm64" && \
            rustup target add aarch64-unknown-linux-gnu && \
            apt-get update && \
            apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu && \
            apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*; \
            TARGET="aarch64-unknown-linux-gnu" ;; \
        *) \
            echo "Unsupported platform: $TARGETPLATFORM" && exit 1 ;; \
    esac && \
    if [ "$TARGETPLATFORM" != "$BUILDPLATFORM" ]; then \
        echo "Cross-compiling from $BUILDPLATFORM to $TARGETPLATFORM"; \
        mkdir -p .cargo && \
        echo '[target.'$TARGET']' > .cargo/config.toml && \
        echo 'linker = "'$(case "$TARGETPLATFORM" in \
            "linux/arm64") echo "aarch64-linux-gnu-gcc" ;; \
            *) echo "gcc" ;; \
        esac)'"' >> .cargo/config.toml; \
    fi

# Build with release optimizations for the target platform
RUN case "$TARGETPLATFORM" in \
        "linux/amd64") \
            cargo build --release --target x86_64-unknown-linux-gnu ;; \
        "linux/arm64") \
            cargo build --release --target aarch64-unknown-linux-gnu ;; \
    esac

# Determine the path to the built binary based on target platform
RUN mkdir -p /build && \
    case "$TARGETPLATFORM" in \
        "linux/amd64") \
            cp /usr/src/app/target/x86_64-unknown-linux-gnu/release/rgb-lightning-node /build/ ;; \
        "linux/arm64") \
            cp /usr/src/app/target/aarch64-unknown-linux-gnu/release/rgb-lightning-node /build/ ;; \
    esac

# Create smaller runtime image
FROM --platform=$TARGETPLATFORM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Copy the built binary from the builder stage
COPY --from=builder /build/rgb-lightning-node /usr/bin/

# Create a non-root user
RUN useradd -ms /bin/bash rln
USER rln

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]