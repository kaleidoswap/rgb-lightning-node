FROM rust:1.87.0-bookworm AS builder

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

# Copy manifests
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# Cache dependencies
RUN cargo fetch

# Build only dependencies to cache them
RUN cargo build --release --locked
RUN rm src/*.rs

# Copy source code
COPY ./src ./src

# Set environment variables for optimal compilation
ENV RUSTFLAGS="-C target-cpu=native"
ENV CARGO_BUILD_JOBS="8"

# Build application
RUN cargo build --release --locked

FROM debian:bookworm-slim

COPY --from=builder /app/target/release/rgb-lightning-node /usr/bin/rgb-lightning-node

RUN apt-get update && apt install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]
