# syntax=docker/dockerfile:1.7
# One Dockerfile for every Rust service. The build stage compiles every service binary once,
# so building all twelve images reuses one compilation:
#   docker build -f infrastructure/docker/service.Dockerfile --build-arg SERVICE=identity-service -t identity-service .
# RUNTIME picks the final image: `plain` for most services, `xmlsec` for identity-service
# (SAML signatures are checked by xmlsec1, a C library; scripts/images/build.sh sets it).
ARG RUNTIME=plain

FROM rust:1.94-bookworm AS build
# libcurl headers: librdkafka includes curl.h even with curl off. OpenSSL is the system's,
# shared by librdkafka, the openssl crate and xmlsec1, so a process has one copy of it.
# libxml2/xmlsec1 headers and libclang: samael binds to xmlsec1 with bindgen.
RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler cmake libcurl4-openssl-dev pkg-config libssl-dev libclang-dev libxml2-dev libxmlsec1-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY services services
COPY proto proto
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bins \
    && mkdir -p /out/lib \
    && find target/release -maxdepth 1 -type f -perm -u+x -name '*-service' -exec cp {} /out/ \; \
    && cp "$(gcc -print-file-name=libgcc_s.so.1)" /out/lib/

# Distroless: no shell, no package manager, non-root by default. TLS to the data stores is
# rustls; Kafka's TLS is librdkafka over the image's OpenSSL (libssl3, tracked by the image's
# package list, so scanners see it). Only the unwinder (libgcc_s) is copied in.
FROM gcr.io/distroless/base-debian12:nonroot AS runtime-plain
COPY --from=build /out/lib/libgcc_s.so.1 /usr/lib/

# identity-service also needs libxmlsec1 (with libxml2, libxslt and OpenSSL). Distroless has no
# package for it, so this is Debian slim with those installed from Debian, which scanners can
# track; the service runs as the same non-root uid as distroless.
FROM debian:bookworm-slim AS runtime-xmlsec
RUN apt-get update && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends libxmlsec1-openssl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 65532 --user-group --no-create-home --shell /usr/sbin/nologin nonroot

FROM runtime-${RUNTIME}
ARG SERVICE
COPY --from=build /out/${SERVICE} /service
USER 65532:65532
ENV HTTP_ADDR=0.0.0.0:8080 GRPC_ADDR=0.0.0.0:9090 LOG_FORMAT=json
EXPOSE 8080 9090
ENTRYPOINT ["/service"]
