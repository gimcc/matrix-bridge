FROM rust:1.94-bookworm AS chef
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-chef
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release && \
    cp /build/target/release/matrix-bridge /matrix-bridge

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /matrix-bridge /usr/local/bin/matrix-bridge

RUN mkdir -p /data

ENV BRIDGE_CONFIG=/data/config.toml
ENV BRIDGE_REGISTRATION=/data/registration.yaml

EXPOSE 29320

ENTRYPOINT ["matrix-bridge"]
