FROM rust:1.87.0-bookworm AS builder

COPY . .

RUN cargo build --release

# Start a new stage for the final image
FROM debian:bookworm-slim

# Copy the binary from the builder stage
COPY --from=builder ./target/release/rgb-lightning-node /usr/bin/rgb-lightning-node

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]