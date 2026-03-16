FROM rust:bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/matrix-bridge /usr/local/bin/matrix-bridge

RUN mkdir -p /data

ENV BRIDGE_CONFIG=/data/config.toml
ENV BRIDGE_REGISTRATION=/data/registration.yaml

EXPOSE 29320

ENTRYPOINT ["matrix-bridge"]
