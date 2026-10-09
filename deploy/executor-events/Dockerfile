# syntax=docker/dockerfile:1.7
# Executor Events adapter (Rust port of packages/executor-events-adapter).
# Build from the repository root so the Cargo workspace and lock file resolve:
#   docker build -f deploy/executor-events/Dockerfile -t ghcr.io/micthiesen/executor-events-adapter .
# Dockerfile.dockerignore beside this file limits the context to the workspace.

FROM rust:1.97.1-bookworm AS build
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p omni-events-adapter --bin executor-events-adapter \
    && install -m 0755 target/release/executor-events-adapter /usr/local/bin/executor-events-adapter

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /usr/local/bin/executor-events-adapter /usr/local/bin/executor-events-adapter
USER nonroot
ENV PORT=4789
EXPOSE 4789
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/executor-events-adapter", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/executor-events-adapter"]
