# syntax=docker/dockerfile:1.27.1
# Executor Events adapter (the `omni-events-adapter` crate).
# Build from the repository root so the Cargo workspace and lock file resolve:
#   docker build -f deploy/executor-events/Dockerfile -t ghcr.io/micthiesen/executor-events-adapter .
# Dockerfile.dockerignore beside this file limits the context to the workspace.
#
# cargo-chef keeps the dependency build in its own layer, which CI restores from
# the GitHub Actions cache (cache mounts are never exported there).

FROM rust:1.97.1-bookworm AS chef
COPY deploy/build-tools/fetch-verified.sh deploy/build-tools/install-cargo-chef.sh /tmp/build-tools/
RUN /tmp/build-tools/install-cargo-chef.sh /usr/local/cargo/bin && rm -rf /tmp/build-tools
WORKDIR /src
COPY rust-toolchain.toml ./
RUN rustup toolchain install

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p omni-events-adapter
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
# The adapter has no workspace path dependencies; other crates keep their skeletons.
COPY crates/omni-events-adapter ./crates/omni-events-adapter
RUN cargo build --release --locked -p omni-events-adapter --bin executor-events-adapter \
    && install -m 0755 target/release/executor-events-adapter /usr/local/bin/executor-events-adapter

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /usr/local/bin/executor-events-adapter /usr/local/bin/executor-events-adapter
USER nonroot
ENV PORT=4789
EXPOSE 4789
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/executor-events-adapter", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/executor-events-adapter"]
