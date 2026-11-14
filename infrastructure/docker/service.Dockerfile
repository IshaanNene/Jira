# syntax=docker/dockerfile:1.7
# One Dockerfile for every Rust service. The build stage compiles every service binary once,
# so building all twelve images reuses one compilation:
#   docker build -f infrastructure/docker/service.Dockerfile --build-arg SERVICE=identity-service -t identity-service .
FROM rust:1.94-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler cmake && rm -rf /var/lib/apt/lists/*
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

# Distroless without OpenSSL: no shell, no package manager, non-root by default. TLS is rustls,
# Kafka's OpenSSL and zlib are linked in statically, so the binaries need only glibc and the
# unwinder (libgcc_s, copied from the build stage into a default library directory).
FROM gcr.io/distroless/base-nossl-debian12:nonroot
ARG SERVICE
COPY --from=build /out/lib/libgcc_s.so.1 /usr/lib/
COPY --from=build /out/${SERVICE} /service
USER nonroot:nonroot
ENV HTTP_ADDR=0.0.0.0:8080 GRPC_ADDR=0.0.0.0:9090 LOG_FORMAT=json
EXPOSE 8080 9090
ENTRYPOINT ["/service"]
