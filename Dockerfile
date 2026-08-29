FROM node:24.19.0-slim AS build

ENV CI=true

RUN npm install -g pnpm@11.20.0

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches/ patches/
COPY frontend/package.json frontend/
RUN pnpm install --frozen-lockfile

COPY . .
RUN pnpm run build && pnpm --filter frontend run build
RUN pnpm prune --prod

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

FROM node:24.19.0-slim AS runtime

# ffmpeg: PressPods audio pipeline (loudnorm + intro concat)
# CUPS + brlaser + pdfinfo: bounded, model-aware PDF conversion for Brother printing
# The compatible HL-L2360D profile is physically verified on this HL-L2370DW, including duplex.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    cups \
    ffmpeg \
    ghostscript \
    poppler-utils \
    printer-driver-brlaser \
  && mkdir -p /usr/share/omni-printing /tmp/brlaser-ppd \
  && ppdc -d /tmp/brlaser-ppd /usr/share/cups/drv/brlaser.drv \
  && cp /tmp/brlaser-ppd/brl2360d.ppd /usr/share/omni-printing/brother-hll2370dw.ppd \
  && rm -rf /tmp/brlaser-ppd \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=build --chown=node:node /app/node_modules ./node_modules
COPY --from=build --chown=node:node /app/dist ./dist
COPY --from=build --chown=node:node /app/frontend/dist ./frontend/dist
COPY --from=build --chown=node:node /app/assets ./assets
COPY --from=livestream-assets --chown=node:node /models ./assets/livestream-intelligence/models
COPY --from=livestream-assets /yt-dlp /usr/local/bin/yt-dlp
COPY --from=build --chown=node:node /app/package.json ./package.json

RUN mkdir -p /data && chown node:node /data

ENV DOCKERIZED=true NODE_ENV=production DB_NAME=/data/docstore.db
USER node

EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["node", "-e", "fetch('http://127.0.0.1:'+(process.env.FRONTEND_PORT||3000)+'/api/health').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"]

CMD ["node", "dist/index.js"]
