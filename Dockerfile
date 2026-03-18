FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 bridge && useradd -u 1000 -g bridge -d /data -s /sbin/nologin bridge

ARG TARGETARCH
COPY matrix-bridge-linux-${TARGETARCH} /usr/local/bin/matrix-bridge
RUN chmod +x /usr/local/bin/matrix-bridge

RUN mkdir -p /data && chown bridge:bridge /data

ENV BRIDGE_CONFIG=/data/config.toml
ENV BRIDGE_REGISTRATION=/data/registration.yaml

EXPOSE 29320

USER bridge
WORKDIR /data

ENTRYPOINT ["matrix-bridge"]
