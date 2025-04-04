# Use a smaller base image for builder
FROM rust:1.83.0-slim-bookworm AS builder

# Install git for submodule handling
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Set up working directory
WORKDIR /usr/src/app

# Clone the repository with submodules
RUN git clone https://github.com/RGB-Tools/rgb-lightning-node --recurse-submodules --shallow-submodules .

# Build with release optimizations
RUN cargo build --release

# Create smaller runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Copy only the built binary
COPY --from=builder /usr/src/app/target/release/rgb-lightning-node /usr/bin/

# Create a non-root user
RUN useradd -ms /bin/bash rln
USER rln

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]