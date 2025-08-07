FROM rust:1.89-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    git \
    && rm -rf /var/lib/apt/lists/*

# Create a new empty shell project
RUN USER=root cargo new --bin app
WORKDIR /app

# Copy the Git submodule first
COPY rust-lightning rust-lightning/

# Copy manifests first for better caching
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# Build only dependencies (this layer will be cached if dependencies don't change)
RUN cargo build --release --locked && rm src/*.rs target/release/deps/app*

# Copy source code
COPY ./src ./src

# Set environment variables for optimal compilation
ENV RUSTFLAGS="-C target-cpu=generic -C opt-level=3 -C codegen-units=1 -C lto=thin"
ENV CARGO_BUILD_JOBS="$(nproc)"

# Build application (only app code will rebuild if dependencies haven't changed)
RUN cargo build --release --locked

# Use distroless for smaller final image and better security
FROM gcr.io/distroless/cc-debian12:nonroot

# Copy the binary
COPY --from=builder /app/target/release/rgb-lightning-node /usr/bin/rgb-lightning-node

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]