FROM rust:1.91-slim-trixie AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked

WORKDIR /app


FROM chef AS planner

COPY . .
RUN cargo chef prepare --recipe-path recipe.json


FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json
COPY rust-lightning/ rust-lightning/
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release


FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

COPY --from=builder /app/target/release/rgb-lightning-node /usr/bin/rgb-lightning-node

ENTRYPOINT ["/usr/bin/rgb-lightning-node"]
