# syntax=docker/dockerfile:1.7

# --- Rust toolchain with cargo-chef (dependency layers cached separately) ---
FROM rust:1.97.1-bookworm AS chef
ARG CARGO_CHEF_VERSION=0.1.78
RUN cargo install cargo-chef --version "${CARGO_CHEF_VERSION}" --locked
WORKDIR /src
# Install the pinned toolchain's components once for every later stage.
COPY rust-toolchain.toml ./
RUN rustup toolchain install

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# --- Server binaries ---
FROM chef AS server
# sherpa-onnx-sys links a prebuilt static library; supply the SHA-256-verified
# archive so its build script never downloads an unchecked copy.
COPY deploy/sherpa-onnx/fetch-static-lib.sh /usr/local/bin/fetch-sherpa-onnx
RUN fetch-sherpa-onnx /opt/sherpa-onnx
ENV SHERPA_ONNX_ARCHIVE_DIR=/opt/sherpa-onnx
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json \
    -p omni-notify -p omni-live-intel
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo build --release --locked -p omni-notify --bin omni-notify \
    && cargo build --release --locked -p omni-live-intel \
      --bin omni-voice-enroll --bin omni-intel-doctor \
    && mkdir -p /out \
    && install -m 0755 target/release/omni-notify target/release/omni-voice-enroll \
      target/release/omni-intel-doctor /out/

# --- Web frontend (wasm is platform independent: build once on the builder) ---
FROM --platform=$BUILDPLATFORM chef AS web
COPY deploy/web-tools/install.sh /tmp/install-web-tools.sh
RUN /tmp/install-web-tools.sh /usr/local/bin && rm /tmp/install-web-tools.sh
RUN rustup target add wasm32-unknown-unknown
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --profile wasm-release --target wasm32-unknown-unknown --locked \
    --recipe-path recipe.json -p omni-web
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo fetch --locked \
    && cd crates/omni-web \
    && trunk build --release --offline --locked
# The SPA service serves .br/.gz siblings when the client accepts them.
RUN apt-get update \
  && apt-get install -y --no-install-recommends brotli \
  && rm -rf /var/lib/apt/lists/* \
  && find crates/omni-web/dist -type f \
    \( -name '*.html' -o -name '*.js' -o -name '*.css' -o -name '*.wasm' \) \
    -exec brotli -k -q 11 {} + -exec gzip -k -n -9 {} +

# --- Livestream intelligence assets ---
FROM debian:bookworm-slim AS livestream-assets

ARG TARGETARCH
ARG YT_DLP_VERSION=2026.08.19
ARG SHERPA_MODEL_RELEASE=https://github.com/k2-fsa/sherpa-onnx/releases/download
ARG SILERO_SHA256=c36d490aff5ab924ca6c7aeec4d8f6bd3d22db6fa17611b9c5b17eae58ac3a20
ARG SPEAKER_SHA256=357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b
ARG PARAKEET_SHA256=5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf

RUN apt-get update \
  && apt-get install -y --no-install-recommends bzip2 ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /models \
  && case "${TARGETARCH}" in \
    amd64) YT_DLP_ASSET=yt-dlp_linux; YT_DLP_SHA256=58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a ;; \
    arm64) YT_DLP_ASSET=yt-dlp_linux_aarch64; YT_DLP_SHA256=b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc ;; \
    *) echo "Unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
  esac \
  && curl -fsSL -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -o /yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/${YT_DLP_ASSET}" \
  && chmod 755 /yt-dlp \
  && curl -fsSL -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -o /models/silero_vad.int8.onnx \
    "${SHERPA_MODEL_RELEASE}/asr-models/silero_vad.int8.onnx" \
  && curl -fsSL -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -o /models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx \
    "${SHERPA_MODEL_RELEASE}/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx" \
  && curl -fsSL -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -o /tmp/parakeet.tar.bz2 \
    "${SHERPA_MODEL_RELEASE}/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2" \
  && echo "${YT_DLP_SHA256}  /yt-dlp" | sha256sum -c - \
  && echo "${SILERO_SHA256}  /models/silero_vad.int8.onnx" | sha256sum -c - \
  && echo "${SPEAKER_SHA256}  /models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx" | sha256sum -c - \
  && echo "${PARAKEET_SHA256}  /tmp/parakeet.tar.bz2" | sha256sum -c - \
  && tar -xjf /tmp/parakeet.tar.bz2 -C /models \
  && rm /tmp/parakeet.tar.bz2

# --- Runtime ---
# Node stays only as yt-dlp's JavaScript runtime (`--js-runtimes node`).
FROM node:24.19.0-slim AS runtime

# ffmpeg: PressPods audio pipeline (arnndn denoise, loudnorm, intro concat) and
#   livestream audio capture
# CUPS + brlaser + ghostscript + pdfinfo: bounded, model-aware PDF conversion for
#   Brother printing. The compatible HL-L2360D profile is physically verified on
#   this HL-L2370DW, including duplex.
# ca-certificates: rustls verifies TLS against the system trust store.
# tini: PID 1 that forwards signals and reaps orphaned subprocesses.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    cups \
    ffmpeg \
    ghostscript \
    poppler-utils \
    printer-driver-brlaser \
    tini \
  && mkdir -p /usr/share/omni-printing /tmp/brlaser-ppd \
  && ppdc -d /tmp/brlaser-ppd /usr/share/cups/drv/brlaser.drv \
  && cp /tmp/brlaser-ppd/brl2360d.ppd /usr/share/omni-printing/brother-hll2370dw.ppd \
  && rm -rf /tmp/brlaser-ppd \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=server /out/ /usr/local/bin/
COPY --from=web --chown=node:node /src/crates/omni-web/dist ./web
COPY --chown=node:node assets ./assets
COPY --from=livestream-assets --chown=node:node /models ./assets/livestream-intelligence/models
COPY --from=livestream-assets /yt-dlp /usr/local/bin/yt-dlp
COPY --chown=node:node docs/licenses ./licenses

RUN mkdir -p /data && chown node:node /data

ENV DOCKERIZED=true DB_NAME=/data/docstore.db OMNI_WEB_DIST=/app/web
USER node

# Fails the build when a runtime invariant is missing: ffmpeg arnndn,
# firequalizer and libmp3lame, the denoise model, the brlaser filter and PPD,
# cupsfilter, pdfinfo, the sherpa models and a runnable yt-dlp. DOCKERIZED is
# unset for this one command because production config validation requires
# runtime secrets (OMNI_MCP_TOKEN); none of the checked paths depend on it.
RUN DOCKERIZED=false omni-notify doctor --image

EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["omni-notify", "healthcheck"]

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["omni-notify"]
