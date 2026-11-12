# syntax=docker/dockerfile:1.7
# One Dockerfile for every Rust service:
#   docker build -f infrastructure/docker/service.Dockerfile --build-arg SERVICE=identity-service -t identity-service .
FROM rust:1.94-bookworm AS build
ARG SERVICE
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY services services
COPY proto proto
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p "${SERVICE}" && cp "target/release/${SERVICE}" /service

# Distroless: no shell, no package manager, non-root by default.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /service /service
USER nonroot:nonroot
ENV HTTP_ADDR=0.0.0.0:8080 GRPC_ADDR=0.0.0.0:9090 LOG_FORMAT=json
EXPOSE 8080 9090
ENTRYPOINT ["/service"]
